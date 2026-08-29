//! Adapters between the editor's provider traits and a live language
//! server: client capability declaration, document synchronization, and one
//! provider struct implementing every editor trait over LSP requests.

use anyhow::Result;
use gpui::{App, Entity, SharedString, Task, Window};
use gpui_component::input::{
    CodeActionProvider, CompletionProvider, DefinitionProvider, DocumentColorProvider,
    DocumentHighlightProvider, DocumentRangeSemanticTokensProvider, EditorState, HoverProvider,
    Rope, RopeExt, SignatureHelpProvider,
};
use lsp_types::{
    ClientCapabilities, CodeAction, CodeActionOrCommand, ColorInformation, CompletionContext,
    CompletionResponse, CompletionTriggerKind, GotoDefinitionResponse, LocationLink,
    SemanticTokens, SemanticTokensLegend, SemanticTokensRangeResult, TextDocumentIdentifier,
    TextDocumentPositionParams, Uri,
};
use std::{ops::Range, rc::Rc};

use crate::client::LspClient;

/// The capabilities this harness actually implements — kept in lockstep
/// with the features wired below, so the server never offers behavior the
/// editor cannot honor.
pub fn client_capabilities() -> ClientCapabilities {
    ClientCapabilities {
        general: Some(lsp_types::GeneralClientCapabilities {
            position_encodings: Some(vec![lsp_types::PositionEncodingKind::UTF16]),
            ..Default::default()
        }),
        text_document: Some(lsp_types::TextDocumentClientCapabilities {
            synchronization: Some(lsp_types::TextDocumentSyncClientCapabilities::default()),
            completion: Some(lsp_types::CompletionClientCapabilities {
                completion_item: Some(lsp_types::CompletionItemCapability {
                    snippet_support: Some(true),
                    documentation_format: Some(vec![
                        lsp_types::MarkupKind::Markdown,
                        lsp_types::MarkupKind::PlainText,
                    ]),
                    resolve_support: Some(lsp_types::CompletionItemCapabilityResolveSupport {
                        properties: vec!["documentation".into(), "additionalTextEdits".into()],
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            signature_help: Some(lsp_types::SignatureHelpClientCapabilities {
                signature_information: Some(lsp_types::SignatureInformationSettings {
                    documentation_format: Some(vec![
                        lsp_types::MarkupKind::Markdown,
                        lsp_types::MarkupKind::PlainText,
                    ]),
                    parameter_information: Some(lsp_types::ParameterInformationSettings {
                        label_offset_support: Some(true),
                    }),
                    active_parameter_support: Some(true),
                }),
                context_support: Some(true),
                ..Default::default()
            }),
            hover: Some(lsp_types::HoverClientCapabilities {
                content_format: Some(vec![
                    lsp_types::MarkupKind::Markdown,
                    lsp_types::MarkupKind::PlainText,
                ]),
                ..Default::default()
            }),
            definition: Some(lsp_types::GotoCapability {
                link_support: Some(true),
                ..Default::default()
            }),
            code_action: Some(lsp_types::CodeActionClientCapabilities {
                code_action_literal_support: Some(lsp_types::CodeActionLiteralSupport {
                    code_action_kind: lsp_types::CodeActionKindLiteralSupport {
                        value_set: vec!["quickfix".into(), "refactor".into(), "source".into()],
                    },
                }),
                ..Default::default()
            }),
            document_highlight: Some(lsp_types::DocumentHighlightClientCapabilities::default()),
            color_provider: Some(lsp_types::DocumentColorClientCapabilities::default()),
            semantic_tokens: Some(lsp_types::SemanticTokensClientCapabilities {
                requests: lsp_types::SemanticTokensClientCapabilitiesRequests {
                    range: Some(true),
                    full: None,
                },
                token_types: vec![
                    lsp_types::SemanticTokenType::NAMESPACE,
                    lsp_types::SemanticTokenType::TYPE,
                    lsp_types::SemanticTokenType::STRUCT,
                    lsp_types::SemanticTokenType::PARAMETER,
                    lsp_types::SemanticTokenType::VARIABLE,
                    lsp_types::SemanticTokenType::PROPERTY,
                    lsp_types::SemanticTokenType::FUNCTION,
                    lsp_types::SemanticTokenType::METHOD,
                    lsp_types::SemanticTokenType::KEYWORD,
                    lsp_types::SemanticTokenType::COMMENT,
                    lsp_types::SemanticTokenType::STRING,
                    lsp_types::SemanticTokenType::NUMBER,
                    lsp_types::SemanticTokenType::OPERATOR,
                ],
                token_modifiers: vec![],
                formats: vec![lsp_types::TokenFormat::RELATIVE],
                ..Default::default()
            }),
            publish_diagnostics: Some(lsp_types::PublishDiagnosticsClientCapabilities::default()),
            ..Default::default()
        }),
        workspace: Some(lsp_types::WorkspaceClientCapabilities {
            apply_edit: Some(true),
            workspace_edit: Some(lsp_types::WorkspaceEditClientCapabilities {
                document_changes: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }),
        window: Some(lsp_types::WindowClientCapabilities {
            show_document: Some(lsp_types::ShowDocumentClientCapabilities { support: true }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Announce the document to the server (`textDocument/didOpen`).
pub fn notify_document_opened(
    client: &LspClient,
    uri: &Uri,
    language_id: &str,
    state: &EditorState,
) {
    client.notify::<lsp_types::notification::DidOpenTextDocument>(
        lsp_types::DidOpenTextDocumentParams {
            text_document: lsp_types::TextDocumentItem {
                uri: uri.clone(),
                language_id: language_id.to_string(),
                version: state.document_version() as i32,
                text: state.text().to_string(),
            },
        },
    );
}

/// Send the document's full new content (`textDocument/didChange`, full
/// sync). The version is the editor's own monotonic document version, so
/// the server and the editor agree about which snapshot a response
/// belongs to.
pub fn notify_document_changed(client: &LspClient, uri: &Uri, state: &EditorState) {
    client.notify::<lsp_types::notification::DidChangeTextDocument>(
        lsp_types::DidChangeTextDocumentParams {
            text_document: lsp_types::VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: state.document_version() as i32,
            },
            content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: state.text().to_string(),
            }],
        },
    );
}

/// One provider object implementing every editor trait over the client.
pub struct ServerProviders {
    client: LspClient,
    uri: Uri,
    completion_triggers: Vec<String>,
    signature_help_triggers: Vec<String>,
    signature_help_retriggers: Vec<String>,
    semantic_tokens_legend: SemanticTokensLegend,
}

impl ServerProviders {
    fn document_position(&self, text: &Rope, offset: usize) -> TextDocumentPositionParams {
        TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: self.uri.clone(),
            },
            position: text.offset_to_position(offset),
        }
    }
}

/// Install providers on the editor for exactly the features the server's
/// reported capabilities cover. Must be called after the initialize
/// handshake completed.
pub fn install_providers(
    client: &LspClient,
    editor: &Entity<EditorState>,
    uri: &Uri,
    cx: &mut App,
) {
    let Some(capabilities) = client.capabilities() else {
        return;
    };

    let completion_triggers = capabilities
        .completion_provider
        .as_ref()
        .and_then(|options| options.trigger_characters.clone())
        .unwrap_or_default();
    let semantic_tokens = capabilities.semantic_tokens_provider.as_ref().map(|caps| {
        let options = match caps {
            lsp_types::SemanticTokensServerCapabilities::SemanticTokensOptions(options) => options,
            lsp_types::SemanticTokensServerCapabilities::SemanticTokensRegistrationOptions(
                registration,
            ) => &registration.semantic_tokens_options,
        };
        (options.legend.clone(), options.range.unwrap_or(false))
    });

    let signature_help_options = capabilities.signature_help_provider.as_ref();
    let providers = Rc::new(ServerProviders {
        client: client.clone(),
        uri: uri.clone(),
        completion_triggers,
        signature_help_triggers: signature_help_options
            .and_then(|options| options.trigger_characters.clone())
            .unwrap_or_default(),
        signature_help_retriggers: signature_help_options
            .and_then(|options| options.retrigger_characters.clone())
            .unwrap_or_default(),
        semantic_tokens_legend: semantic_tokens
            .as_ref()
            .map(|(legend, _)| legend.clone())
            .unwrap_or_default(),
    });

    editor.update(cx, |state, cx| {
        let lsp = state.lsp_mut();
        lsp.set_document_uri(uri.clone());
        if capabilities.completion_provider.is_some() {
            lsp.completion_provider = Some(providers.clone());
        }
        let hover_enabled = match &capabilities.hover_provider {
            None => false,
            Some(lsp_types::HoverProviderCapability::Simple(enabled)) => *enabled,
            Some(lsp_types::HoverProviderCapability::Options(_)) => true,
        };
        if hover_enabled {
            lsp.hover_provider = Some(providers.clone());
        }
        if truthy(&capabilities.definition_provider) {
            lsp.definition_provider = Some(providers.clone());
        }
        if capabilities.code_action_provider.is_some() {
            lsp.code_action_providers = vec![providers.clone()];
        }
        if capabilities.signature_help_provider.is_some() {
            lsp.signature_help_provider = Some(providers.clone());
        }
        if truthy(&capabilities.document_highlight_provider) {
            lsp.document_highlight_provider = Some(providers.clone());
        }
        if capabilities.color_provider.is_some() {
            lsp.document_color_provider = Some(providers.clone());
        }
        if semantic_tokens.is_some_and(|(_, range)| range) {
            lsp.semantic_tokens_provider = Some(providers.clone());
        }
        cx.notify();
    });
}

/// Whether an `Option<OneOf<bool, _>>`-shaped server capability is enabled.
fn truthy<T>(capability: &Option<lsp_types::OneOf<bool, T>>) -> bool {
    match capability {
        None => false,
        Some(lsp_types::OneOf::Left(enabled)) => *enabled,
        Some(lsp_types::OneOf::Right(_)) => true,
    }
}

impl CompletionProvider for ServerProviders {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        // The editor reports the whole query string as the trigger
        // character; real servers only accept their declared single-char
        // triggers, anything else is a plain invocation.
        let context = match &trigger.trigger_character {
            Some(query) if self.completion_triggers.contains(query) => CompletionContext {
                trigger_kind: CompletionTriggerKind::TRIGGER_CHARACTER,
                trigger_character: Some(query.clone()),
            },
            _ => CompletionContext {
                trigger_kind: CompletionTriggerKind::INVOKED,
                trigger_character: None,
            },
        };
        let request =
            self.client
                .request::<lsp_types::request::Completion>(lsp_types::CompletionParams {
                    text_document_position: self.document_position(text, offset),
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                    context: Some(context),
                });
        cx.spawn(async move |_| Ok(request.await?.unwrap_or(CompletionResponse::Array(vec![]))))
    }

    fn resolve(
        &self,
        item: lsp_types::CompletionItem,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<lsp_types::CompletionItem>> {
        let request = self
            .client
            .request::<lsp_types::request::ResolveCompletionItem>(item);
        cx.spawn(async move |_| request.await)
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        let Some(last) = new_text.chars().last() else {
            return false;
        };
        last.is_alphanumeric()
            || last == '_'
            || self
                .completion_triggers
                .iter()
                .any(|trigger| trigger == last.to_string().as_str())
    }
}

impl HoverProvider for ServerProviders {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<lsp_types::Hover>>> {
        let request =
            self.client
                .request::<lsp_types::request::HoverRequest>(lsp_types::HoverParams {
                    text_document_position_params: self.document_position(text, offset),
                    work_done_progress_params: Default::default(),
                });
        cx.spawn(async move |_| request.await)
    }
}

impl SignatureHelpProvider for ServerProviders {
    fn signature_help(
        &self,
        text: &Rope,
        offset: usize,
        context: lsp_types::SignatureHelpContext,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<lsp_types::SignatureHelp>>> {
        let request = self
            .client
            .request::<lsp_types::request::SignatureHelpRequest>(lsp_types::SignatureHelpParams {
                text_document_position_params: self.document_position(text, offset),
                work_done_progress_params: Default::default(),
                context: Some(context),
            });
        cx.spawn(async move |_| request.await)
    }

    fn trigger_characters(&self) -> Vec<String> {
        self.signature_help_triggers.clone()
    }

    fn retrigger_characters(&self) -> Vec<String> {
        self.signature_help_retriggers.clone()
    }
}

impl DocumentHighlightProvider for ServerProviders {
    fn document_highlights(
        &self,
        text: &Rope,
        offset: usize,
        cx: &mut App,
    ) -> Task<Result<Vec<lsp_types::DocumentHighlight>>> {
        let request = self
            .client
            .request::<lsp_types::request::DocumentHighlightRequest>(
                lsp_types::DocumentHighlightParams {
                    text_document_position_params: self.document_position(text, offset),
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                },
            );
        cx.spawn(async move |_| Ok(request.await?.unwrap_or_default()))
    }
}

impl DefinitionProvider for ServerProviders {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>> {
        let request = self.client.request::<lsp_types::request::GotoDefinition>(
            lsp_types::GotoDefinitionParams {
                text_document_position_params: self.document_position(text, offset),
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            },
        );
        cx.spawn(async move |_| {
            let links = match request.await? {
                None => vec![],
                Some(GotoDefinitionResponse::Scalar(location)) => vec![location_link(location)],
                Some(GotoDefinitionResponse::Array(locations)) => {
                    locations.into_iter().map(location_link).collect()
                }
                Some(GotoDefinitionResponse::Link(links)) => links,
            };
            Ok(links)
        })
    }
}

fn location_link(location: lsp_types::Location) -> LocationLink {
    LocationLink {
        origin_selection_range: None,
        target_uri: location.uri,
        target_range: location.range,
        target_selection_range: location.range,
    }
}

impl CodeActionProvider for ServerProviders {
    fn id(&self) -> SharedString {
        "language-server".into()
    }

    fn code_actions(
        &self,
        state: Entity<EditorState>,
        range: Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<CodeAction>>> {
        let text = state.read(cx).text().clone();
        let request = self
            .client
            .request::<lsp_types::request::CodeActionRequest>(lsp_types::CodeActionParams {
                text_document: TextDocumentIdentifier {
                    uri: self.uri.clone(),
                },
                range: lsp_types::Range {
                    start: text.offset_to_position(range.start),
                    end: text.offset_to_position(range.end),
                },
                context: lsp_types::CodeActionContext::default(),
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            });
        cx.spawn(async move |_| {
            let actions = request
                .await?
                .unwrap_or_default()
                .into_iter()
                .filter_map(|action| match action {
                    CodeActionOrCommand::CodeAction(action) => Some(action),
                    CodeActionOrCommand::Command(_) => None,
                })
                .collect();
            Ok(actions)
        })
    }

    fn perform_code_action(
        &self,
        state: Entity<EditorState>,
        action: CodeAction,
        _push_to_history: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        if let Some(edit) = &action.edit {
            state.update(cx, |state, cx| {
                state.apply_workspace_edit(edit, window, cx);
            });
        }
        // Actions carrying only a command need workspace/executeCommand,
        // which this harness does not implement yet.
        Task::ready(Ok(()))
    }
}

impl DocumentColorProvider for ServerProviders {
    fn document_colors(
        &self,
        _text: &Rope,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<ColorInformation>>> {
        let request = self.client.request::<lsp_types::request::DocumentColor>(
            lsp_types::DocumentColorParams {
                text_document: TextDocumentIdentifier {
                    uri: self.uri.clone(),
                },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            },
        );
        cx.spawn(async move |_| request.await)
    }
}

impl DocumentRangeSemanticTokensProvider for ServerProviders {
    fn legend(&self) -> SemanticTokensLegend {
        self.semantic_tokens_legend.clone()
    }

    fn semantic_tokens(
        &self,
        text: &Rope,
        range: Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<SemanticTokens>> {
        let request = self
            .client
            .request::<lsp_types::request::SemanticTokensRangeRequest>(
                lsp_types::SemanticTokensRangeParams {
                    text_document: TextDocumentIdentifier {
                        uri: self.uri.clone(),
                    },
                    range: lsp_types::Range {
                        start: text.offset_to_position(range.start),
                        end: text.offset_to_position(range.end),
                    },
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                },
            );
        cx.spawn(async move |_| {
            let tokens = match request.await? {
                Some(SemanticTokensRangeResult::Tokens(tokens)) => tokens,
                Some(SemanticTokensRangeResult::Partial(partial)) => SemanticTokens {
                    result_id: None,
                    data: partial.data,
                },
                None => SemanticTokens::default(),
            };
            Ok(tokens)
        })
    }
}
