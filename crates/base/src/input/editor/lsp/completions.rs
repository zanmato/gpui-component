use crate::input::EditorMode;
use anyhow::Result;
use gpui::{App, Context, EntityInputHandler, Pixels, Task, Window, px};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionResponse, InlineCompletionContext,
    InlineCompletionItem, InlineCompletionResponse, InlineCompletionTriggerKind,
    request::Completion,
};
use ropey::Rope;
use std::{cell::RefCell, ops::Range, rc::Rc, time::Duration};

use crate::input::InputBaseState;

/// Default debounce duration for inline completions.
const DEFAULT_INLINE_COMPLETION_DEBOUNCE: Duration = Duration::from_millis(300);

/// Display options for the LSP completion popover.
///
/// Accessed through [`super::Lsp::completion_menu`] so embedders can tweak the
/// popover without growing the [`InputBaseState`] API.
#[derive(Debug, Clone, Copy)]
pub struct CompletionMenuOptions {
    /// Maximum width of the popover.
    ///
    /// Defaults to 320 px, which is fine for most identifiers but can
    /// truncate longer labels. Widen this when hosting an editor that
    /// surfaces long completion labels.
    pub max_width: Pixels,
    /// The keys that accept the highlighted completion.
    ///
    /// Defaults to [`CompletionAcceptKeys::Enter`]. A key that does not accept
    /// keeps its editing meaning and closes the popover: Enter inserts a new
    /// line, Tab indents.
    pub accept_keys: CompletionAcceptKeys,
}

impl Default for CompletionMenuOptions {
    fn default() -> Self {
        Self {
            max_width: px(320.),
            accept_keys: CompletionAcceptKeys::default(),
        }
    }
}

/// The keys that accept the highlighted item of the completion popover.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum CompletionAcceptKeys {
    /// Enter accepts, Tab indents.
    #[default]
    Enter,
    /// Tab accepts, Enter inserts a new line.
    Tab,
    /// Both Enter and Tab accept.
    EnterAndTab,
}

impl CompletionAcceptKeys {
    /// Whether Enter accepts the highlighted completion.
    pub fn is_enter(self) -> bool {
        matches!(self, Self::Enter | Self::EnterAndTab)
    }

    /// Whether Tab accepts the highlighted completion.
    pub fn is_tab(self) -> bool {
        matches!(self, Self::Tab | Self::EnterAndTab)
    }
}

/// A trait for providing code completions based on the current input state and context.
pub trait CompletionProvider {
    /// Fetches completions based on the given byte offset.
    ///
    /// - The `offset` is in bytes of current cursor.
    ///
    /// textDocument/completion
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_completion
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>>;

