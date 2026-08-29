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

    /// The method names of every request and notification received, in
    /// arrival order — for asserting cross-method ordering.
    pub fn received_methods(&self) -> Vec<String> {
        self.received
            .lock()
            .unwrap()
            .iter()
            .filter_map(|message| message.get("method").and_then(Value::as_str))
            .map(str::to_string)
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
    async fn signature_help_requests_carry_trigger_context(cx: &mut TestAppContext) {
        let (server, client) = cx.update(|cx| {
            FakeServer::start(
                json!({ "signatureHelpProvider": { "triggerCharacters": ["(", ","] } }),
                cx,
            )
        });
        client.initialize(initialize_params()).await.unwrap();
        server.handle("textDocument/signatureHelp", |_| {
            json!({
                "signatures": [{
                    "label": "func Fprintf(w io.Writer, format string, a ...any)",
                    "parameters": [
                        { "label": "w io.Writer" },
                        { "label": "format string" },
                    ],
                }],
                "activeSignature": 0,
                "activeParameter": 0,
            })
        });

        let (editor, cx) = build_editor(cx);
        let uri = document_uri();
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_value("fmt.Fprintf", window, cx);
            });
            install_providers(&client, &editor, &uri, cx);
        });

        let task = cx.update(|window, cx| {
            let provider = editor
                .read(cx)
                .lsp()
                .signature_help_provider
                .clone()
                .unwrap();
            provider.signature_help(
                &Rope::from("fmt.Fprintf("),
                12,
                lsp_types::SignatureHelpContext {
                    trigger_kind: lsp_types::SignatureHelpTriggerKind::TRIGGER_CHARACTER,
                    trigger_character: Some("(".into()),
                    is_retrigger: false,
                    active_signature_help: None,
                },
                window,
                cx,
            )
        });
        let help = task.await.expect("request succeeds").expect("has help");
        assert_eq!(help.signatures.len(), 1);
        assert_eq!(help.active_parameter, Some(0));

        let requests = server.received("textDocument/signatureHelp");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0]["params"]["context"]["triggerCharacter"],
            json!("(")
        );
        assert_eq!(requests[0]["params"]["position"]["character"], json!(12));
        // The provider surfaced the server's declared trigger characters,
        // which drive the editor-side trigger logic.
        cx.update(|_, cx| {
            let provider = editor
                .read(cx)
                .lsp()
                .signature_help_provider
                .clone()
                .unwrap();
            assert_eq!(provider.trigger_characters(), vec!["(", ","]);
        });
    }

    #[gpui::test]
    async fn formatting_requests_carry_tab_options_and_ranges(cx: &mut TestAppContext) {
        let (server, client) = cx.update(|cx| {
            FakeServer::start(
                json!({
                    "documentFormattingProvider": true,
                    "documentRangeFormattingProvider": true,
                }),
                cx,
            )
        });
        client.initialize(initialize_params()).await.unwrap();
        server.handle("textDocument/formatting", |_| json!([]));
        server.handle("textDocument/rangeFormatting", |_| json!([]));

        let (editor, cx) = build_editor(cx);
        let uri = document_uri();
        let text = "hello world";
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_value(text, window, cx);
            });
            install_providers(&client, &editor, &uri, cx);
        });

        let provider =
            cx.update(|_, cx| editor.read(cx).lsp().formatting_provider.clone().unwrap());
        assert!(provider.supports_format());
        assert!(provider.supports_range_format());

        let options = lsp_types::FormattingOptions {
            tab_size: 4,
            insert_spaces: false,
            ..Default::default()
        };
        let task =
            cx.update(|window, cx| provider.format(&Rope::from(text), options.clone(), window, cx));
        task.await.expect("format request succeeds");

        let requests = server.received("textDocument/formatting");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["params"]["options"]["tabSize"], json!(4));
        assert_eq!(
            requests[0]["params"]["options"]["insertSpaces"],
            json!(false)
        );

        // Range formatting converts the byte range to UTF-16 positions.
        let task = cx.update(|window, cx| {
            provider.range_format(&Rope::from(text), 6..11, options, window, cx)
        });
        task.await.expect("range format request succeeds");

        let requests = server.received("textDocument/rangeFormatting");
        assert_eq!(requests.len(), 1);
        let range = &requests[0]["params"]["range"];
        assert_eq!(range["start"]["character"], json!(6));
        assert_eq!(range["end"]["character"], json!(11));
    }

    #[gpui::test]
    async fn on_type_formatting_uses_the_servers_trigger_characters(cx: &mut TestAppContext) {
        let (server, client) = cx.update(|cx| {
            FakeServer::start(
                json!({
                    "documentOnTypeFormattingProvider": {
                        "firstTriggerCharacter": "}",
                        "moreTriggerCharacter": [";"],
                    },
                }),
                cx,
            )
        });
        client.initialize(initialize_params()).await.unwrap();
        server.handle("textDocument/onTypeFormatting", |_| json!([]));

        let (editor, cx) = build_editor(cx);
        let uri = document_uri();
        let text = "func main() {}";
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_value(text, window, cx);
            });
            install_providers(&client, &editor, &uri, cx);
        });

        let provider = cx.update(|_, cx| {
            editor
                .read(cx)
                .lsp()
                .on_type_formatting_provider
                .clone()
                .unwrap()
        });
        // Both declared trigger characters drive the editor-side check.
        assert_eq!(provider.trigger_characters(), vec!["}", ";"]);

        let task = cx.update(|window, cx| {
            provider.on_type_format(
                &Rope::from(text),
                text.len(),
                "}",
                lsp_types::FormattingOptions {
                    tab_size: 2,
                    insert_spaces: true,
                    ..Default::default()
                },
                window,
                cx,
            )
        });
        task.await.expect("on-type format request succeeds");

        let requests = server.received("textDocument/onTypeFormatting");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["params"]["ch"], json!("}"));
        assert_eq!(requests[0]["params"]["position"]["character"], json!(14));
        assert_eq!(requests[0]["params"]["options"]["tabSize"], json!(2));
    }

    #[gpui::test]
    async fn inlay_hints_request_the_whole_document(cx: &mut TestAppContext) {
        let (server, client) =
            cx.update(|cx| FakeServer::start(json!({ "inlayHintProvider": true }), cx));
        client.initialize(initialize_params()).await.unwrap();
        server.handle("textDocument/inlayHint", |_| {
            json!([{
                "position": { "line": 0, "character": 9 },
                "label": [{ "value": "n:" }, { "value": " int" }],
                "paddingRight": true,
            }])
        });

        let (editor, cx) = build_editor(cx);
        let uri = document_uri();
        let text = "add(1, 2)\n";
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_value(text, window, cx);
            });
            install_providers(&client, &editor, &uri, cx);
        });

        let provider =
            cx.update(|_, cx| editor.read(cx).lsp().inlay_hint_provider.clone().unwrap());
        let task = cx.update(|window, cx| {
            provider.inlay_hints(&Rope::from(text), 0..text.len(), window, cx)
        });
        let hints = task.await.expect("inlay hint request succeeds");
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].position, lsp_types::Position::new(0, 9));

        let requests = server.received("textDocument/inlayHint");
        assert_eq!(requests.len(), 1);
        let range = &requests[0]["params"]["range"];
        assert_eq!(range["start"], json!({ "line": 0, "character": 0 }));
        assert_eq!(range["end"], json!({ "line": 1, "character": 0 }));
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
    async fn typing_syncs_the_document_before_the_completion_request(cx: &mut TestAppContext) {
        let (server, client) =
            cx.update(|cx| FakeServer::start(json!({ "completionProvider": {} }), cx));
        client.initialize(initialize_params()).await.unwrap();
        server.handle(
            "textDocument/completion",
            |_| json!([{ "label": "Println" }]),
        );

        let (editor, cx) = build_editor(cx);
        let uri = document_uri();
        let _subscription = cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_value("fmt.Pr", window, cx);
                state.set_selected_range(6..6, cx);
                notify_document_opened(&client, &uri, "go", state);
            });
            install_providers(&client, &editor, &uri, cx);
            // The example forwards every buffer change, like main.rs does.
            cx.subscribe(&editor, {
                let client = client.clone();
                let uri = uri.clone();
                move |editor, event: &gpui_component::input::InputEvent, cx| {
                    if matches!(event, gpui_component::input::InputEvent::Change) {
                        notify_document_changed(&client, &uri, editor.read(cx));
                    }
                }
            })
        });
        cx.run_until_parked();
        assert_eq!(server.received("textDocument/didOpen").len(), 1);

        // Typing a character fires the completion request from inside the
        // edit; the didChange describing that edit must reach the server
        // first, or it resolves the position against a stale document.
        cx.update(|window, cx| {
            use gpui::Focusable as _;
            editor.read(cx).focus_handle(cx).focus(window, cx);
            editor.update(cx, |state, cx| {
                use gpui::EntityInputHandler as _;
                state.replace_text_in_range(Some(6..6), "i", window, cx);
            });
        });
        cx.run_until_parked();

        let methods = server.received_methods();
        let change_at = methods
            .iter()
            .position(|method| method == "textDocument/didChange")
            .expect("didChange was sent");
        let completion_at = methods
            .iter()
            .position(|method| method == "textDocument/completion")
            .expect("completion was requested");
        assert!(
            change_at < completion_at,
            "didChange must precede the completion it positions into, got {methods:?}"
        );

        let completions = server.received("textDocument/completion");
        assert_eq!(
            completions[0]["params"]["position"],
            json!({ "line": 0, "character": 7 })
        );
        // The scripted response opened the menu.
        cx.update(|_, cx| {
            assert!(editor.read(cx).completion_menu_state().open);
        });
    }

    #[gpui::test]
    async fn full_lifecycle_from_open_to_shutdown(cx: &mut TestAppContext) {
        let (server, client) = cx.update(|cx| {
            FakeServer::start(
                json!({
                    "hoverProvider": true,
                    "definitionProvider": true,
                    "referencesProvider": true,
                    "documentSymbolProvider": true,
                    "documentFormattingProvider": true,
                    "inlayHintProvider": true,
                    "renameProvider": { "prepareProvider": true },
                }),
                cx,
            )
        });
        client.initialize(initialize_params()).await.unwrap();
        server.handle(
            "textDocument/hover",
            |_| json!({ "contents": "Greeter says hello." }),
        );
        server.handle("textDocument/formatting", |_| json!([]));

        let (editor, cx) = build_editor(cx);
        let uri = document_uri();
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_value("package main", window, cx);
                notify_document_opened(&client, &uri, "go", state);
            });
            install_providers(&client, &editor, &uri, cx);
        });

        // Every advertised capability got its provider slot; the rest
        // stayed unwired.
        cx.update(|_, cx| {
            let state = editor.read(cx);
            assert!(state.lsp().hover_provider.is_some());
            assert!(state.lsp().definition_provider.is_some());
            assert!(state.lsp().references_provider.is_some());
            assert!(state.lsp().document_symbol_provider.is_some());
            assert!(state.lsp().formatting_provider.is_some());
            assert!(state.lsp().inlay_hint_provider.is_some());
            assert!(state.lsp().rename_provider.is_some());
            assert!(state.lsp().completion_provider.is_none());
            assert!(state.lsp().signature_help_provider.is_none());
        });

        // Open, request, edit, request again: versions stay monotonic and
        // the requests round-trip.
        let task = cx.update(|window, cx| {
            let provider = editor.read(cx).lsp().hover_provider.clone().unwrap();
            provider.hover(&Rope::from("package main"), 8, window, cx)
        });
        assert!(task.await.expect("hover succeeds").is_some());

        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_value("package main\n\nfunc main() {}", window, cx);
                notify_document_changed(&client, &uri, state);
            });
        });
        let task = cx.update(|window, cx| {
            let provider = editor.read(cx).lsp().formatting_provider.clone().unwrap();
            provider.format(
                &Rope::from("package main\n\nfunc main() {}"),
                lsp_types::FormattingOptions {
                    tab_size: 4,
                    insert_spaces: false,
                    ..Default::default()
                },
                window,
                cx,
            )
        });
        task.await.expect("format succeeds");
        cx.run_until_parked();

        let opens = server.received("textDocument/didOpen");
        let changes = server.received("textDocument/didChange");
        assert_eq!(opens.len(), 1);
        assert_eq!(changes.len(), 1);
        assert!(
            changes[0]["params"]["textDocument"]["version"]
                .as_i64()
                .unwrap()
                > opens[0]["params"]["textDocument"]["version"]
                    .as_i64()
                    .unwrap()
        );

        // The shutdown sequence: the request is answered, then the exit
        // notification follows.
        client.shutdown().await.expect("shutdown succeeds");
        cx.run_until_parked();
        assert_eq!(server.received("shutdown").len(), 1);
        assert_eq!(server.received("exit").len(), 1);
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
