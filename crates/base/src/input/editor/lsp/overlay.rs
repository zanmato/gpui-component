use super::*;
use crate::input::EditorMode;
use std::ops::Range;

use lsp_types::{CompletionItem, Hover};

#[derive(Clone, Debug, Default)]
pub struct CompletionMenuState {
    pub open: bool,
    pub trigger_start_offset: Option<usize>,
    pub query: String,
    pub items: Vec<CompletionItem>,
    revision: u64,
}

impl CompletionMenuState {
    /// Bumped whenever the content changes.
    ///
    /// A renderer that mirrors this menu compares revisions to decide whether
    /// to rebuild, so it never has to compare the item list itself.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

#[derive(Clone, Debug, Default)]
pub struct CodeActionMenuState {
    pub open: bool,
    pub items: Vec<CodeActionItem>,
    revision: u64,
}

impl CodeActionMenuState {
    /// Bumped whenever the content changes. See [`CompletionMenuState::revision`].
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

#[derive(Clone, Debug)]
pub struct HoverPopoverState {
    pub symbol_range: Range<usize>,
    pub hover: Hover,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ContextMenuContent {
    pub(crate) completion: CompletionMenuState,
    pub(crate) code_action: CodeActionMenuState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputOverlayKind {
    Completion,
    CodeAction,
}

impl InputBaseState<EditorMode> {
    pub fn present_completion_items(
        &mut self,
        trigger_start_offset: usize,
        query: impl Into<String>,
        items: Vec<CompletionItem>,
        cx: &mut Context<Self>,
    ) {
        self.extras
            .context_menu_content
            .completion
            .trigger_start_offset = Some(trigger_start_offset);
        self.extras.context_menu_content.completion.query = query.into();
        self.extras.context_menu_content.completion.items = items;
        self.extras.context_menu_content.completion.open =
            !self.extras.context_menu_content.completion.items.is_empty();
        self.extras.context_menu_content.completion.bump();
        cx.notify();
    }

    pub fn present_code_actions(&mut self, items: Vec<CodeActionItem>, cx: &mut Context<Self>) {
        self.extras.context_menu_content.code_action.items = items;
        self.extras.context_menu_content.code_action.open = !self
            .extras
            .context_menu_content
            .code_action
            .items
            .is_empty();
        self.extras.context_menu_content.code_action.bump();
        cx.notify();
    }

    pub fn present_hover(
        &mut self,
        symbol_range: Range<usize>,
        hover: Hover,
        cx: &mut Context<Self>,
    ) {
        self.extras.hover_popover = Some(HoverPopoverState {
            symbol_range,
            hover,
        });
        cx.notify();
    }

    pub fn present_diagnostic(
        &mut self,
        diagnostic: crate::input::DiagnosticEntry,
        cx: &mut Context<Self>,
    ) {
        self.diagnostic_popover = Some(Rc::new(diagnostic));
        cx.notify();
    }

    pub fn clear_diagnostic_popover(&mut self, cx: &mut Context<Self>) {
        if self.diagnostic_popover.take().is_some() {
            cx.notify();
        }
    }

    pub fn route_overlay_action(
        &mut self,
        action: Box<dyn gpui::Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.handle_action_for_context_menu(action, window, cx)
    }

    pub fn set_overlay_action_handler(
        &mut self,
        handler: impl Fn(
            InputOverlayKind,
            Box<dyn gpui::Action>,
            &mut Window,
            &mut Context<InputBaseState<EditorMode>>,
        ) -> bool
        + 'static,
    ) {
        self.overlay_action_handler = Some(Rc::new(handler));
    }

    pub fn has_overlay_action_handler(&self) -> bool {
        self.overlay_action_handler.is_some()
    }

    pub fn dismiss_completion_overlay(&mut self, cx: &mut Context<Self>) {
        if self.extras.context_menu_content.completion.open {
            self.extras.context_menu_content.completion.open = false;
            cx.notify();
        }
    }

    pub fn dismiss_code_action_overlay(&mut self, cx: &mut Context<Self>) {
        if self.extras.context_menu_content.code_action.open {
            self.extras.context_menu_content.code_action.open = false;
            cx.notify();
        }
    }

    pub fn insert_completion(
        &mut self,
        item: &CompletionItem,
        fallback_range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::input::RopeExt as _;

        let primary = match item.text_edit.as_ref() {
            Some(lsp_types::CompletionTextEdit::Edit(edit)) => edit.clone(),
            Some(lsp_types::CompletionTextEdit::InsertAndReplace(edit)) => {
                lsp_types::TextEdit::new(edit.replace, edit.new_text.clone())
            }
            None => {
                let (range, new_text) = match item.insert_text.as_ref() {
                    Some(insert_text) => {
                        (fallback_range.end..fallback_range.end, insert_text.clone())
                    }
                    None => (fallback_range, item.label.clone()),
                };
                lsp_types::TextEdit::new(
                    lsp_types::Range::new(
                        self.text.offset_to_position(range.start),
                        self.text.offset_to_position(range.end),
                    ),
                    new_text,
                )
            }
        };

        // The primary edit and the item's additional edits (auto-imports and
        // the like) land as one atomic batch: correctly anchored regardless
        // of order, and undone in a single step.
        let mut edits = vec![primary.clone()];
        edits.extend(item.additional_text_edits.clone().unwrap_or_default());

        self.completion_inserting = true;
        if !self.apply_text_edits(&edits, window, cx) {
            // A server sent additional edits overlapping the primary edit;
            // salvage the confirmation itself.
            self.apply_text_edits(&[primary], window, cx);
        }
        self.completion_inserting = false;
        self.focus(window, cx);
    }

    #[doc(hidden)]
    pub fn completion_menu_state(&self) -> &CompletionMenuState {
        &self.extras.context_menu_content.completion
    }

    #[doc(hidden)]
    pub fn code_action_menu_state(&self) -> &CodeActionMenuState {
        &self.extras.context_menu_content.code_action
    }

    pub fn hover_popover(&self) -> Option<&HoverPopoverState> {
        self.extras.hover_popover.as_ref()
    }

    pub fn dismiss_lsp_overlays(&mut self, cx: &mut Context<Self>) {
        self.hide_context_menu(cx);
        self.clear_hover_state(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::build_editor;
    use crate::input::Undo;
    use gpui::TestAppContext;
    use lsp_types::{CompletionItem, CompletionTextEdit, Position, TextEdit};

    fn edit(start: (u32, u32), end: (u32, u32), new_text: &str) -> TextEdit {
        TextEdit::new(
            lsp_types::Range::new(Position::new(start.0, start.1), Position::new(end.0, end.1)),
            new_text.to_string(),
        )
    }

    #[gpui::test]
    fn insert_completion_applies_additional_edits_atomically(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("Prin", window, cx);
                editor.selected_range = (4..4).into();
            });
        });

        // An auto-import style completion: the primary edit completes the
        // typed prefix, the additional edit inserts an import above it.
        let item = CompletionItem {
            label: "Println".into(),
            text_edit: Some(CompletionTextEdit::Edit(edit((0, 0), (0, 4), "Println"))),
            additional_text_edits: Some(vec![edit((0, 0), (0, 0), "import \"fmt\"\n")]),
            ..Default::default()
        };

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.insert_completion(&item, 0..4, window, cx);
                assert_eq!(editor.text().to_string(), "import \"fmt\"\nPrintln");
                // The cursor lands at the end of the primary insertion, not
                // at the additional edit.
                assert_eq!(editor.cursor(), "import \"fmt\"\nPrintln".len());
            });
        });

        // Both edits undo as one step.
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.undo(&Undo, window, cx);
                assert_eq!(editor.text().to_string(), "Prin");
            });
        });
    }
}