    /// Fetches an inline completion suggestion for the given position.
    ///
    /// This is called after a debounce period when the user stops typing.
    /// The provider can analyze the text and cursor position to determine
    /// what inline completion suggestion to show.
    ///
    ///
    /// # Arguments
    /// * `rope` - The current text content
    /// * `offset` - The cursor position in bytes
    ///
    /// textDocument/inlineCompletion
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.18/specification/#textDocument_inlineCompletion
    fn inline_completion(
        &self,
        _rope: &Rope,
        _offset: usize,
        _trigger: InlineCompletionContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<Result<InlineCompletionResponse>> {
        Task::ready(Ok(InlineCompletionResponse::Array(vec![])))
    }

    /// Returns the debounce duration for inline completions.
    ///
    /// Default: 300ms
    #[inline]
    fn inline_completion_debounce(&self) -> Duration {
        DEFAULT_INLINE_COMPLETION_DEBOUNCE
    }

    fn resolve_completions(
        &self,
        _completion_indices: Vec<usize>,
        _completions: Rc<RefCell<Box<[Completion]>>>,
        _: &mut App,
    ) -> Task<Result<bool>> {
        Task::ready(Ok(false))
    }

    /// Determines if the completion should be triggered based on the given byte offset.
    ///
    /// This is called on the main thread.
    fn is_completion_trigger(&self, offset: usize, new_text: &str, cx: &mut App) -> bool;
}

pub(crate) struct InlineCompletion {
    /// Completion item to display as an inline completion suggestion
    pub(crate) item: Option<InlineCompletionItem>,
    /// Task for debouncing inline completion requests
    pub(crate) task: Task<Result<InlineCompletionResponse>>,
}

impl Default for InlineCompletion {
    fn default() -> Self {
        Self {
            item: None,
            task: Task::ready(Ok(InlineCompletionResponse::Array(vec![]))),
        }
    }
}

impl InputBaseState<EditorMode> {
    pub(crate) fn handle_completion_trigger(
        &mut self,
        _range: &Range<usize>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.completion_inserting {
            return;
        }

        let Some(provider) = self.extras.lsp.completion_provider.clone() else {
            return;
        };

        // Always schedule inline completion (debounced).
        // It will check if menu is open before showing the suggestion.
        self.schedule_inline_completion(window, cx);

        let new_offset = self.cursor();
        // Measure the inserted text in the current document. The replaced range
        // uses pre-edit coordinates, which preceding multi-cursor edits can
        // shift. The active caret ends immediately after the normalized input,
        // including selection replacements and IME commits.
        let Some(start) = new_offset.checked_sub(new_text.len()) else {
            return;
        };

        if !provider.is_completion_trigger(start, new_text, cx) {
            return;
        }

        // `trigger_start_offset` latches where the word a menu was opened for
        // begins, so later keystrokes refine the same query instead of starting
        // over at each character. It only describes this edit while the edit
        // continues that word: the document between the latch and the edit
        // must still read as a prefix of the last query. Deleting back into
        // the word keeps it; typing somewhere else, or into a document that
        // has since been replaced, starts a new query at this edit instead of
        // handing the provider text the user never typed as a prefix.
        let completion = &self.extras.context_menu_content.completion;
        let latched = completion.trigger_start_offset.filter(|&latched| {
            latched <= start
                && start <= latched + completion.query.len()
                && self.text.is_char_boundary(latched)
                && self.text.is_char_boundary(start)
                && completion
                    .query
                    .starts_with(self.text.slice(latched..start).to_string().as_str())
        });
        let start_offset = latched.unwrap_or(start);
        if new_offset < start_offset {
            return;
        }

        let query = self
            .text_for_range(
                self.range_to_utf16(&(start_offset..new_offset)),
                &mut None,
                window,
                cx,
            )
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        self.extras
            .context_menu_content
            .completion
            .trigger_start_offset = Some(start_offset);
        self.extras
            .context_menu_content
            .completion
            .query
            .clone_from(&query);

        let completion_context = CompletionContext {
            trigger_kind: lsp_types::CompletionTriggerKind::TRIGGER_CHARACTER,
            trigger_character: Some(query),
        };

        let provider_responses =
            provider.completions(&self.text, new_offset, completion_context, window, cx);
        let version = self.document_version;
        self.extras.context_menu_task = cx.spawn_in(window, async move |editor, cx| {
            let mut completions: Vec<CompletionItem> = vec![];
            if let Some(provider_responses) = provider_responses.await.ok() {
                match provider_responses {
                    CompletionResponse::Array(items) => completions.extend(items),
                    CompletionResponse::List(list) => completions.extend(list.items),
                }
            }

            if completions.is_empty() {
                editor.update(cx, |editor, cx| {
                    editor.extras.context_menu_content.completion.open = false;
                    editor.extras.context_menu_content.completion.items.clear();
                    editor.extras.context_menu_content.completion.bump();
                    cx.notify();
                })?;
                return Ok(());
            }

            editor
                .update_in(cx, |editor, window, cx| {
                    if !editor.focus_handle.is_focused(window) {
                        return;
                    }
                    if editor.document_version != version {
                        return;
                    }

                    editor.extras.context_menu_content.completion.items = completions;
                    editor.extras.context_menu_content.completion.open = !editor
                        .extras
                        .context_menu_content
                        .completion
                        .items
                        .is_empty();
                    editor.extras.context_menu_content.completion.bump();

                    cx.notify();
                })
                .ok();

            Ok(())
        });
    }

    pub(crate) fn hide_context_menu(&mut self, cx: &mut Context<Self>) {
        self.extras.context_menu_content.completion.open = false;
        self.extras.context_menu_content.code_action.open = false;
        self.extras.context_menu_task = Task::ready(Ok(()));
        cx.notify();
    }

