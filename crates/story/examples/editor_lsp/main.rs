//! An editor connected to a real language server over stdio.
//!
//! Run with [gopls](https://pkg.go.dev/golang.org/x/tools/gopls) on PATH:
//!
//! ```sh
//! cargo run -p gpui-component-story --example editor_lsp
//! ```
//!
//! The harness spawns gopls over the embedded `testdata/` Go module,
//! negotiates UTF-16 positions, keeps the document synchronized with full
//! `didChange` notifications, and wires the editor's provider traits —
//! completion, hover, go-to-definition, code actions, semantic tokens —
//! to the corresponding language server requests. Diagnostics are pushed
//! into the editor as they arrive and `workspace/applyEdit` requests
//! mutate the buffer.

mod client;
mod transport;
mod wiring;

#[cfg(test)]
mod fake;

use std::path::PathBuf;

use gpui::{
    AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement, Render,
    SharedString, Styled, Subscription, Window, px, size,
};
use gpui_component::{
    ActiveTheme, h_flex,
    input::{Editor, EditorState, InputEvent, TabSize},
    label::Label,
    v_flex,
};
use gpui_component_assets::Assets;
use lsp_types::Uri;

use crate::client::LspClient;
use crate::wiring::{
    client_capabilities, install_providers, notify_document_changed, notify_document_opened,
};

const LANGUAGE_ID: &str = "go";
const SERVER_COMMAND: &str = "gopls";

struct LspSession {
    client: LspClient,
    uri: Uri,
    _child: smol::process::Child,
}

pub struct Example {
    editor: Entity<EditorState>,
    status: SharedString,
    lsp: Option<LspSession>,
    _subscriptions: Vec<Subscription>,
}

fn testdata_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/editor_lsp/testdata")
}

fn file_uri(path: &std::path::Path) -> Uri {
    format!("file://{}", path.display())
        .parse()
        .expect("valid file uri")
}

impl Example {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let root = testdata_dir();
        let document_path = root.join("main.go");
        let text = std::fs::read_to_string(&document_path).expect("read testdata/main.go");
        let uri = file_uri(&document_path);

        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(LANGUAGE_ID)
                .line_number(true)
                .indent_guides(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: true,
                })
                .soft_wrap(false)
                .default_value(text)
        });

        let focus_handle = editor.focus_handle(cx);
        window.defer(cx, move |window, cx| {
            focus_handle.focus(window, cx);
        });

        let (status, lsp) = match Self::start_language_server(&editor, &uri, &root, window, cx) {
            Ok(session) => (format!("starting {SERVER_COMMAND}…").into(), Some(session)),
            Err(error) => (
                format!("{SERVER_COMMAND} unavailable: {error} — editing works without language features")
                    .into(),
                None,
            ),
        };

        let _subscriptions = vec![
            cx.subscribe(&editor, |this, editor, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change)
                    && let Some(session) = &this.lsp
                {
                    notify_document_changed(&session.client, &session.uri, editor.read(cx));
                }
            }),
        ];

        Self {
            editor,
            status,
            lsp,
            _subscriptions,
        }
    }

    fn start_language_server(
        editor: &Entity<EditorState>,
        uri: &Uri,
        root: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<LspSession> {
        let mut command = smol::process::Command::new(SERVER_COMMAND);
        command.current_dir(root);
        let (client, child) = LspClient::connect_to_command(command, cx)?;

        Self::register_handlers(&client, editor, uri, window, cx);

        let root_uri = file_uri(root);
        #[allow(deprecated)]
        let initialize = client.initialize(lsp_types::InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(root_uri.clone()),
            capabilities: client_capabilities(),
            workspace_folders: Some(vec![lsp_types::WorkspaceFolder {
                uri: root_uri,
                name: "testdata".into(),
            }]),
            ..Default::default()
        });

        cx.spawn_in(window, {
            async move |this, cx| {
                let result = initialize.await;
                this.update(cx, |this, cx| {
                    match result {
                        Ok(_) => {
                            let Some(session) = &this.lsp else {
                                return;
                            };
                            install_providers(&session.client, &this.editor, &session.uri, cx);
                            this.editor.update(cx, |state, _| {
                                notify_document_opened(
                                    &session.client,
                                    &session.uri,
                                    LANGUAGE_ID,
                                    state,
                                );
                            });
                            this.status = format!("{SERVER_COMMAND} ready").into();
                        }
                        Err(error) => {
                            this.status = format!("initialize failed: {error}").into();
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();

        let shutdown_client = client.clone();
        cx.on_app_quit(move |_, _| {
            let client = shutdown_client.clone();
            async move {
                let _ = client.shutdown().await;
            }
        })
        .detach();

        Ok(LspSession {
            client,
            uri: uri.clone(),
            _child: child,
        })
    }

    fn register_handlers(
        client: &LspClient,
        editor: &Entity<EditorState>,
        uri: &Uri,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let document_uri = uri.clone();
        let diagnostics_editor = editor.clone();
        client.on_notification::<lsp_types::notification::PublishDiagnostics, _>(
            move |params, cx| {
                if params.uri != document_uri {
                    return;
                }
                diagnostics_editor.update(cx, |state, cx| {
                    if let Some(set) = state.diagnostics_mut() {
                        set.clear();
                        set.extend(params.diagnostics);
                    }
                    cx.notify();
                });
            },
        );

        let window_handle = window.window_handle();
        let apply_edit_editor = editor.clone();
        client.on_request::<lsp_types::request::ApplyWorkspaceEdit, _>(move |params, cx| {
            let applied = window_handle
                .update(cx, |_, window, cx| {
                    apply_edit_editor.update(cx, |state, cx| {
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

        client.on_request::<lsp_types::request::ShowDocument, _>(|params, cx| {
            let external = params.external.unwrap_or(false)
                || params.uri.scheme().is_some_and(|s| s.as_str() != "file");
            if external {
                cx.open_url(params.uri.as_str());
            }
            Ok(lsp_types::ShowDocumentResult { success: external })
        });

        client.on_request::<lsp_types::request::WorkspaceConfiguration, _>(|params, _| {
            Ok(params
                .items
                .iter()
                .map(|_| serde_json::Value::Null)
                .collect())
        });

        client.on_request::<lsp_types::request::WorkDoneProgressCreate, _>(|_, _| Ok(()));

        client.on_notification::<lsp_types::notification::LogMessage, _>(|params, _| {
            tracing::debug!(target: "editor_lsp", "server log: {}", params.message);
        });
    }
}

impl Render for Example {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(
                Editor::new(&self.editor)
                    .bordered(false)
                    .p_0()
                    .h_full()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(cx.theme().mono_font_size),
            )
            .child(
                h_flex()
                    .w_full()
                    .px_3()
                    .py_1()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(Label::new(self.status.clone()).text_sm()),
            )
    }
}

fn main() {
    let app = gpui_platform::application().with_assets(Assets);

    app.run(move |cx| {
        gpui_component_story::init(cx);
        cx.activate(true);

        gpui_component_story::create_new_window_with_size(
            "Editor LSP",
            Some(size(px(1000.), px(700.))),
            |window, cx| cx.new(|cx| Example::new(window, cx)),
            cx,
        );
    });
}
