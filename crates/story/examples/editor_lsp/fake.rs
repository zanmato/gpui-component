//! An in-process fake language server speaking real Content-Length framed
//! JSON-RPC over in-memory pipes, plus the automated tests that prove each
//! wired feature against it.

use gpui::{App, AppContext as _};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::client::LspClient;
use crate::transport::{MessageReader, MessageWriter};

type Handler = Box<dyn FnMut(Value) -> Value + Send>;

/// A scripted language server: canned per-method handlers, a record of
/// every message the client sent, and a way to push server-initiated
/// requests and notifications.
pub struct FakeServer {
    received: Arc<Mutex<Vec<Value>>>,
    handlers: Arc<Mutex<HashMap<String, Handler>>>,
    outbound_tx: async_channel::Sender<Value>,
}

impl FakeServer {
    /// Start the fake over in-memory pipes and return it with a client
    /// connected to it. `capabilities` is the raw `ServerCapabilities`
    /// JSON the initialize response reports.
    pub fn start(capabilities: Value, cx: &mut App) -> (Self, LspClient) {
        let (server_to_client_reader, server_to_client_writer) = piper::pipe(1024 * 1024);
        let (client_to_server_reader, client_to_server_writer) = piper::pipe(1024 * 1024);
        let client = LspClient::new(server_to_client_reader, client_to_server_writer, cx);

        let received: Arc<Mutex<Vec<Value>>> = Arc::default();
        let handlers: Arc<Mutex<HashMap<String, Handler>>> = Arc::default();
        let (outbound_tx, outbound_rx) = async_channel::unbounded::<Value>();

        cx.background_spawn({
            let mut writer = MessageWriter::new(server_to_client_writer);
            async move {
                while let Ok(message) = outbound_rx.recv().await {
                    let payload = serde_json::to_vec(&message).expect("serializable message");
                    if writer.write(&payload).await.is_err() {
                        break;
                    }
                }
            }
        })
        .detach();

        cx.background_spawn({
            let received = received.clone();
            let handlers = handlers.clone();
            let responses_tx = outbound_tx.clone();
            async move {
                let mut reader = MessageReader::new(client_to_server_reader);
                while let Ok(Some(payload)) = reader.read().await {
                    let message: Value =
                        serde_json::from_slice(&payload).expect("well-formed JSON-RPC");
                    received.lock().unwrap().push(message.clone());

                    let id = message.get("id").cloned();
                    let method = message.get("method").and_then(Value::as_str);
                    let (Some(id), Some(method)) = (id, method) else {
                        continue;
                    };
                    let params = message.get("params").cloned().unwrap_or(Value::Null);
                    let result = match method {
                        "initialize" => json!({ "capabilities": capabilities }),
                        "shutdown" => Value::Null,
                        _ => match handlers.lock().unwrap().get_mut(method) {
                            Some(handler) => handler(params),
                            None => Value::Null,
                        },
                    };
                    let _ = responses_tx
                        .send(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
                        .await;
                }
            }
        })
        .detach();

        (
            Self {
                received,
                handlers,
                outbound_tx,
            },
            client,
        )
    }

    /// Script the response for a request method.
    pub fn handle(&self, method: &str, handler: impl FnMut(Value) -> Value + Send + 'static) {
        self.handlers
            .lock()
            .unwrap()
            .insert(method.to_string(), Box::new(handler));
    }

    /// Push a raw server-to-client message (request or notification).
    pub fn send(&self, message: Value) {
        self.outbound_tx.try_send(message).expect("client alive");
    }

    /// Every message received for `method`, in arrival order.
    pub fn received(&self, method: &str) -> Vec<Value> {
        self.received
            .lock()
            .unwrap()
            .iter()
            .filter(|message| message.get("method").and_then(Value::as_str) == Some(method))
            .cloned()
            .collect()
    }

    /// Every response (a message without a method) the client sent back.
    pub fn received_responses(&self) -> Vec<Value> {
        self.received
            .lock()
            .unwrap()
            .iter()
            .filter(|message| message.get("method").is_none())
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wiring::{
        client_capabilities, install_providers, notify_document_changed, notify_document_opened,
    };
    use gpui::{Entity, Render, TestAppContext, VisualTestContext, div};
    use gpui_component::input::{EditorState, Rope};
    use lsp_types::Uri;

    struct EditorProbe {
        state: Entity<EditorState>,
    }

    impl Render for EditorProbe {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            div()
        }
    }

    fn document_uri() -> Uri {
        "file:///workspace/main.go".parse().unwrap()
    }

    fn initialize_params() -> lsp_types::InitializeParams {
        lsp_types::InitializeParams {
            capabilities: client_capabilities(),
            ..Default::default()
        }
    }

    fn build_editor(cx: &mut TestAppContext) -> (Entity<EditorState>, &mut VisualTestContext) {
        cx.update(gpui_component::init);
        let (probe, cx) = cx.add_window_view(|window, cx| EditorProbe {
            state: cx.new(|cx| EditorState::new(window, cx)),
        });
        let state = probe.read_with(cx, |probe, _| probe.state.clone());
        (state, cx)
    }

    #[gpui::test]
    async fn initialize_negotiates_utf16_and_stores_capabilities(cx: &mut TestAppContext) {
        let (server, client) =
            cx.update(|cx| FakeServer::start(json!({ "hoverProvider": true }), cx));

        client
            .initialize(initialize_params())
            .await
            .expect("initialize succeeds");
        cx.run_until_parked();

        let requests = server.received("initialize");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0]["params"]["capabilities"]["general"]["positionEncodings"],
            json!(["utf-16"])
        );
        assert_eq!(
            client.capabilities().and_then(|caps| caps.hover_provider),
            Some(lsp_types::HoverProviderCapability::Simple(true))
        );
        // The handshake was confirmed with the `initialized` notification.
        assert_eq!(server.received("initialized").len(), 1);
    }

    #[gpui::test]
    async fn document_sync_sends_full_text_with_monotonic_versions(cx: &mut TestAppContext) {
        let (server, client) = cx.update(|cx| FakeServer::start(json!({}), cx));
        client.initialize(initialize_params()).await.unwrap();

        let (editor, cx) = build_editor(cx);
        let uri = document_uri();

        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_value("package main", window, cx);
                notify_document_opened(&client, &uri, "go", state);
            });
        });
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_value("package main\n\nfunc main() {}", window, cx);
                notify_document_changed(&client, &uri, state);
            });
        });
        cx.run_until_parked();

        let opens = server.received("textDocument/didOpen");
        assert_eq!(opens.len(), 1);
        assert_eq!(
            opens[0]["params"]["textDocument"]["text"],
            json!("package main")
        );
        let open_version = opens[0]["params"]["textDocument"]["version"]
            .as_i64()
            .unwrap();

        let changes = server.received("textDocument/didChange");
        assert_eq!(changes.len(), 1);
        let change = &changes[0]["params"];
        // Full sync: one change event covering the whole document.
        assert_eq!(
            change["contentChanges"],
            json!([{ "text": "package main\n\nfunc main() {}" }])
        );
        assert!(change["textDocument"]["version"].as_i64().unwrap() > open_version);
    }

    #[gpui::test]
    async fn completion_requests_use_utf16_positions(cx: &mut TestAppContext) {
        let (server, client) = cx.update(|cx| {
            FakeServer::start(
                json!({ "completionProvider": { "triggerCharacters": ["."] } }),
                cx,
            )
        });
        client.initialize(initialize_params()).await.unwrap();
        server.handle("textDocument/completion", |_| json!([{ "label": "Greet" }]));

        let (editor, cx) = build_editor(cx);
        let uri = document_uri();
        // 世/界 are one UTF-16 unit each, 🌍 is a surrogate pair: the cursor
        // after "世界🌍x" is character 5 in UTF-16 but byte 11 in UTF-8.
        let text = "世界🌍x";
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_value(text, window, cx);
            });
            install_providers(&client, &editor, &uri, cx);
        });

        let task = cx.update(|window, cx| {
            let provider = editor.read(cx).lsp().completion_provider.clone().unwrap();
            provider.completions(
                &Rope::from(text),
                text.len(),
                lsp_types::CompletionContext {
                    trigger_kind: lsp_types::CompletionTriggerKind::TRIGGER_CHARACTER,
                    trigger_character: Some("x".into()),
                },
                window,
                cx,
            )
        });
        let response = task.await.expect("completion request succeeds");
        match response {
            lsp_types::CompletionResponse::Array(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].label, "Greet");
            }
            other => panic!("unexpected response shape: {other:?}"),
        }

        let requests = server.received("textDocument/completion");
        assert_eq!(requests.len(), 1);
        // `TextDocumentPositionParams` is flattened into the params object.
        let position = &requests[0]["params"]["position"];
        assert_eq!(position["line"], json!(0));
        assert_eq!(position["character"], json!(5));
        // A multi-char query is not a legal trigger character, so it is
        // reported as a plain invocation.
        assert_eq!(
            requests[0]["params"]["context"]["triggerKind"],
            json!(lsp_types::CompletionTriggerKind::INVOKED)
        );
    }

    #[gpui::test]
    async fn completion_resolve_supplies_documentation_and_additional_edits(
        cx: &mut TestAppContext,
    ) {
        let (server, client) = cx.update(|cx| {
            FakeServer::start(
                json!({ "completionProvider": { "resolveProvider": true } }),
                cx,
            )
        });
        client.initialize(initialize_params()).await.unwrap();
        cx.run_until_parked();

        // The handshake advertises what the resolve flow can honor.
        let initialize = &server.received("initialize")[0];
        assert_eq!(
            initialize["params"]["capabilities"]["textDocument"]["completion"]["completionItem"]["resolveSupport"]
                ["properties"],
            json!(["documentation", "additionalTextEdits"])
        );

        server.handle("completionItem/resolve", |mut item| {
            item["documentation"] = json!("Println formats and prints.");
            item["additionalTextEdits"] = json!([{
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 0 },
                },
                "newText": "import \"fmt\"\n",
            }]);
            item
        });

        let (editor, cx) = build_editor(cx);
        let uri = document_uri();
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_value("Prin", window, cx);
            });
            install_providers(&client, &editor, &uri, cx);
        });

        let bare_item = lsp_types::CompletionItem {
            label: "Println".into(),
            text_edit: Some(lsp_types::CompletionTextEdit::Edit(
                lsp_types::TextEdit::new(
                    lsp_types::Range::new(
                        lsp_types::Position::new(0, 0),
                        lsp_types::Position::new(0, 4),
                    ),
                    "Println".into(),
                ),
            )),
            ..Default::default()
        };
        let task = cx.update(|window, cx| {
            let provider = editor.read(cx).lsp().completion_provider.clone().unwrap();
            provider.resolve(bare_item, window, cx)
        });
        let resolved = task.await.expect("resolve succeeds");
        assert!(resolved.documentation.is_some());
        assert_eq!(
            resolved
                .additional_text_edits
                .as_ref()
                .map(|edits| edits.len()),
            Some(1)
        );

        // Confirming the resolved item applies the auto-import together
        // with the completion itself.
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.insert_completion(&resolved, 0..4, window, cx);
                assert_eq!(state.text().to_string(), "import \"fmt\"\nPrintln");
            });
        });
    }

    #[gpui::test]
    async fn publish_diagnostics_reach_the_diagnostic_set(cx: &mut TestAppContext) {
        let (server, client) = cx.update(|cx| FakeServer::start(json!({}), cx));
        client.initialize(initialize_params()).await.unwrap();

        let (editor, cx) = build_editor(cx);
        let uri = document_uri();
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_value("package main\nbroken", window, cx);
            });
        });
        {
            let editor = editor.clone();
            let uri = uri.clone();
            client.on_notification::<lsp_types::notification::PublishDiagnostics, _>(
                move |params, cx| {
                    if params.uri != uri {
                        return;
                    }
                    editor.update(cx, |state, cx| {
                        if let Some(set) = state.diagnostics_mut() {
                            set.clear();
                            set.extend(params.diagnostics);
                        }
                        cx.notify();
                    });
                },
            );
        }

        server.send(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": uri.as_str(),
                "diagnostics": [{
                    "range": {
                        "start": { "line": 1, "character": 0 },
                        "end": { "line": 1, "character": 6 },
                    },
                    "severity": 1,
                    "message": "expected declaration",
                }],
            },
        }));
        cx.run_until_parked();

        cx.update(|_, cx| {
            let state = editor.read(cx);
            let diagnostics = state.diagnostics().expect("editor mode has diagnostics");
            assert_eq!(diagnostics.len(), 1);
            let entry = diagnostics.iter().next().unwrap();
            assert_eq!(entry.diagnostic.message, "expected declaration");
        });
    }

    #[gpui::test]
    async fn apply_edit_requests_mutate_the_buffer_and_confirm(cx: &mut TestAppContext) {
        let (server, client) = cx.update(|cx| FakeServer::start(json!({}), cx));
        client.initialize(initialize_params()).await.unwrap();

        let (editor, cx) = build_editor(cx);
        let uri = document_uri();
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_value("hello world", window, cx);
                state.lsp_mut().set_document_uri(uri.clone());
            });
        });
        let window_handle = cx.update(|window, _| window.window_handle());
        {
            let editor = editor.clone();
            client.on_request::<lsp_types::request::ApplyWorkspaceEdit, _>(move |params, cx| {
                let applied = window_handle
                    .update(cx, |_, window, cx| {
                        editor.update(cx, |state, cx| {
                            state.apply_workspace_edit(&params.edit, window, cx)
                        })
                    })
                    .unwrap_or(false);
                Ok(lsp_types::ApplyWorkspaceEditResponse {
                    applied,
                    failure_reason: None,
                    failed_change: None,
                })
            });
        }

        let mut changes = serde_json::Map::new();
        changes.insert(
            uri.to_string(),
            json!([{
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 5 },
                },
                "newText": "goodbye",
            }]),
        );
        server.send(json!({
            "jsonrpc": "2.0",
            "id": 100,
            "method": "workspace/applyEdit",
            "params": { "edit": { "changes": changes } },
        }));
        cx.run_until_parked();

        cx.update(|_, cx| {
            assert_eq!(editor.read(cx).text().to_string(), "goodbye world");
        });
        let responses = server.received_responses();
        let response = responses
            .iter()
            .find(|response| response["id"] == json!(100))
            .expect("applyEdit was answered");
        assert_eq!(response["result"]["applied"], json!(true));
    }
}