    pub(crate) fn is_context_menu_open(&self, _cx: &gpui::App) -> bool {
        self.extras.context_menu_content.completion.open
            || self.extras.context_menu_content.code_action.open
    }

    pub(crate) fn handle_action_for_context_menu(
        &mut self,
        action: Box<dyn gpui::Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let is_enter = crate::input::Enter::is_primary(&*action);
        let is_tab = action.partial_eq(&crate::input::IndentInline);
        let closes_overlay = is_enter || is_tab || action.partial_eq(&crate::input::Escape);
        let kind = if self.extras.context_menu_content.completion.open {
            Some(super::InputOverlayKind::Completion)
        } else if self.extras.context_menu_content.code_action.open {
            Some(super::InputOverlayKind::CodeAction)
        } else {
            None
        };
        if kind == Some(super::InputOverlayKind::Completion) {
            // A key that does not accept keeps its editing meaning. Enter
            // still closes the list, so the new line does not leave a stale
            // suggestion under the caret.
            let accept_keys = self.extras.lsp.completion_menu.accept_keys;
            if is_enter && !accept_keys.is_enter() {
                self.hide_context_menu(cx);
                return false;
            }
            if is_tab && !accept_keys.is_tab() {
                return false;
            }
        } else if is_tab {
            return false;
        }
        let Some((kind, handler)) = kind.zip(self.overlay_action_handler.clone()) else {
            return false;
        };
        let handled = handler(kind, action, window, cx);
        if handled && closes_overlay {
            self.hide_context_menu(cx);
        }
        handled
    }

    /// Schedule an inline completion request after debouncing.
    pub(crate) fn schedule_inline_completion(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Clear any existing inline completion on text change
        self.clear_inline_completion(cx);

        let Some(provider) = self.extras.lsp.completion_provider.clone() else {
            return;
        };

        let offset = self.cursor();
        let text = self.text.clone();
        let debounce = provider.inline_completion_debounce();
        let background_executor = cx.background_executor().clone();
        let version = self.document_version;

        self.extras.inline_completion.task = cx.spawn_in(window, async move |editor, cx| {
            // Debounce: wait before fetching to avoid unnecessary requests while typing
            background_executor.timer(debounce).await;

            // Now fetch the inline completion after the debounce period
            let task = editor.update_in(cx, |editor, window, cx| {
                // Check if the document or cursor has changed during debounce
                if editor.cursor() != offset || editor.document_version != version {
                    return None;
                }

                // Don't fetch if completion menu is open
                if editor.is_context_menu_open(cx) {
                    return None;
                }

                let trigger = InlineCompletionContext {
                    trigger_kind: InlineCompletionTriggerKind::Automatic,
                    selected_completion_info: None,
                };

                Some(provider.inline_completion(&text, offset, trigger, window, cx))
            })?;

            let Some(task) = task else {
                return Ok(InlineCompletionResponse::Array(vec![]));
            };

            let response = task.await?;

            editor.update_in(cx, |editor, _window, cx| {
                // Only apply if the document and cursor are still unchanged
                if editor.cursor() != offset || editor.document_version != version {
                    return;
                }

                // Don't show if completion menu opened while we were fetching
                if editor.is_context_menu_open(cx) {
                    return;
                }

                if let Some(item) = match response.clone() {
                    InlineCompletionResponse::Array(items) => items.into_iter().next(),
                    InlineCompletionResponse::List(comp_list) => comp_list.items.into_iter().next(),
                } {
                    editor.extras.inline_completion.item = Some(item);
                    cx.notify();
                }
            })?;

            Ok(response)
        });
    }

    /// Check if an inline completion suggestion is currently displayed.
    #[inline]
    pub(crate) fn has_inline_completion(&self) -> bool {
        self.extras.inline_completion.item.is_some()
    }

    /// Clear the inline completion suggestion.
    pub(crate) fn clear_inline_completion(&mut self, cx: &mut Context<Self>) {
        self.extras.inline_completion = InlineCompletion::default();
        cx.notify();
    }

