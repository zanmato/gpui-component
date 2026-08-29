use anyhow::Result;
use gpui::{App, Context, Hsla, SharedString, Task, Window};
use ropey::Rope;
use std::rc::Rc;

use crate::input::{EditorMode, InputBaseState};

mod code_actions;
mod completions;
mod definitions;
mod document_colors;
mod document_highlights;
mod document_symbols;
mod goto;
mod hover;
mod overlay;
mod references;
mod semantic_tokens;
mod signature_help;
mod snippet;
mod workspace_edit;

pub use code_actions::*;
pub use completions::*;
pub use definitions::*;
pub use document_colors::*;
pub use document_highlights::*;
pub use document_symbols::*;
pub use goto::*;
pub use hover::*;
pub use overlay::*;
pub use references::*;
pub use semantic_tokens::*;
pub use signature_help::*;
pub(crate) use snippet::*;

#[cfg(test)]
pub(crate) mod test_support {
    use crate::input::{EditorMode, EditorState, InputBaseState};
    use crate::theme::Theme;
    use gpui::{Entity, TestAppContext, VisualTestContext, div, prelude::*};

    struct TestRoot(Entity<InputBaseState<EditorMode>>);

    impl Render for TestRoot {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().size_full().child(self.0.clone())
        }
    }

    /// Open an editor state in a test window, for the LSP tests.
    pub(crate) fn build_editor(
        cx: &mut TestAppContext,
    ) -> (Entity<InputBaseState<EditorMode>>, VisualTestContext) {
        let mut editor = None;
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                cx.set_global(Theme::default());
                crate::input::init(cx);
                editor = Some(cx.new(|cx| EditorState::new(window, cx)));
                cx.new(|_| TestRoot(editor.clone().unwrap()))
            })
            .unwrap()
        });
        let cx = VisualTestContext::from_window(window.into(), cx);
        (editor.unwrap(), cx)
    }
}

/// Host hook to show a document when following an LSP location
/// (Go to Definition), modeled after the `window/showDocument` request.
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#window_showDocument
///
/// Called before the built-in behavior. Return `true` if the host has shown
/// the document (e.g. opened a docs window for a virtual/external URI);
/// return `false` to fall through to the default handling (`external` URIs
/// open in the browser, anything else jumps within the current document).
pub type ShowDocumentHandler =
    Rc<dyn Fn(&lsp_types::ShowDocumentParams, &mut Window, &mut App) -> bool>;

/// LSP ServerCapabilities
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#serverCapabilities
pub struct Lsp {
    /// The completion provider.
    pub completion_provider: Option<Rc<dyn CompletionProvider>>,
    /// The code action providers.
    pub code_action_providers: Vec<Rc<dyn CodeActionProvider>>,
    /// The hover provider.
    pub hover_provider: Option<Rc<dyn HoverProvider>>,
    /// The definition provider.
    pub definition_provider: Option<Rc<dyn DefinitionProvider>>,
    /// The signature help provider.
    pub signature_help_provider: Option<Rc<dyn SignatureHelpProvider>>,
    /// The document highlight provider.
    pub document_highlight_provider: Option<Rc<dyn DocumentHighlightProvider>>,
    /// The references provider.
    pub references_provider: Option<Rc<dyn ReferencesProvider>>,
    /// The document symbol provider.
    pub document_symbol_provider: Option<Rc<dyn DocumentSymbolProvider>>,
    /// The type definition provider.
    pub type_definition_provider: Option<Rc<dyn TypeDefinitionProvider>>,
    /// The implementation provider.
    pub implementation_provider: Option<Rc<dyn ImplementationProvider>>,
    /// The declaration provider.
    pub declaration_provider: Option<Rc<dyn DeclarationProvider>>,
    /// The document color provider.
    pub document_color_provider: Option<Rc<dyn DocumentColorProvider>>,
    /// The range semantic tokens provider.
    pub semantic_tokens_provider: Option<Rc<dyn DocumentRangeSemanticTokensProvider>>,
    /// Optional host hook to show documents for Go to Definition locations,
    /// following the `window/showDocument` request (see [`ShowDocumentHandler`]).
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#window_showDocument
    pub show_document: Option<ShowDocumentHandler>,

    /// Display options for the completion popover.
    pub completion_menu: CompletionMenuOptions,