    /// Accept the inline completion, inserting it at the cursor position.
    /// Returns true if a completion was accepted, false if there was none.
    pub(crate) fn accept_inline_completion(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(completion_item) = self.extras.inline_completion.item.take() else {
            return false;
        };

        let cursor = self.cursor();
        let range_utf16 = self.range_to_utf16(&(cursor..cursor));
        let completion_text = completion_item.insert_text;
        self.replace_text_in_range_silent(Some(range_utf16), &completion_text, window, cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{AppContext as _, TestAppContext, px, size};

    use super::CompletionAcceptKeys;
    use crate::input::{EditorState, Enter, IndentInline, InputOverlayKind};

    const ENTER: Enter = Enter {
        secondary: false,
        shift: false,
    };

    /// Opens an editor holding "ab" with the completion menu open, and returns
    /// it with a log of the actions its overlay handler was given.
    fn open_menu(
        accept_keys: CompletionAcceptKeys,
        cx: &mut TestAppContext,
    ) -> (
        gpui::WindowHandle<gpui::EmptyView>,
        gpui::Entity<EditorState>,
        Rc<RefCell<Vec<&'static str>>>,
    ) {
        cx.update(crate::init);
        let mut editor = None;
        let window = cx.open_window(size(px(400.), px(100.)), |window, cx| {
            editor = Some(cx.new(|cx| EditorState::new(window, cx).default_value("ab")));
            gpui::EmptyView
        });
        let editor = editor.unwrap();
        let log = Rc::new(RefCell::new(Vec::new()));
        editor.update(cx, |state, _| {
            let log = log.clone();
            state.set_overlay_action_handler(move |kind, action, _, _| {
                assert_eq!(kind, InputOverlayKind::Completion);
                let name = if Enter::is_primary(&*action) {
                    "enter"
                } else if action.partial_eq(&IndentInline) {
                    "tab"
                } else {
                    return false;
                };
                log.borrow_mut().push(name);
                true
            });
            state.extras.lsp.completion_menu.accept_keys = accept_keys;
            state.extras.context_menu_content.completion.open = true;
            state.set_cursor_to(2);
        });
        (window, editor, log)
    }

    #[gpui::test]
    fn test_enter_accepts_by_default(cx: &mut TestAppContext) {
        let (window, editor, log) = open_menu(CompletionAcceptKeys::default(), cx);
        window
            .update(cx, |_, window, cx| {
                editor.update(cx, |state, cx| {
                    assert!(state.handle_action_for_context_menu(Box::new(ENTER), window, cx));
                    assert!(!state.is_context_menu_open(cx));

                    state.extras.context_menu_content.completion.open = true;
                    state.indent_inline(&IndentInline, window, cx);
                    assert_ne!(state.value(), "ab", "Tab indents");
                });
            })
            .unwrap();
        assert_eq!(log.borrow().as_slice(), &["enter"]);
    }

    #[gpui::test]
    fn test_tab_accept_leaves_enter_to_the_editor(cx: &mut TestAppContext) {
        let (window, editor, log) = open_menu(CompletionAcceptKeys::Tab, cx);
        window
            .update(cx, |_, window, cx| {
                editor.update(cx, |state, cx| {
                    // Enter is not forwarded, and it closes the list so the
                    // editor's own Enter inserts the new line.
                    assert!(!state.handle_action_for_context_menu(Box::new(ENTER), window, cx));
                    assert!(!state.is_context_menu_open(cx));

                    state.extras.context_menu_content.completion.open = true;
                    state.indent_inline(&IndentInline, window, cx);
                    assert_eq!(state.value(), "ab", "Tab accepts instead of indenting");
                    assert!(!state.is_context_menu_open(cx));
                });
            })
            .unwrap();
        assert_eq!(log.borrow().as_slice(), &["tab"]);
    }

    #[gpui::test]
    fn test_enter_and_tab_both_accept(cx: &mut TestAppContext) {
        let (window, editor, log) = open_menu(CompletionAcceptKeys::EnterAndTab, cx);
        window
            .update(cx, |_, window, cx| {
                editor.update(cx, |state, cx| {
                    assert!(state.handle_action_for_context_menu(Box::new(ENTER), window, cx));
                    state.extras.context_menu_content.completion.open = true;
                    state.indent_inline(&IndentInline, window, cx);
                    assert_eq!(state.value(), "ab");
                });
            })
            .unwrap();
        assert_eq!(log.borrow().as_slice(), &["enter", "tab"]);
    }
}