    /// The URI identifying the document this editor hosts, see
    /// [`Self::document_uri`].
    pub(crate) document_uri: Option<lsp_types::Uri>,

    pub(crate) document_colors: Vec<(lsp_types::Range, Hsla)>,
    /// Cached semantic tokens as absolute position ranges + theme token-type
    /// names. Color is resolved from the name at paint time so theme switches
    /// take effect without a refetch.
    pub(crate) semantic_tokens: Vec<(lsp_types::Range, SharedString)>,
    /// Occurrences of the symbol under the cursor, as resolved byte
    /// ranges. Cleared on every edit.
    pub(crate) document_highlights: Vec<(std::ops::Range<usize>, lsp_types::DocumentHighlightKind)>,
    pub(crate) _hover_task: Task<Result<()>>,
    pub(crate) _document_color_task: Task<()>,
    pub(crate) _semantic_tokens_task: Task<()>,
    pub(crate) _signature_help_task: Task<()>,
    pub(crate) _document_highlight_task: Task<()>,
    pub(crate) _references_task: Task<()>,
    pub(crate) _document_symbols_task: Task<()>,
    pub(crate) _goto_task: Task<()>,
}

impl Default for Lsp {
    fn default() -> Self {
        Self {
            completion_provider: None,
            code_action_providers: vec![],
            hover_provider: None,
            definition_provider: None,
            signature_help_provider: None,
            document_highlight_provider: None,
            document_highlights: vec![],
            _document_highlight_task: Task::ready(()),
            references_provider: None,
            _references_task: Task::ready(()),
            document_symbol_provider: None,
            _document_symbols_task: Task::ready(()),
            type_definition_provider: None,
            implementation_provider: None,
            declaration_provider: None,
            _goto_task: Task::ready(()),
            document_color_provider: None,
            completion_menu: CompletionMenuOptions::default(),
            semantic_tokens_provider: None,
            show_document: None,
            document_uri: None,
            document_colors: vec![],
            semantic_tokens: vec![],
            _hover_task: Task::ready(Ok(())),
            _document_color_task: Task::ready(()),
            _semantic_tokens_task: Task::ready(()),
            _signature_help_task: Task::ready(()),
        }
    }
}

impl Lsp {
    /// Set the URI identifying the document this editor hosts.
    ///
    /// [`InputBaseState::apply_workspace_edit`] uses it to pick this
    /// document's edits out of a [`lsp_types::WorkspaceEdit`]. Without a URI
    /// every text edit is treated as targeting this editor.
    pub fn set_document_uri(&mut self, uri: lsp_types::Uri) {
        self.document_uri = Some(uri);
    }

    /// The URI identifying the document this editor hosts, if configured.
    pub fn document_uri(&self) -> Option<&lsp_types::Uri> {
        self.document_uri.as_ref()
    }

    /// Update the LSP when the text changes.
    ///
    /// `version` is the document version the given `text` belongs to; a
    /// response resolving against a newer document is discarded, the refetch
    /// scheduled by that newer edit supplies the fresh data.
    pub(crate) fn update(
        &mut self,
        text: &Rope,
        version: u64,
        window: &mut Window,
        cx: &mut Context<InputBaseState<EditorMode>>,
    ) {
        self.update_document_colors(text, version, window, cx);
        self.update_semantic_tokens(text, version, window, cx);
    }

    /// Reset all LSP states.
    pub(crate) fn reset(&mut self) {
        self.document_colors.clear();
        self.semantic_tokens.clear();
        self._hover_task = Task::ready(Ok(()));
        self._document_color_task = Task::ready(());
        self._semantic_tokens_task = Task::ready(());
        self._signature_help_task = Task::ready(());
        self.document_highlights.clear();
        self._document_highlight_task = Task::ready(());
        self._references_task = Task::ready(());
        self._document_symbols_task = Task::ready(());
        self._goto_task = Task::ready(());
    }
}

impl InputBaseState<EditorMode> {
    /// Apply a list of [`lsp_types::TextEdit`] to mutate the text.
    ///
    /// The batch is applied atomically through
    /// [`Self::apply_text_edits`]: all ranges are resolved against the
    /// document before any edit lands, so a multi-edit response anchors
    /// correctly regardless of its order.
    pub fn apply_lsp_edits(
        &mut self,
        text_edits: &Vec<lsp_types::TextEdit>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_text_edits(text_edits, window, cx);
    }
}
