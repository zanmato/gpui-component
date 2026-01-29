//! A text input field that allows the user to enter text.
//!
//! Based on the `Input` example from the `gpui` crate.
//! https://github.com/zed-industries/zed/blob/main/crates/gpui/examples/input.rs
use anyhow::Result;
use gpui::{
    Action, App, AppContext, Bounds, ClipboardItem, Context, Entity, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyBinding,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Pixels, Point, Render, ScrollHandle, ScrollWheelEvent, ShapedLine, SharedString, Styled as _,
    Subscription, Task, UTF16Selection, Window, actions, div, point, prelude::FluentBuilder as _,
    px,
};
use gpui::{Half, TextAlign};
use ropey::{Rope, RopeSlice};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ops::Range;
use std::rc::Rc;
use sum_tree::Bias;

use super::{
    DisplayMap, blink_cursor::BlinkCursor, change::Change, element::TextElement,
    mask_pattern::MaskPattern, mode::InputMode, number_input,
};
use crate::Size;
use crate::actions::{SelectDown, SelectLeft, SelectRight, SelectUp};
use crate::highlighter::DiagnosticSet;
use crate::input::blink_cursor::CURSOR_WIDTH;
use crate::input::change::OperationType;
use crate::input::movement::MoveDirection;
use crate::input::selection::TextSelector;
use crate::input::{
    CursorId, HoverDefinition, InlineCompletion, Lsp, Position, RopeExt as _, Selection,
    Selections,
    display_map::LineLayout,
    element::RIGHT_MARGIN,
    popovers::{ContextMenu, DiagnosticPopover, HoverPopover, MouseContextMenu},
    search::{self, SearchPanel},
};
use crate::{Root, history::History};

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = input, no_json)]
pub struct Enter {
    /// Is confirm with secondary.
    pub secondary: bool,
}

actions!(
    input,
    [
        Backspace,
        Delete,
        DeleteToBeginningOfLine,
        DeleteToEndOfLine,
        DeleteToPreviousWordStart,
        DeleteToNextWordEnd,
        Indent,
        Outdent,
        IndentInline,
        OutdentInline,
        MoveUp,
        MoveDown,
        MoveLeft,
        MoveRight,
        MoveHome,
        MoveEnd,
        MovePageUp,
        MovePageDown,
        SelectAll,
        SelectToStartOfLine,
        SelectToEndOfLine,
        SelectToStart,
        SelectToEnd,
        SelectToPreviousWordStart,
        SelectToNextWordEnd,
        ShowCharacterPalette,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        MoveToStartOfLine,
        MoveToEndOfLine,
        MoveToStart,
        MoveToEnd,
        MoveToPreviousWord,
        MoveToNextWord,
        Escape,
        ToggleCodeActions,
        Search,
        GoToDefinition,
        // Multi-cursor actions
        AddCursorAbove,
        AddCursorBelow,
        SplitSelectionToLines,
        RemoveAllCursors,
        SelectAllOccurrences,
    ]
);

#[derive(Clone)]
pub enum InputEvent {
    Change,
    PressEnter { secondary: bool },
    Focus,
    Blur,
}

pub(super) const CONTEXT: &str = "Input";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("shift-backspace", Backspace, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("delete", Delete, Some(CONTEXT)),
        KeyBinding::new("shift-delete", Delete, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-backspace", DeleteToBeginningOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-delete", DeleteToEndOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-backspace", DeleteToPreviousWordStart, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-backspace", DeleteToPreviousWordStart, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-delete", DeleteToNextWordEnd, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-delete", DeleteToNextWordEnd, Some(CONTEXT)),
        KeyBinding::new("enter", Enter { secondary: false }, Some(CONTEXT)),
        KeyBinding::new("shift-enter", Enter { secondary: false }, Some(CONTEXT)),
        KeyBinding::new("secondary-enter", Enter { secondary: true }, Some(CONTEXT)),
        KeyBinding::new("escape", Escape, Some(CONTEXT)),
        KeyBinding::new("up", MoveUp, Some(CONTEXT)),
        KeyBinding::new("down", MoveDown, Some(CONTEXT)),
        KeyBinding::new("left", MoveLeft, Some(CONTEXT)),
        KeyBinding::new("right", MoveRight, Some(CONTEXT)),
        KeyBinding::new("pageup", MovePageUp, Some(CONTEXT)),
        KeyBinding::new("pagedown", MovePageDown, Some(CONTEXT)),
        KeyBinding::new("tab", IndentInline, Some(CONTEXT)),
        KeyBinding::new("shift-tab", OutdentInline, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-]", Indent, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-]", Indent, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-[", Outdent, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-[", Outdent, Some(CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("shift-up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("shift-down", SelectDown, Some(CONTEXT)),
        KeyBinding::new("home", MoveHome, Some(CONTEXT)),
        KeyBinding::new("end", MoveEnd, Some(CONTEXT)),
        KeyBinding::new("shift-home", SelectToStartOfLine, Some(CONTEXT)),
        KeyBinding::new("shift-end", SelectToEndOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-shift-a", SelectToStartOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-shift-e", SelectToEndOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("shift-cmd-left", SelectToStartOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("shift-cmd-right", SelectToEndOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-shift-left", SelectToPreviousWordStart, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-left", SelectToPreviousWordStart, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-shift-right", SelectToNextWordEnd, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-right", SelectToNextWordEnd, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-a", SelectAll, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-a", SelectAll, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-c", Copy, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-c", Copy, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-x", Cut, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-x", Cut, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-v", Paste, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-v", Paste, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-a", MoveHome, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-left", MoveHome, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-e", MoveEnd, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-right", MoveEnd, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-z", Undo, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-shift-z", Redo, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-up", MoveToStart, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-down", MoveToEnd, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-left", MoveToPreviousWord, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-right", MoveToNextWord, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-left", MoveToPreviousWord, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-right", MoveToNextWord, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-shift-up", SelectToStart, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-shift-down", SelectToEnd, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-z", Undo, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-y", Redo, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-.", ToggleCodeActions, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-.", ToggleCodeActions, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-f", Search, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-f", Search, Some(CONTEXT)),
        // Multi-cursor keybindings
        KeyBinding::new("shift-alt-up", AddCursorAbove, Some(CONTEXT)),
        KeyBinding::new("shift-alt-down", AddCursorBelow, Some(CONTEXT)),
        KeyBinding::new("secondary-escape", RemoveAllCursors, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-l", SplitSelectionToLines, Some(CONTEXT)),
    ]);

    search::init(cx);
    number_input::init(cx);
}

/// Whitespace indicators for rendering spaces and tabs.
#[derive(Clone, Default)]
pub(crate) struct WhitespaceIndicators {
    /// Shaped line for space character indicator (•)
    pub(crate) space: ShapedLine,
    /// Shaped line for tab character indicator (→)
    pub(crate) tab: ShapedLine,
}

#[derive(Clone)]
pub(super) struct LastLayout {
    /// The visible range (no wrap) of lines in the viewport, the value is row (0-based) index.
    pub(super) visible_range: Range<usize>,
    /// The first visible line top position in scroll viewport.
    pub(super) visible_top: Pixels,
    /// The range of byte offset of the visible lines.
    pub(super) visible_range_offset: Range<usize>,
    /// The last layout lines (Only have visible lines).
    pub(super) lines: Rc<Vec<LineLayout>>,
    /// The line_height of text layout, this will change will InputElement painted.
    pub(super) line_height: Pixels,
    /// The wrap width of text layout, this will change will InputElement painted.
    pub(super) wrap_width: Option<Pixels>,
    /// The line number area width of text layout, if not line number, this will be 0px.
    pub(super) line_number_width: Pixels,
    /// The cursor position (top, left) in pixels.
    pub(super) cursor_bounds: Option<Bounds<Pixels>>,
    /// The text align of the text layout.
    pub(super) text_align: TextAlign,
    /// The content width of the text layout.
    pub(super) content_width: Pixels,
}

impl LastLayout {
    /// Get the line layout for the given row (0-based).
    ///
    /// 0 is the viewport first visible line.
    ///
    /// Returns None if the row is out of range.
    #[allow(dead_code)]
    pub(crate) fn line(&self, row: usize) -> Option<&LineLayout> {
        if row < self.visible_range.start || row >= self.visible_range.end {
            return None;
        }

        self.lines.get(row.saturating_sub(self.visible_range.start))
    }

    /// Get the alignment offset for the given line width.
    pub(super) fn alignment_offset(&self, line_width: Pixels) -> Pixels {
        match self.text_align {
            TextAlign::Left => px(0.),
            TextAlign::Center => (self.content_width - line_width).half().max(px(0.)),
            TextAlign::Right => (self.content_width - line_width).max(px(0.)),
        }
    }
}

/// InputState to keep editing state of the [`super::Input`].
pub struct InputState {
    pub(super) focus_handle: FocusHandle,
    pub(super) mode: InputMode,
    pub(super) text: Rope,
    pub(super) display_map: DisplayMap,
    pub(super) history: History<Change>,
    pub(super) blink_cursor: Entity<BlinkCursor>,
    pub(super) loading: bool,
    pub(super) selections: Selections,
    pub(super) search_panel: Option<Entity<SearchPanel>>,
    pub(super) searchable: bool,
    /// Range for save the selected word, use to keep word range when drag move.
    pub(super) selected_word_range: Option<Selection>,
    pub(super) selection_reversed: bool,
    /// The marked range is the temporary insert text on IME typing.
    pub(super) ime_marked_range: Option<Selection>,
    pub(super) last_layout: Option<LastLayout>,
    pub(super) last_cursor: Option<usize>,
    /// The input container bounds
    pub(super) input_bounds: Bounds<Pixels>,
    /// The text bounds
    pub(super) last_bounds: Option<Bounds<Pixels>>,
    pub(super) last_selected_range: Option<Selection>,
    pub(super) selecting: bool,
    /// Start offset for column selection
    pub(super) column_select_start: Option<usize>,
    pub(super) size: Size,
    pub(super) disabled: bool,
    pub(super) masked: bool,
    pub(super) clean_on_escape: bool,
    pub(super) soft_wrap: bool,
    pub(super) show_whitespaces: bool,
    pub(super) pattern: Option<regex::Regex>,
    pub(super) validate: Option<Box<dyn Fn(&str, &mut Context<Self>) -> bool + 'static>>,
    pub(crate) scroll_handle: ScrollHandle,
    /// The deferred scroll offset to apply on next layout.
    pub(crate) deferred_scroll_offset: Option<Point<Pixels>>,
    /// The size of the scrollable content.
    pub(crate) scroll_size: gpui::Size<Pixels>,
    pub(super) text_align: TextAlign,

    /// The mask pattern for formatting the input text
    pub(crate) mask_pattern: MaskPattern,
    pub(super) placeholder: SharedString,

    /// Popover
    diagnostic_popover: Option<Entity<DiagnosticPopover>>,
    /// Completion/CodeAction context menu
    pub(super) context_menu: Option<ContextMenu>,
    pub(super) mouse_context_menu: Entity<MouseContextMenu>,
    /// A flag to indicate if we are currently inserting a completion item.
    pub(super) completion_inserting: bool,
    pub(super) hover_popover: Option<Entity<HoverPopover>>,
    /// The LSP definitions locations for "Go to Definition" feature.
    pub(super) hover_definition: HoverDefinition,

    pub lsp: Lsp,

    /// A flag to indicate if we have a pending update to the text.
    ///
    /// If true, will call some update (for example LSP, Syntax Highlight) before render.
    _pending_update: bool,
    /// A flag to indicate if we should ignore the next completion event.
    pub(super) silent_replace_text: bool,
    /// A flag to indicate if we should skip setting cursor position (used during multi-cursor undo/redo)
    skip_set_cursor: bool,

    /// To remember the horizontal column (x-coordinate) of the cursor position for keep column for move up/down.
    ///
    /// The first element is the x-coordinate (Pixels), preferred to use this.
    /// The second element is the column (usize), fallback to use this.
    pub(super) preferred_column: Option<(Pixels, usize)>,
    /// The last operation type (INSERT or DELETE) to detect changes for grouping.
    /// This helps separate INSERT and DELETE operations into different undo groups.
    pub(super) last_operation_type: Option<OperationType>,
    /// Counter for generating unique operation_ids for changes within a version group.
    pub(super) operation_id_counter: usize,
    _subscriptions: Vec<Subscription>,

    pub(super) _context_menu_task: Task<Result<()>>,
    pub(super) inline_completion: InlineCompletion,

    pub(super) selections_before_edit: Option<Vec<Selection>>,
}

impl EventEmitter<InputEvent> for InputState {}

impl InputState {
    /// Create a Input state with default [`InputMode::SingleLine`] mode.
    ///
    /// See also: [`Self::multi_line`], [`Self::auto_grow`] to set other mode.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle().tab_stop(true);
        let blink_cursor = cx.new(|_| BlinkCursor::new());
        let history = History::new().group_interval(std::time::Duration::from_secs(1));

        let _subscriptions = vec![
            // Observe the blink cursor to repaint the view when it changes.
            cx.observe(&blink_cursor, |_, _, cx| cx.notify()),
            // Blink the cursor when the window is active, pause when it's not.
            cx.observe_window_activation(window, |input, window, cx| {
                if window.is_window_active() {
                    let focus_handle = input.focus_handle.clone();
                    if focus_handle.is_focused(window) {
                        input.blink_cursor.update(cx, |blink_cursor, cx| {
                            blink_cursor.start(cx);
                        });
                    }
                }
            }),
            cx.on_focus(&focus_handle, window, Self::on_focus),
            cx.on_blur(&focus_handle, window, Self::on_blur),
        ];

        let text_style = window.text_style();
        let mouse_context_menu = MouseContextMenu::new(cx.entity(), window, cx);

        Self {
            focus_handle: focus_handle.clone(),
            text: "".into(),
            display_map: DisplayMap::new(text_style.font(), window.rem_size(), None),
            blink_cursor,
            history,
            selections: Selections::default(),
            search_panel: None,
            searchable: false,
            selected_word_range: None,
            selection_reversed: false,
            ime_marked_range: None,
            input_bounds: Bounds::default(),
            selecting: false,
            column_select_start: None,
            disabled: false,
            masked: false,
            clean_on_escape: false,
            soft_wrap: true,
            show_whitespaces: false,
            loading: false,
            pattern: None,
            validate: None,
            mode: InputMode::default(),
            last_layout: None,
            last_bounds: None,
            last_selected_range: None,
            last_cursor: None,
            scroll_handle: ScrollHandle::new(),
            scroll_size: gpui::size(px(0.), px(0.)),
            deferred_scroll_offset: None,
            preferred_column: None,
            placeholder: SharedString::default(),
            mask_pattern: MaskPattern::default(),
            text_align: TextAlign::Left,
            lsp: Lsp::default(),
            diagnostic_popover: None,
            context_menu: None,
            mouse_context_menu,
            completion_inserting: false,
            hover_popover: None,
            hover_definition: HoverDefinition::default(),
            silent_replace_text: false,
            skip_set_cursor: false,
            size: Size::default(),
            last_operation_type: None,
            operation_id_counter: 0,
            _subscriptions,
            _context_menu_task: Task::ready(Ok(())),
            _pending_update: false,
            inline_completion: InlineCompletion::default(),
            selections_before_edit: None,
        }
    }

    /// Set Input to use multi line mode.
    ///
    /// Default rows is 2.
    pub fn multi_line(mut self, multi_line: bool) -> Self {
        self.mode = self.mode.multi_line(multi_line);
        self
    }

    /// Set Input to use [`InputMode::AutoGrow`] mode with min, max rows limit.
    pub fn auto_grow(mut self, min_rows: usize, max_rows: usize) -> Self {
        self.mode = InputMode::auto_grow(min_rows, max_rows);
        self
    }

    /// Set Input to use [`InputMode::CodeEditor`] mode.
    ///
    /// Default options:
    ///
    /// - line_number: true
    /// - tab_size: 2
    /// - hard_tabs: false
    /// - height: 100%
    /// - multi_line: true
    /// - indent_guides: true
    ///
    /// If `highlighter` is None, will use the default highlighter.
    ///
    /// Code Editor aim for help used to simple code editing or display, not a full-featured code editor.
    ///
    /// ## Features
    ///
    /// - Syntax Highlighting
    /// - Auto Indent
    /// - Line Number
    /// - Large Text support, up to 50K lines.
    pub fn code_editor(mut self, language: impl Into<SharedString>) -> Self {
        let language: SharedString = language.into();
        self.mode = InputMode::code_editor(language);
        self.searchable = true;
        self
    }

    /// Set this input is searchable, default is false (Default true for Code Editor).
    pub fn searchable(mut self, searchable: bool) -> Self {
        debug_assert!(self.mode.is_multi_line());
        self.searchable = searchable;
        self
    }

    /// Set placeholder
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Set enable/disable code folding, only for [`InputMode::CodeEditor`] mode.
    ///
    /// Default: true
    pub fn folding(mut self, folding: bool) -> Self {
        debug_assert!(self.mode.is_code_editor());
        if let InputMode::CodeEditor { folding: f, .. } = &mut self.mode {
            *f = folding;
        }
        self
    }

    /// Set code folding at runtime, only for [`InputMode::CodeEditor`] mode.
    ///
    /// When disabling, all existing folds are cleared.
    pub fn set_folding(&mut self, folding: bool, _: &mut Window, cx: &mut Context<Self>) {
        debug_assert!(self.mode.is_code_editor());
        if let InputMode::CodeEditor { folding: f, .. } = &mut self.mode {
            *f = folding;
        }
        if !folding {
            self.display_map.clear_folds();
        }
        cx.notify();
    }

    /// Set enable/disable line number, only for [`InputMode::CodeEditor`] mode.
    pub fn line_number(mut self, line_number: bool) -> Self {
        debug_assert!(self.mode.is_code_editor() && self.mode.is_multi_line());
        if let InputMode::CodeEditor { line_number: l, .. } = &mut self.mode {
            *l = line_number;
        }
        self
    }

    /// Set line number, only for [`InputMode::CodeEditor`] mode.
    pub fn set_line_number(&mut self, line_number: bool, _: &mut Window, cx: &mut Context<Self>) {
        debug_assert!(self.mode.is_code_editor() && self.mode.is_multi_line());
        if let InputMode::CodeEditor { line_number: l, .. } = &mut self.mode {
            *l = line_number;
        }
        cx.notify();
    }

    /// Set the number of rows for the multi-line Textarea.
    ///
    /// This is only used when `multi_line` is set to true.
    ///
    /// default: 2
    pub fn rows(mut self, rows: usize) -> Self {
        match &mut self.mode {
            InputMode::PlainText { rows: r, .. } | InputMode::CodeEditor { rows: r, .. } => {
                *r = rows
            }
            InputMode::AutoGrow {
                max_rows: max_r,
                rows: r,
                ..
            } => {
                *r = rows;
                *max_r = rows;
            }
        }
        self
    }

    /// Set highlighter language for for [`InputMode::CodeEditor`] mode.
    pub fn set_highlighter(
        &mut self,
        new_language: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        match &mut self.mode {
            InputMode::CodeEditor {
                language,
                highlighter,
                ..
            } => {
                *language = new_language.into();
                *highlighter.borrow_mut() = None;
            }
            _ => {}
        }
        cx.notify();
    }

    fn reset_highlighter(&mut self, cx: &mut Context<Self>) {
        match &mut self.mode {
            InputMode::CodeEditor { highlighter, .. } => {
                *highlighter.borrow_mut() = None;
            }
            _ => {}
        }
        cx.notify();
    }

    #[inline]
    pub fn diagnostics(&self) -> Option<&DiagnosticSet> {
        self.mode.diagnostics()
    }

    #[inline]
    pub fn diagnostics_mut(&mut self) -> Option<&mut DiagnosticSet> {
        self.mode.diagnostics_mut()
    }

    /// Set placeholder
    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    /// Find which line and sub-line the given offset belongs to, along with the position within that sub-line.
    ///
    /// Returns:
    ///
    /// - The index of the line (zero-based) containing the offset.
    /// - The index of the sub-line (zero-based) within the line containing the offset.
    /// - The position of the offset.
    pub(super) fn line_and_position_for_offset(
        &self,
        offset: usize,
    ) -> (usize, usize, Option<Point<Pixels>>) {
        let Some(last_layout) = &self.last_layout else {
            return (0, 0, None);
        };
        let line_height = last_layout.line_height;

        let mut prev_lines_offset = last_layout.visible_range_offset.start;
        let mut y_offset = last_layout.visible_top;
        for (line_index, line) in last_layout.lines.iter().enumerate() {
            let local_offset = offset.saturating_sub(prev_lines_offset);
            if let Some(pos) = line.position_for_index(local_offset, last_layout) {
                let sub_line_index = (pos.y / line_height) as usize;
                let adjusted_pos = point(pos.x + last_layout.line_number_width, pos.y + y_offset);
                return (line_index, sub_line_index, Some(adjusted_pos));
            }

            y_offset += line.size(line_height).height;
            prev_lines_offset += line.len() + 1;
        }
        (0, 0, None)
    }

    /// Set the text of the input field.
    ///
    /// And the selection_range will be reset to 0..0.
    pub fn set_value(
        &mut self,
        value: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.history.ignore = true;
        self.replace_text(value, window, cx);
        self.history.ignore = false;

        // Ensure cursor to start when set text
        if self.mode.is_single_line() {
            self.set_selection(self.text.len(), self.text.len());
        } else {
            self.set_cursor_to(0);
        }

        if self.mode.is_code_editor() {
            self._pending_update = true;
            self.lsp.reset();
        }

        // Move scroll to top
        self.scroll_handle.set_offset(point(px(0.), px(0.)));

        cx.notify();
    }

    /// Insert text at the current cursor position.
    ///
    /// And the cursor will be moved to the end of inserted text.
    pub fn insert(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.disabled = false;
        let text: SharedString = text.into();
        let range_utf16 = self.range_to_utf16(&(self.cursor()..self.cursor()));
        self.replace_text_in_range_silent(Some(range_utf16), &text, window, cx);
        let new_end = self.active_selection().end;
        self.set_selection(new_end, new_end);
    }

    /// Replace text at the current cursor position.
    ///
    /// And the cursor will be moved to the end of replaced text.
    pub fn replace(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_disabled = self.disabled;
        self.disabled = false;
        let text: SharedString = text.into();
        self.replace_text_in_range_silent(None, &text, window, cx);
        let new_end = self.active_selection().end;
        self.set_selection(new_end, new_end);
        self.disabled = was_disabled;
    }

    fn replace_text(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_disabled = self.disabled;
        self.disabled = false;
        let text: SharedString = text.into();
        let range = 0..self.text.chars().map(|c| c.len_utf16()).sum();
        self.replace_text_in_range_silent(Some(range), &text, window, cx);
        self.reset_highlighter(cx);
        self.disabled = was_disabled;
    }

    /// Set with disabled mode.
    ///
    /// See also: [`Self::set_disabled`], [`Self::is_disabled`].
    #[allow(unused)]
    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Set with password masked state.
    ///
    /// Only for [`InputMode::SingleLine`] mode.
    pub fn masked(mut self, masked: bool) -> Self {
        debug_assert!(self.mode.is_single_line());
        self.masked = masked;
        self
    }

    /// Set the password masked state of the input field.
    ///
    /// Only for [`InputMode::SingleLine`] mode.
    pub fn set_masked(&mut self, masked: bool, _: &mut Window, cx: &mut Context<Self>) {
        debug_assert!(self.mode.is_single_line());
        self.masked = masked;
        cx.notify();
    }

    /// Set true to clear the input by pressing Escape key.
    pub fn clean_on_escape(mut self) -> Self {
        self.clean_on_escape = true;
        self
    }

    /// Set the soft wrap mode for multi-line input, default is true.
    pub fn soft_wrap(mut self, wrap: bool) -> Self {
        debug_assert!(self.mode.is_multi_line());
        self.soft_wrap = wrap;
        self
    }

    /// Set whether to show whitespace characters.
    pub fn show_whitespaces(mut self, show: bool) -> Self {
        self.show_whitespaces = show;
        self
    }

    /// Update the soft wrap mode for multi-line input, default is true.
    pub fn set_soft_wrap(&mut self, wrap: bool, _: &mut Window, cx: &mut Context<Self>) {
        debug_assert!(self.mode.is_multi_line());
        self.soft_wrap = wrap;
        if wrap {
            let wrap_width = self
                .last_layout
                .as_ref()
                .and_then(|b| b.wrap_width)
                .unwrap_or(self.input_bounds.size.width);

            self.display_map.on_layout_changed(Some(wrap_width), cx);

            // Reset scroll to left 0
            let mut offset = self.scroll_handle.offset();
            offset.x = px(0.);
            self.scroll_handle.set_offset(offset);
        } else {
            self.display_map.on_layout_changed(None, cx);
        }
        cx.notify();
    }

    /// Update whether to show whitespace characters.
    pub fn set_show_whitespaces(&mut self, show: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.show_whitespaces = show;
        cx.notify();
    }

    /// Set the regular expression pattern of the input field.
    ///
    /// Only for [`InputMode::SingleLine`] mode.
    pub fn pattern(mut self, pattern: regex::Regex) -> Self {
        debug_assert!(self.mode.is_single_line());
        self.pattern = Some(pattern);
        self
    }

    /// Set the regular expression pattern of the input field with reference.
    ///
    /// Only for [`InputMode::SingleLine`] mode.
    pub fn set_pattern(
        &mut self,
        pattern: regex::Regex,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        debug_assert!(self.mode.is_single_line());
        self.pattern = Some(pattern);
    }

    /// Set the validation function of the input field.
    ///
    /// Only for [`InputMode::SingleLine`] mode.
    pub fn validate(mut self, f: impl Fn(&str, &mut Context<Self>) -> bool + 'static) -> Self {
        debug_assert!(self.mode.is_single_line());
        self.validate = Some(Box::new(f));
        self
    }

    /// Set true to show spinner at the input right.
    ///
    /// Only for [`InputMode::SingleLine`] mode.
    pub fn set_loading(&mut self, loading: bool, _: &mut Window, cx: &mut Context<Self>) {
        debug_assert!(self.mode.is_single_line());
        self.loading = loading;
        cx.notify();
    }

    /// Set the default value of the input field.
    pub fn default_value(mut self, value: impl Into<SharedString>) -> Self {
        let text: SharedString = value.into();
        self.text = Rope::from(text.as_str());
        if let Some(diagnostics) = self.mode.diagnostics_mut() {
            diagnostics.reset(&self.text)
        }
        // Note: We can't call display_map.set_text here because it needs cx.
        // The text will be set during prepare_if_need in element.rs
        self._pending_update = true;
        self
    }

    /// Return the value of the input field.
    pub fn value(&self) -> SharedString {
        SharedString::new(self.text.to_string())
    }

    /// Return the portion of the value within the input field that
    /// is selected by the user
    pub fn selected_value(&self) -> SharedString {
        SharedString::new(self.selected_text().to_string())
    }

    /// Return the value without mask.
    pub fn unmask_value(&self) -> SharedString {
        self.mask_pattern.unmask(&self.text.to_string()).into()
    }

    /// Return the text [`Rope`] of the input field.
    pub fn text(&self) -> &Rope {
        &self.text
    }

    /// Return the (0-based) [`Position`] of the cursor.
    pub fn cursor_position(&self) -> Position {
        let offset = self.cursor();
        self.text.offset_to_position(offset)
    }

    /// Set (0-based) [`Position`] of the cursor.
    ///
    /// This will move the cursor to the specified line and column, and update the selection range.
    pub fn set_cursor_position(
        &mut self,
        position: impl Into<Position>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let position: Position = position.into();
        let offset = self.text.position_to_offset(&position);

        self.move_to_offset(offset, None, cx);
        self.update_preferred_column();
        self.focus(window, cx);
    }

    /// Focus the input field.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        self.blink_cursor.update(cx, |cursor, cx| {
            cursor.start(cx);
        });
    }

    /// Extend a selection to a new offset, updating the appropriate end based on
    /// the reversed flag and normalizing if needed.
    fn extend_selection_to(selection: &mut Selection, new_offset: usize) {
        if selection.reversed {
            selection.start = new_offset;
        } else {
            selection.end = new_offset;
        }

        // Normalize so start <= end
        if selection.end < selection.start {
            selection.reversed = !selection.reversed;
            std::mem::swap(&mut selection.start, &mut selection.end);
        }
    }

    pub(super) fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.clear_inline_completion(cx);

        // Extend each selection left by one character boundary
        let new_selections: Vec<Selection> = self
            .selections
            .iter()
            .map(|sel| {
                let mut new_sel = sel.clone();
                let new_offset = self.previous_boundary(sel.cursor_offset());
                Self::extend_selection_to(&mut new_sel, new_offset);
                new_sel
            })
            .collect();

        self.selections.replace_all(new_selections);
        self.selections.merge_overlapping();
        cx.notify();
    }

    pub(super) fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.clear_inline_completion(cx);

        // Extend each selection right by one character boundary
        let new_selections: Vec<Selection> = self
            .selections
            .iter()
            .map(|sel| {
                let mut new_sel = sel.clone();
                let new_offset = self.next_boundary(sel.cursor_offset());
                Self::extend_selection_to(&mut new_sel, new_offset);
                new_sel
            })
            .collect();

        self.selections.replace_all(new_selections);
        self.selections.merge_overlapping();
        cx.notify();
    }

    pub(super) fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.select_vertical(-1, cx);
    }

    pub(super) fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.select_vertical(1, cx);
    }

    fn select_vertical(&mut self, row_direction: isize, cx: &mut Context<Self>) {
        if self.mode.is_single_line() {
            return;
        }
        self.clear_inline_completion(cx);

        let text = &self.text;

        let new_selections: Vec<Selection> = self
            .selections
            .iter()
            .map(|sel| {
                let cursor_offset = sel.cursor_offset();
                let cursor_point = text.offset_to_point(cursor_offset);

                // Use the column from column_anchor if available, otherwise use current column
                let preferred_column = sel.column_anchor.unwrap_or(cursor_point.column);

                // Calculate target row
                let target_row = if row_direction < 0 {
                    cursor_point
                        .row
                        .saturating_sub(row_direction.unsigned_abs())
                } else {
                    (cursor_point.row + row_direction as usize)
                        .min(text.lines_len().saturating_sub(1))
                };

                // Get the target column, clamping to line length if needed
                let line = text.slice_line(target_row);
                let target_column = preferred_column.min(line.len());

                let line_start = text.line_start_offset(target_row);
                let new_offset = line_start + target_column;

                // Extend selection to the new offset
                let mut new_sel = sel.clone();
                Self::extend_selection_to(&mut new_sel, new_offset);
                new_sel
            })
            .collect();

        self.selections.replace_all(new_selections);
        self.selections.merge_overlapping();
        cx.notify();
    }

    pub(super) fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        // Remove all but the active cursor when selecting all
        self.selections.remove_all_but_active();
        self.set_selection(0, self.text.len());
        cx.notify();
    }

    pub(super) fn select_to_start(
        &mut self,
        _: &SelectToStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Remove all but the active cursor when selecting to start
        self.selections.remove_all_but_active();
        let active = self.active_selection_mut();
        // Update the end (cursor position) to 0, then normalize
        active.end = 0;
        // Normalize so start <= end
        if active.end < active.start {
            active.reversed = !active.reversed;
            std::mem::swap(&mut active.start, &mut active.end);
        }
        self.pause_blink_cursor(cx);
        cx.notify();
    }

    pub(super) fn select_to_end(
        &mut self,
        _: &SelectToEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Remove all but the active cursor when selecting to end
        self.selections.remove_all_but_active();
        let doc_end = self.text.len();
        let active = self.active_selection_mut();
        // Selecting to doc_end (after cursor), update end, keep start as cursor anchor
        active.end = doc_end;
        // Normalize so start <= end
        if active.end < active.start {
            active.reversed = !active.reversed;
            std::mem::swap(&mut active.start, &mut active.end);
        }
        self.pause_blink_cursor(cx);
        cx.notify();
    }

    pub(super) fn select_to_start_of_line(
        &mut self,
        _: &SelectToStartOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_selections: Vec<Selection> = self
            .selections
            .iter()
            .map(|sel| {
                let cursor_offset = sel.cursor_offset();
                let line_start = self.start_of_line_at(cursor_offset);
                let mut new_sel = sel.clone();
                // Update the end (cursor position) to line start, then normalize
                new_sel.end = line_start;
                // Normalize so start <= end
                if new_sel.end < new_sel.start {
                    new_sel.reversed = !new_sel.reversed;
                    std::mem::swap(&mut new_sel.start, &mut new_sel.end);
                }
                new_sel
            })
            .collect();

        self.selections.replace_all(new_selections);
        self.pause_blink_cursor(cx);
        cx.notify();
    }

    pub(super) fn select_to_end_of_line(
        &mut self,
        _: &SelectToEndOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.text.clone();
        let new_selections: Vec<Selection> = self
            .selections
            .iter()
            .map(|sel| {
                let cursor_offset = sel.cursor_offset();
                let mut line_end = self.end_of_line_at(cursor_offset);
                // line_end_offset returns the position after the line content.
                // For a line ending with \n, it returns the position after \n.
                // We need to move back to the last character before the \n.
                if line_end > 0 {
                    if let Some(ch) = text.char_at(line_end) {
                        if ch == '\n' || ch == '\r' {
                            line_end = line_end.saturating_sub(1);
                        }
                    }
                }
                let mut new_sel = sel.clone();
                // Update the end (cursor position) to line end, then normalize
                new_sel.end = line_end;
                // Normalize so start <= end
                if new_sel.end < new_sel.start {
                    new_sel.reversed = !new_sel.reversed;
                    std::mem::swap(&mut new_sel.start, &mut new_sel.end);
                }
                new_sel
            })
            .collect();

        self.selections.replace_all(new_selections);
        self.pause_blink_cursor(cx);
        cx.notify();
    }

    pub(super) fn select_to_previous_word(
        &mut self,
        _: &SelectToPreviousWordStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.text.clone();
        let new_selections: Vec<Selection> = self
            .selections
            .iter()
            .map(|sel| {
                let new_offset = TextSelector::previous_word_start_at(&text, sel.start);
                let mut new_sel = sel.clone();
                if new_sel.reversed {
                    new_sel.start = new_offset;
                } else {
                    new_sel.end = new_offset;
                }
                new_sel
            })
            .collect();

        self.selections.replace_all(new_selections);
        self.pause_blink_cursor(cx);
        cx.notify();
    }

    pub(super) fn select_to_next_word(
        &mut self,
        _: &SelectToNextWordEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.text.clone();
        let new_selections: Vec<Selection> = self
            .selections
            .iter()
            .map(|sel| {
                let new_offset = TextSelector::next_word_end_at(&text, sel.end);
                let mut new_sel = sel.clone();
                if new_sel.reversed {
                    new_sel.start = new_offset;
                } else {
                    new_sel.end = new_offset;
                }
                new_sel
            })
            .collect();

        self.selections.replace_all(new_selections);
        self.pause_blink_cursor(cx);
        cx.notify();
    }

    /// Get start of line byte offset of cursor
    pub(super) fn start_of_line(&self) -> usize {
        if self.mode.is_single_line() {
            return 0;
        }

        let row = self.text.offset_to_point(self.cursor()).row;
        self.text.line_start_offset(row)
    }

    /// Get start of line byte offset for the line containing the given offset.
    pub(super) fn start_of_line_at(&self, offset: usize) -> usize {
        if self.mode.is_single_line() {
            return 0;
        }

        let row = self.text.offset_to_point(offset).row;
        self.text.line_start_offset(row)
    }

    /// Get end of line byte offset of cursor
    pub(super) fn end_of_line(&self) -> usize {
        if self.mode.is_single_line() {
            return self.text.len();
        }

        let row = self.text.offset_to_point(self.cursor()).row;
        self.text.line_end_offset(row)
    }

    /// Get end of line byte offset for the line containing the given offset.
    pub(super) fn end_of_line_at(&self, offset: usize) -> usize {
        if self.mode.is_single_line() {
            return self.text.len();
        }

        let row = self.text.offset_to_point(offset).row;
        self.text.line_end_offset(row)
    }

    /// Get indent string of next line.
    ///
    /// To get current and next line indent, to return more depth one.
    pub(super) fn indent_of_next_line(&mut self) -> String {
        if self.mode.is_single_line() {
            return "".into();
        }

        let mut current_indent = String::new();
        let mut next_indent = String::new();
        let current_line_start_pos = self.start_of_line();
        let next_line_start_pos = self.end_of_line();
        for c in self.text.slice(current_line_start_pos..).chars() {
            if !c.is_whitespace() {
                break;
            }
            if c == '\n' || c == '\r' {
                break;
            }
            current_indent.push(c);
        }

        for c in self.text.slice(next_line_start_pos..).chars() {
            if !c.is_whitespace() {
                break;
            }
            if c == '\n' || c == '\r' {
                break;
            }
            next_indent.push(c);
        }

        if next_indent.len() > current_indent.len() {
            return next_indent;
        } else {
            return current_indent;
        }
    }

    pub(super) fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        self.selections_before_edit = Some(self.selections.iter().cloned().collect());
        let mut any_changed = false;
        let mut new_selections = Vec::new();
        for selection in self.selections.iter() {
            if selection.is_collapsed() {
                let prev = self.previous_boundary(selection.start);
                if prev != selection.start {
                    new_selections.push(Selection::new(selection.id, prev, selection.start));
                    any_changed = true;
                } else {
                    new_selections.push(selection.clone());
                }
            } else {
                new_selections.push(selection.clone());
            }
        }
        if any_changed {
            self.selections.replace_all(new_selections);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    pub(super) fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        self.selections_before_edit = Some(self.selections.iter().cloned().collect());
        let mut any_changed = false;
        let mut new_selections = Vec::new();
        for selection in self.selections.iter() {
            if selection.is_collapsed() {
                let next = self.next_boundary(selection.start);
                if next != selection.start {
                    new_selections.push(Selection::new(selection.id, selection.start, next));
                    any_changed = true;
                } else {
                    new_selections.push(selection.clone());
                }
            } else {
                new_selections.push(selection.clone());
            }
        }
        if any_changed {
            self.selections.replace_all(new_selections);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    pub(super) fn delete_to_beginning_of_line(
        &mut self,
        _: &DeleteToBeginningOfLine,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.active_selection().is_collapsed() {
            self.replace_text_in_range(None, "", window, cx);
            self.pause_blink_cursor(cx);
            return;
        }

        let mut offset = self.start_of_line();
        if offset == self.cursor() {
            offset = offset.saturating_sub(1);
        }
        self.replace_text_in_range_silent(
            Some(self.range_to_utf16(&(offset..self.cursor()))),
            "",
            window,
            cx,
        );
        self.pause_blink_cursor(cx);
    }

    pub(super) fn delete_to_end_of_line(
        &mut self,
        _: &DeleteToEndOfLine,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.active_selection().is_collapsed() {
            self.replace_text_in_range(None, "", window, cx);
            self.pause_blink_cursor(cx);
            return;
        }

        let mut offset = self.end_of_line();
        if offset == self.cursor() {
            offset = (offset + 1).clamp(0, self.text.len());
        }
        self.replace_text_in_range_silent(
            Some(self.range_to_utf16(&(self.cursor()..offset))),
            "",
            window,
            cx,
        );
        self.pause_blink_cursor(cx);
    }

    pub(super) fn delete_previous_word(
        &mut self,
        _: &DeleteToPreviousWordStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.active_selection().is_collapsed() {
            self.replace_text_in_range(None, "", window, cx);
            self.pause_blink_cursor(cx);
            return;
        }

        let offset =
            TextSelector::previous_word_start_at(&self.text, self.active_selection().start);
        self.replace_text_in_range_silent(
            Some(self.range_to_utf16(&(offset..self.cursor()))),
            "",
            window,
            cx,
        );
        self.pause_blink_cursor(cx);
    }

    pub(super) fn delete_next_word(
        &mut self,
        _: &DeleteToNextWordEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.active_selection().is_collapsed() {
            self.replace_text_in_range(None, "", window, cx);
            self.pause_blink_cursor(cx);
            return;
        }

        let offset = TextSelector::next_word_end_at(&self.text, self.cursor());
        self.replace_text_in_range_silent(
            Some(self.range_to_utf16(&(self.cursor()..offset))),
            "",
            window,
            cx,
        );
        self.pause_blink_cursor(cx);
    }

    pub(super) fn enter(&mut self, action: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if self.handle_action_for_context_menu(Box::new(action.clone()), window, cx) {
            return;
        }

        // Clear inline completion on enter (user chose not to accept it)
        if self.has_inline_completion() {
            self.clear_inline_completion(cx);
        }

        if self.mode.is_multi_line() {
            // Get current line indent
            let indent = if self.mode.is_code_editor() {
                self.indent_of_next_line()
            } else {
                "".to_string()
            };

            // Add newline and indent
            let new_line_text = format!("\n{}", indent);
            self.replace_text_in_range_silent(None, &new_line_text, window, cx);
            self.pause_blink_cursor(cx);
        } else {
            // Single line input, just emit the event (e.g.: In a dialog to confirm).
            cx.propagate();
        }

        cx.emit(InputEvent::PressEnter {
            secondary: action.secondary,
        });
    }

    pub(super) fn clean(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text("", window, cx);
        self.set_cursor_to(0);
        self.scroll_to(0, None, cx);
    }

    pub(super) fn escape(&mut self, action: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        if self.handle_action_for_context_menu(Box::new(action.clone()), window, cx) {
            return;
        }

        // Remove extra cursors on escape when in multi-cursor mode
        if !self.selections.is_single() {
            self.selections.remove_all_but_active();
        }

        // Clear inline completion on escape
        if self.has_inline_completion() {
            self.clear_inline_completion(cx);
            return; // Consume the escape, don't propagate
        }

        if self.ime_marked_range.is_some() {
            self.unmark_text(window, cx);
        }

        if self.clean_on_escape {
            return self.clean(window, cx);
        }

        cx.propagate();
    }

    /// Add a cursor at the specified offset.
    /// Does not add a cursor if it would overlap with an existing selection.
    pub(super) fn add_cursor_at(&mut self, offset: usize, cx: &mut Context<Self>) {
        // Check if offset is inside or at the boundary of any existing selection
        for sel in self.selections.iter() {
            // Check if the new cursor position is inside the selection
            if sel.contains(offset) {
                return;
            }

            // Also check if the cursor is at the exact same position as an existing cursor
            if sel.is_collapsed() && sel.cursor_offset() == offset {
                return;
            }
        }

        let id = self.selections.generate_id();
        let new_cursor = Selection::new(id, offset, offset);
        self.selections.add(new_cursor);
        cx.notify();
    }

    /// Build a columnar selection from start_offset to end_offset.
    /// This creates multiple selections, one per line, at the same column positions.
    pub(super) fn build_columnar_selection(
        &mut self,
        start_offset: usize,
        end_offset: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (start, end) = if start_offset < end_offset {
            (start_offset, end_offset)
        } else {
            (end_offset, start_offset)
        };

        let start_point = self.text.offset_to_point(start);
        let end_point = self.text.offset_to_point(end);

        // Get the visual column positions for start and end
        let start_display_point = self.display_map.offset_to_wrap_display_point(start);
        let end_display_point = self.display_map.offset_to_wrap_display_point(end);

        let start_col = start_display_point.column;
        let end_col = end_display_point.column;

        let (start_col, end_col) = if start_col < end_col {
            (start_col, end_col)
        } else {
            (end_col, start_col)
        };

        let start_row = start_point.row.min(end_point.row);
        let end_row = start_point.row.max(end_point.row);

        // Build selections for each row in the range
        let num_rows = end_row - start_row + 1;
        let mut new_selections = Vec::with_capacity(num_rows);
        for row in start_row..=end_row {
            let line = self.text.slice_line(row);
            let line_start_offset = self.text.line_start_offset(row);

            // Clamp column to line length
            let line_start_col = start_col.min(line.len());
            let line_end_col = end_col.min(line.len());

            // Create selection (collapsed if columns are equal, range if different)
            if line_start_col <= line_end_col {
                let sel_start = line_start_offset + line_start_col;
                let sel_end = line_start_offset + line_end_col;

                let id = self.selections.generate_id();
                new_selections.push(Selection::new(id, sel_start, sel_end));
            }
        }

        // If we couldn't create any columnar selections (e.g., out of bounds),
        // create at least one cursor at the end position to avoid empty selections
        if new_selections.is_empty() {
            let id = self.selections.generate_id();
            new_selections.push(Selection::new(id, end, end));
        }

        self.selections.replace_all(new_selections);
        cx.notify();
    }

    /// Split the current selection into one selection per line.
    /// This is useful when you have a multi-line selection and want to edit
    /// each line independently.
    pub(super) fn split_selection_to_lines(
        &mut self,
        _: &SplitSelectionToLines,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = self.active_selection();
        let (start, end) = if active.start < active.end {
            (active.start, active.end)
        } else {
            (active.end, active.start)
        };

        if start == end {
            return; // No selection to split
        }

        let start_point = self.text.offset_to_point(start);
        let end_point = self.text.offset_to_point(end);

        if start_point.row == end_point.row {
            return; // Single line
        }

        let num_rows = end_point.row - start_point.row + 1;
        let mut new_selections = Vec::with_capacity(num_rows);

        // Add a selection for each line in the range
        for row in start_point.row..=end_point.row {
            let line_start_offset = self.text.line_start_offset(row);
            let line = self.text.slice_line(row);
            let line_len = line.len();

            // For the first line, start from the original start position
            // For middle lines, select the entire line
            // For the last line, end at the original end position
            let (sel_start, sel_end) = if row == start_point.row {
                (start, line_start_offset + line_len)
            } else if row == end_point.row {
                (line_start_offset, end)
            } else {
                (line_start_offset, line_start_offset + line_len)
            };

            if sel_start < sel_end {
                let id = self.selections.generate_id();
                new_selections.push(Selection::new(id, sel_start, sel_end));
            }
        }

        if !new_selections.is_empty() {
            self.selections.replace_all(new_selections);
        }
        cx.notify();
    }

    pub(super) fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Clear inline completion on any mouse interaction
        self.clear_inline_completion(cx);

        // If there have IME marked range and is empty (Means pressed Esc to abort IME typing)
        // Clear the marked range.
        if let Some(ime_marked_range) = &self.ime_marked_range {
            if ime_marked_range.len() == 0 {
                self.ime_marked_range = None;
            }
        }

        self.selecting = true;
        let offset = self.index_for_mouse_position(event.position);

        if self.handle_click_hover_definition(event, offset, window, cx) {
            return;
        }

        // Triple click to select line
        if event.button == MouseButton::Left && event.click_count >= 3 {
            self.select_line(offset, window, cx);
            return;
        }

        // Double click to select word
        if event.button == MouseButton::Left && event.click_count == 2 {
            self.select_word(offset, window, cx);
            return;
        }

        // Show Mouse context menu
        if event.button == MouseButton::Right {
            self.handle_right_click_menu(event, offset, window, cx);
            return;
        }

        // Alt+Shift+Click to start column selection (columnar selection)
        if event.button == MouseButton::Left && event.modifiers.alt && event.modifiers.shift {
            self.column_select_start = Some(offset);
            self.move_to_offset(offset, None, cx);
            return;
        }

        // Alt+Click to add cursor at clicked position
        if event.button == MouseButton::Left && event.modifiers.alt {
            self.add_cursor_at(offset, cx);
            return;
        }

        // Regular click without modifiers, remove extra cursors
        if event.button == MouseButton::Left
            && !self.selections.is_single()
            && !event.modifiers.shift
        {
            self.selections.remove_all_but_active();
        }

        if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to_offset(offset, None, cx)
        }
    }

    pub(super) fn on_mouse_up(
        &mut self,
        _: &MouseUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if self.active_selection().is_collapsed() {
            self.active_selection_mut().reversed = false;
        }
        self.selecting = false;
        self.selected_word_range = None;
        self.column_select_start = None;
    }

    pub(super) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Show diagnostic popover on mouse move
        let offset = self.index_for_mouse_position(event.position);
        self.handle_mouse_move(offset, event, window, cx);

        if self.mode.is_code_editor() {
            if let Some(diagnostic) = self
                .mode
                .diagnostics()
                .and_then(|set| set.for_offset(offset))
            {
                if let Some(diagnostic_popover) = self.diagnostic_popover.as_ref() {
                    if diagnostic_popover.read(cx).diagnostic.range == diagnostic.range {
                        diagnostic_popover.update(cx, |this, cx| {
                            this.show(cx);
                        });

                        return;
                    }
                }

                self.diagnostic_popover = Some(DiagnosticPopover::new(diagnostic, cx.entity(), cx));
                cx.notify();
            } else {
                if let Some(diagnostic_popover) = self.diagnostic_popover.as_mut() {
                    diagnostic_popover.update(cx, |this, cx| {
                        this.check_to_hide(event.position, cx);
                    })
                }
            }
        }
    }

    pub(super) fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let line_height = self
            .last_layout
            .as_ref()
            .map(|layout| layout.line_height)
            .unwrap_or(window.line_height());
        let delta = event.delta.pixel_delta(line_height);

        let old_offset = self.scroll_handle.offset();
        self.update_scroll_offset(Some(old_offset + delta), cx);

        // Only stop propagation if the offset actually changed
        if self.scroll_handle.offset() != old_offset {
            cx.stop_propagation();
        }

        self.diagnostic_popover = None;
    }

    pub(super) fn update_scroll_offset(
        &mut self,
        offset: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let mut offset = offset.unwrap_or(self.scroll_handle.offset());
        // In addition to left alignment, a cursor position will be reserved on the right side
        let safe_x_offset = if self.text_align == TextAlign::Left {
            px(0.)
        } else {
            -CURSOR_WIDTH
        };

        let safe_y_range =
            (-self.scroll_size.height + self.input_bounds.size.height).min(px(0.0))..px(0.);
        let safe_x_range = (-self.scroll_size.width + self.input_bounds.size.width + safe_x_offset)
            .min(safe_x_offset)..px(0.);

        offset.y = if self.mode.is_single_line() {
            px(0.)
        } else {
            offset.y.clamp(safe_y_range.start, safe_y_range.end)
        };
        offset.x = offset.x.clamp(safe_x_range.start, safe_x_range.end);
        self.scroll_handle.set_offset(offset);
        cx.notify();
    }

    /// Scroll to make the given offset visible.
    ///
    /// If `direction` is Some, will keep edges at the same side.
    pub(crate) fn scroll_to(
        &mut self,
        offset: usize,
        direction: Option<MoveDirection>,
        cx: &mut Context<Self>,
    ) {
        let Some(last_layout) = self.last_layout.as_ref() else {
            return;
        };
        let Some(bounds) = self.last_bounds.as_ref() else {
            return;
        };

        let mut scroll_offset = self.scroll_handle.offset();
        let was_offset = scroll_offset;
        let line_height = last_layout.line_height;

        let point = self.text.offset_to_point(offset);

        let row = point.row;

        let mut row_offset_y = px(0.);
        for (ix, _wrap_line) in self.display_map.lines().iter().enumerate() {
            if ix == row {
                break;
            }

            // Only accumulate height for visible (non-folded) wrap rows
            let visible_wrap_rows = self.display_map.visible_wrap_row_count_for_buffer_line(ix);
            row_offset_y += line_height * visible_wrap_rows;
        }

        // Apart from left alignment, just leave enough space for the cursor size on the right side.
        let safety_margin = if last_layout.text_align == TextAlign::Left {
            RIGHT_MARGIN
        } else {
            CURSOR_WIDTH
        };
        if let Some(line) = last_layout
            .lines
            .get(row.saturating_sub(last_layout.visible_range.start))
        {
            // Check to scroll horizontally and soft wrap lines
            if let Some(pos) = line.position_for_index(point.column, last_layout) {
                let bounds_width = bounds.size.width - last_layout.line_number_width;
                let col_offset_x = pos.x;
                row_offset_y += pos.y;
                if col_offset_x - safety_margin < -scroll_offset.x {
                    // If the position is out of the visible area, scroll to make it visible
                    scroll_offset.x = -col_offset_x + safety_margin;
                } else if col_offset_x + safety_margin > -scroll_offset.x + bounds_width {
                    scroll_offset.x = -(col_offset_x - bounds_width + safety_margin);
                }
            }
        }

        // Check if row_offset_y is out of the viewport
        // If row offset is not in the viewport, scroll to make it visible
        let edge_height = if direction.is_some() && self.mode.is_code_editor() {
            3 * line_height
        } else {
            line_height
        };
        if row_offset_y - edge_height + line_height < -scroll_offset.y {
            // Scroll up
            scroll_offset.y = -row_offset_y + edge_height - line_height;
        } else if row_offset_y + edge_height > -scroll_offset.y + bounds.size.height {
            // Scroll down
            scroll_offset.y = -(row_offset_y - bounds.size.height + edge_height);
        }

        // Avoid necessary scroll, when it was already in the correct position.
        if direction == Some(MoveDirection::Up) {
            scroll_offset.y = scroll_offset.y.max(was_offset.y);
        } else if direction == Some(MoveDirection::Down) {
            scroll_offset.y = scroll_offset.y.min(was_offset.y);
        }

        scroll_offset.x = scroll_offset.x.min(px(0.));
        scroll_offset.y = scroll_offset.y.min(px(0.));
        self.deferred_scroll_offset = Some(scroll_offset);
        cx.notify();
    }

    pub(super) fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    pub(super) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let selection = self.active_selection();
        if selection.is_collapsed() {
            return;
        }

        let selected_text = self.text.slice(selection.start..selection.end).to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(selected_text));
    }

    pub(super) fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self.active_selection();
        if selection.is_collapsed() {
            return;
        }

        let selected_text = self.text.slice(selection.start..selection.end).to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(selected_text));

        self.replace_text_in_range_silent(None, "", window, cx);
    }

    pub(super) fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(clipboard) = cx.read_from_clipboard() {
            let mut new_text = clipboard.text().unwrap_or_default();
            if !self.mode.is_multi_line() {
                new_text = new_text.replace('\n', "");
            }

            self.replace_text_in_range_silent(None, &new_text, window, cx);
            self.scroll_to(self.cursor(), None, cx);
        }
    }

    fn push_history(
        &mut self,
        text: &Rope,
        old_range: impl Into<Selection>,
        new_range: impl Into<Selection>,
        new_text: &str,
        operation_type: OperationType,
        shared_operation_id: Option<usize>,
    ) {
        if self.history.ignore {
            return;
        }

        // If operation type changed from last time, force a new undo group
        // Don't force for the first operation (when last_operation_type is None) to allow
        // initial operations of the same type to be grouped by time
        if self.last_operation_type.is_some() && self.last_operation_type != Some(operation_type) {
            self.history.force_new_group();
        }

        let old_range = old_range.into();
        let new_range = new_range.into();
        let old_text = text.slice(old_range.start..old_range.end).to_string();

        // Use provided operation_id or get current counter
        let operation_id = shared_operation_id.unwrap_or(self.operation_id_counter);

        let change = Change::new(
            old_range.clone(),
            &old_text,
            new_range,
            new_text,
            operation_id,
        );
        self.history.push(change);

        // Update the last operation type
        self.last_operation_type = Some(operation_type);

        // Only increment counter if not using a shared operation_id
        if shared_operation_id.is_none() {
            self.operation_id_counter += 1;
        }
    }

    pub(super) fn undo(&mut self, _: &Undo, _window: &mut Window, cx: &mut Context<Self>) {
        self.history.ignore = true;
        if let Some(changes) = self.history.undo() {
            // Check if this is a multi-cursor operation
            let has_multi_cursor = changes.len() > 1;

            // Skip cursor movement during multi-cursor undo to preserve restored selections
            if has_multi_cursor {
                self.skip_set_cursor = true;
            }

            // Group changes by operation_id and process operations in reverse order
            // (highest operation_id first, since that was the last operation done)
            let mut ops: BTreeMap<usize, Vec<&Change>> = BTreeMap::new();
            for change in &changes {
                ops.entry(change.operation_id).or_default().push(change);
            }

            // Process operations in reverse order (highest operation_id first)
            for (_op_id, op_changes) in ops.into_iter().rev() {
                // Within each operation, sort by old_range.start ASCENDING (left-to-right)
                // This allows us to use simple stored positions
                let mut sorted_changes: Vec<_> = op_changes.iter().collect();
                sorted_changes.sort_by_key(|c| c.old_range.start);

                for change in sorted_changes {
                    let pos = change.old_range.start;

                    // Delete the new text from old_range.start (it was inserted here)
                    if !change.new_text.is_empty() {
                        let delete_end = pos + change.new_text.len();
                        if pos < self.text.len() && delete_end <= self.text.len() {
                            self.text.remove(pos..delete_end);
                        }
                    }

                    // Insert the old text at the original position
                    if !change.old_text.is_empty() {
                        let pos = pos.min(self.text.len());
                        self.text.insert(pos, &change.old_text);
                    }
                }
            }

            // Restore cursor positions from old_range of changes with the lowest operation_id
            // (the state before any of the grouped operations happened)
            let min_op_id = changes.iter().map(|c| c.operation_id).min().unwrap_or(0);
            let restored_selections: Vec<Selection> = changes
                .iter()
                .filter(|c| c.operation_id == min_op_id)
                .map(|c| c.old_range.clone())
                .collect();
            self.selections.replace_all(restored_selections);

            // Re-enable cursor movement after undo is complete
            self.skip_set_cursor = false;

            self.display_map
                .on_text_changed(&self.text, &(0..self.text.len()), &self.text, cx);
            // Re-parse syntax highlighting from scratch after undo
            self.mode.update_highlighter(None, &self.text, true, cx);
            self.update_fold_candidates();
            if let Some(diagnostics) = self.mode.diagnostics_mut() {
                diagnostics.reset(&self.text)
            }
            self.update_preferred_column();
            self.update_search(cx);
            cx.emit(InputEvent::Change);
            cx.notify();
        }
        self.last_operation_type = None;
        self.history.ignore = false;
    }

    pub(super) fn redo(&mut self, _: &Redo, _window: &mut Window, cx: &mut Context<Self>) {
        self.history.ignore = true;
        if let Some(changes) = self.history.redo() {
            // Check if this is a multi-cursor operation
            let has_multi_cursor = changes.len() > 1;

            // Preserve the operation type from the changes we're redoing
            let last_op_type = changes.iter().rev().find_map(|c| {
                if c.new_text.is_empty() && c.old_text.is_empty() {
                    None
                } else if !c.new_text.is_empty() {
                    Some(OperationType::Insert)
                } else {
                    Some(OperationType::Delete)
                }
            });

            // Skip cursor movement during multi-cursor redo to preserve restored selections
            if has_multi_cursor {
                self.skip_set_cursor = true;
            }

            // Group changes by operation_id and process operations in ascending order
            // (the original order they were created)
            let mut ops: BTreeMap<usize, Vec<&Change>> = BTreeMap::new();
            for change in &changes {
                ops.entry(change.operation_id).or_default().push(change);
            }

            // Process operations in ascending order (lowest operation_id first)
            for (_op_id, op_changes) in ops.into_iter() {
                // Within each operation, sort by old_range.start descending to redo right-to-left
                let mut sorted_changes: Vec<_> = op_changes.iter().collect();
                sorted_changes.sort_by(|a, b| b.old_range.start.cmp(&a.old_range.start));

                for change in sorted_changes {
                    let pos = change.old_range.start;

                    // Delete the old text from the position (if any)
                    if !change.old_text.is_empty() {
                        let old_text_end = pos + change.old_text.len();
                        if pos < self.text.len() && old_text_end <= self.text.len() {
                            self.text.remove(pos..old_text_end);
                        }
                    }

                    // Insert the new text at the position
                    if !change.new_text.is_empty() {
                        let insert_pos = pos.min(self.text.len());
                        self.text.insert(insert_pos, &change.new_text);
                    }
                }
            }

            // Calculate final cursor positions after all redos
            // For each cursor: final_pos = old_range.start + (inserts_at_or_before * new_text.len()) - (deletions_before)
            let max_op_id = changes.iter().map(|c| c.operation_id).max().unwrap_or(0);
            let last_op_changes: Vec<&Change> = changes
                .iter()
                .filter(|c| c.operation_id == max_op_id)
                .collect();

            // Sort by old_range.start to calculate positions
            let mut sorted_for_calc: Vec<_> = last_op_changes.iter().collect();
            sorted_for_calc.sort_by_key(|c| c.old_range.start);

            let mut restored_selections: Vec<Selection> = Vec::with_capacity(last_op_changes.len());
            for change in &sorted_for_calc {
                let insert_len = change.new_text.len();
                let inserts_at_or_before = sorted_for_calc
                    .iter()
                    .filter(|c| c.old_range.start <= change.old_range.start)
                    .count();
                let deletions_before: usize = sorted_for_calc
                    .iter()
                    .filter(|c| c.old_range.start < change.old_range.start)
                    .map(|c| c.old_text.len())
                    .sum();
                let final_pos =
                    change.old_range.start + inserts_at_or_before * insert_len - deletions_before;

                let mut sel = change.new_range.clone();
                sel.start = final_pos;
                sel.end = final_pos;
                restored_selections.push(sel);
            }
            self.selections.replace_all(restored_selections);

            // Re-enable cursor movement after redo is complete
            self.skip_set_cursor = false;

            // Final UI update
            self.display_map
                .on_text_changed(&self.text, &(0..self.text.len()), &self.text, cx);
            // Re-parse syntax highlighting from scratch after redo
            self.mode.update_highlighter(None, &self.text, true, cx);
            self.update_fold_candidates();
            if let Some(diagnostics) = self.mode.diagnostics_mut() {
                diagnostics.reset(&self.text)
            }
            self.update_preferred_column();
            self.update_search(cx);
            cx.emit(InputEvent::Change);
            cx.notify();

            // Restore the last operation type from the changes we just redone
            self.last_operation_type = last_op_type;
        }
        self.history.ignore = false;
    }

    /// Get byte offset of the cursor.
    ///
    /// The offset is the UTF-8 offset.
    pub fn cursor(&self) -> usize {
        if let Some(ime_marked_range) = &self.ime_marked_range {
            return ime_marked_range.end;
        }

        self.selections.active().cursor_offset()
    }

    /// Returns the active selection for reading
    pub fn active_selection(&self) -> &Selection {
        self.selections.active()
    }

    /// Returns the active selection range for external code (rendering, etc.)
    pub fn active_selection_range(&self) -> Range<usize> {
        let sel = self.selections.active();
        sel.start..sel.end
    }

    /// Sets the active selection range (used by indent operations)
    pub fn set_active_range(&mut self, start: usize, end: usize) {
        let active = self.active_selection_mut();
        active.start = start;
        active.end = end;
    }

    /// Returns a mutable reference to the active selection
    fn active_selection_mut(&mut self) -> &mut Selection {
        let active_id = self.selections.active().id;
        self.selections
            .iter_mut()
            .find(|s| s.id == active_id)
            .expect("Active selection should always exist")
    }

    /// Sets the active selection to a new range
    pub fn set_selection(&mut self, start: usize, end: usize) {
        let active = self.active_selection_mut();
        active.start = start;
        active.end = end;
    }

    /// Sets the active selection to a collapsed cursor at the given offset
    pub fn set_cursor_to(&mut self, offset: usize) {
        let active = self.active_selection_mut();
        active.start = offset;
        active.end = offset;
    }

    pub(crate) fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        // If the text is empty, always return 0
        if self.text.len() == 0 {
            return 0;
        }

        let (Some(bounds), Some(last_layout)) =
            (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };

        let line_height = last_layout.line_height;
        let line_number_width = last_layout.line_number_width;

        // TIP: About the IBeam cursor
        //
        // If cursor style is IBeam, the mouse mouse position is in the middle of the cursor (This is special in OS)

        // The position is relative to the bounds of the text input
        //
        // bounds.origin:
        //
        // - included the input padding.
        // - included the scroll offset.
        let inner_position = position - bounds.origin - point(line_number_width, px(0.));

        let mut y_offset = last_layout.visible_top;

        // Traverse visible buffer lines
        for (line_index, line_layout) in last_layout.lines.iter().enumerate() {
            // visible_range is based on buffer lines, so this gives us the buffer line directly
            let buffer_line = last_layout.visible_range.start + line_index;

            // Skip hidden (folded) lines - they have 0 height
            if self.display_map.is_buffer_line_hidden(buffer_line) {
                continue;
            }

            let line_start_offset = self.text.line_start_offset(buffer_line);

            // Calculate line origin for this display row
            let line_origin = point(px(0.), y_offset);
            let pos = inner_position - line_origin;

            // Return offset by use closest_index_for_x if is single line mode.
            if self.mode.is_single_line() {
                let local_index = line_layout.closest_index_for_x(pos.x, last_layout);
                let index = line_start_offset + local_index;
                return if self.masked {
                    self.text.char_index_to_offset(index)
                } else {
                    index.min(self.text.len())
                };
            }

            // Check if mouse is in this line's bounds
            if let Some(local_index) = line_layout.closest_index_for_position(pos, last_layout) {
                let index = line_start_offset + local_index;
                return if self.masked {
                    self.text.char_index_to_offset(index)
                } else {
                    index.min(self.text.len())
                };
            } else if pos.y < px(0.) {
                // Mouse is above this line, return start of this line
                return if self.masked {
                    self.text.char_index_to_offset(line_start_offset)
                } else {
                    line_start_offset
                };
            }

            y_offset += line_layout.size(line_height).height;
        }

        // Mouse is below all visible lines, return end of text
        let index = self.text.len();
        if self.masked {
            self.text.char_index_to_offset(index)
        } else {
            index
        }
    }

    /// Returns a y offsetted point for the line origin.
    /// Select the text from the current cursor position to the given offset.
    ///
    /// The offset is the UTF-8 offset.
    ///
    /// Ensure the offset use self.next_boundary or self.previous_boundary to get the correct offset.
    pub(crate) fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.clear_inline_completion(cx);

        let offset = offset.clamp(0, self.text.len());

        // Get the word range before we take a mutable reference to active
        let word_range_to_keep = self
            .selected_word_range
            .as_ref()
            .map(|wr| (wr.start, wr.end));

        let active = self.active_selection_mut();

        Self::extend_selection_to(active, offset);

        // Ensure keep word selected range
        if let Some((start, end)) = word_range_to_keep {
            if active.start > start {
                active.start = start;
            }
            if active.end < end {
                active.end = end;
            }
        }
        if active.is_collapsed() {
            self.update_preferred_column();
        }
        cx.notify()
    }

    /// Unselects the currently selected text.
    pub fn unselect(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.cursor();
        self.set_cursor_to(offset);
        cx.notify()
    }

    #[inline]
    pub(super) fn offset_from_utf16(&self, offset: usize) -> usize {
        self.text.offset_utf16_to_offset(offset)
    }

    #[inline]
    pub(super) fn offset_to_utf16(&self, offset: usize) -> usize {
        self.text.offset_to_offset_utf16(offset)
    }

    #[inline]
    pub(super) fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    #[inline]
    pub(super) fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    /// If offset falls on a hidden (folded) line, clamp backward to the end of
    /// the fold header line (last visible position before the fold).
    fn clamp_offset_to_visible_backward(&self, offset: usize) -> usize {
        let line = self.text.offset_to_point(offset).row;
        if self.display_map.is_buffer_line_hidden(line) {
            for fold in self.display_map.folded_ranges() {
                if line > fold.start_line && line <= fold.end_line {
                    return self.text.line_end_offset(fold.start_line);
                }
            }
        }
        offset
    }

    /// If offset falls on a hidden (folded) line, clamp forward to the start of
    /// the fold end line (first visible position after the fold).
    fn clamp_offset_to_visible_forward(&self, offset: usize) -> usize {
        let line = self.text.offset_to_point(offset).row;
        if self.display_map.is_buffer_line_hidden(line) {
            for fold in self.display_map.folded_ranges() {
                if line > fold.start_line && line <= fold.end_line {
                    return self.text.line_start_offset(fold.end_line);
                }
            }
        }
        offset
    }

    pub(super) fn previous_boundary(&self, offset: usize) -> usize {
        let mut offset = self.text.clip_offset(offset.saturating_sub(1), Bias::Left);
        if let Some(ch) = self.text.char_at(offset) {
            if ch == '\r' {
                offset -= 1;
            }
        }

        self.clamp_offset_to_visible_backward(offset)
    }

    pub(super) fn next_boundary(&self, offset: usize) -> usize {
        let mut offset = self.text.clip_offset(offset + 1, Bias::Right);
        if let Some(ch) = self.text.char_at(offset) {
            if ch == '\r' {
                offset += 1;
            }
        }

        self.clamp_offset_to_visible_forward(offset)
    }

    /// Returns the true to let InputElement to render cursor, when Input is focused and current BlinkCursor is visible.
    pub(crate) fn show_cursor(&self, window: &Window, cx: &App) -> bool {
        (self.focus_handle.is_focused(window) || self.is_context_menu_open(cx))
            && !self.disabled
            && self.blink_cursor.read(cx).visible()
            && window.is_window_active()
    }

    fn on_focus(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.blink_cursor.update(cx, |cursor, cx| {
            cursor.start(cx);
        });
        cx.emit(InputEvent::Focus);
    }

    fn on_blur(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_context_menu_open(cx) {
            return;
        }

        // NOTE: Do not cancel select, when blur.
        // Because maybe user want to copy the selected text by AppMenuBar (will take focus handle).

        self.hover_popover = None;
        self.diagnostic_popover = None;
        self.context_menu = None;
        self.clear_inline_completion(cx);
        self.blink_cursor.update(cx, |cursor, cx| {
            cursor.stop(cx);
        });
        Root::update(window, cx, |root, _, _| {
            root.focused_input = None;
        });
        cx.emit(InputEvent::Blur);
        cx.notify();
    }

    pub(super) fn pause_blink_cursor(&mut self, cx: &mut Context<Self>) {
        self.blink_cursor.update(cx, |cursor, cx| {
            cursor.pause(cx);
        });
    }

    pub(super) fn on_key_down(&mut self, _: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.pause_blink_cursor(cx);
    }

    pub(super) fn on_drag_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.text.len() == 0 {
            return;
        }

        if self.last_layout.is_none() {
            return;
        }

        if !self.focus_handle.is_focused(window) {
            return;
        }

        if !self.selecting {
            return;
        }

        let offset = self.index_for_mouse_position(event.position);

        // Handle column selection when Alt is held
        if event.modifiers.alt {
            if let Some(start_offset) = self.column_select_start {
                self.build_columnar_selection(start_offset, offset, window, cx);
                return;
            }
        }

        self.select_to(offset, cx);
    }

    fn is_valid_input(&self, new_text: &str, cx: &mut Context<Self>) -> bool {
        if new_text.is_empty() {
            return true;
        }

        if let Some(validate) = &self.validate {
            if !validate(new_text, cx) {
                return false;
            }
        }

        if !self.mask_pattern.is_valid(new_text) {
            return false;
        }

        let Some(pattern) = &self.pattern else {
            return true;
        };

        pattern.is_match(new_text)
    }

    /// Set the mask pattern for formatting the input text.
    ///
    /// The pattern can contain:
    /// - 9: Any digit or dot
    /// - A: Any letter
    /// - *: Any character
    /// - Other characters will be treated as literal mask characters
    ///
    /// Example: "(999)999-999" for phone numbers
    pub fn mask_pattern(mut self, pattern: impl Into<MaskPattern>) -> Self {
        self.mask_pattern = pattern.into();
        if let Some(placeholder) = self.mask_pattern.placeholder() {
            self.placeholder = placeholder.into();
        }
        self
    }

    pub fn set_mask_pattern(
        &mut self,
        pattern: impl Into<MaskPattern>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mask_pattern = pattern.into();
        if let Some(placeholder) = self.mask_pattern.placeholder() {
            self.placeholder = placeholder.into();
        }
        cx.notify();
    }

    pub(super) fn set_input_bounds(&mut self, new_bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        let wrap_width_changed = self.input_bounds.size.width != new_bounds.size.width;
        self.input_bounds = new_bounds;

        // Update display_map wrap_width if changed.
        if let Some(last_layout) = self.last_layout.as_ref() {
            if wrap_width_changed {
                let wrap_width = if !self.soft_wrap {
                    // None to disable wrapping (will use Pixels::MAX)
                    None
                } else {
                    last_layout.wrap_width
                };

                self.display_map.on_layout_changed(wrap_width, cx);
                self.mode.update_auto_grow(&self.display_map);
                cx.notify();
            }
        }
    }

    pub(super) fn selected_text(&self) -> RopeSlice<'_> {
        let selection = self.active_selection();
        let range = selection.start..selection.end;
        let range_utf16 = self.range_to_utf16(&range);
        let range = self.range_from_utf16(&range_utf16);
        self.text.slice(range)
    }

    pub(crate) fn range_to_bounds(&self, range: &Range<usize>) -> Option<Bounds<Pixels>> {
        let Some(last_layout) = self.last_layout.as_ref() else {
            return None;
        };

        let Some(last_bounds) = self.last_bounds else {
            return None;
        };

        let (_, _, start_pos) = self.line_and_position_for_offset(range.start);
        let (_, _, end_pos) = self.line_and_position_for_offset(range.end);

        let Some(start_pos) = start_pos else {
            return None;
        };
        let Some(end_pos) = end_pos else {
            return None;
        };

        Some(Bounds::from_corners(
            last_bounds.origin + start_pos,
            last_bounds.origin + end_pos + point(px(0.), last_layout.line_height),
        ))
    }

    /// Replace text in range in silent.
    ///
    /// This will not trigger any UI interaction, such as auto-completion.
    pub(crate) fn replace_text_in_range_silent(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.silent_replace_text = true;
        self.replace_text_in_range(range_utf16, new_text, window, cx);
        self.silent_replace_text = false;
    }

    /// Update fold candidates from tree-sitter syntax tree (full extraction).
    /// Used only on initial load or language changes.
    fn update_fold_candidates(&mut self) {
        if !self.mode.is_folding() {
            return;
        }

        let Some(highlighter_rc) = self.mode.highlighter() else {
            return;
        };

        let highlighter = highlighter_rc.borrow();
        let Some(highlighter) = highlighter.as_ref() else {
            return;
        };

        let Some(tree) = highlighter.tree() else {
            return;
        };

        let fold_ranges = crate::input::display_map::extract_fold_ranges(tree);
        self.display_map.set_fold_candidates(fold_ranges);
    }

    /// Incrementally update fold candidates after a text edit.
    /// Only traverses the edited region of the syntax tree instead of the full tree.
    fn update_fold_candidates_incremental(&mut self, edit_range: &Range<usize>, new_text: &str) {
        if !self.mode.is_folding() {
            return;
        }

        let Some(highlighter_rc) = self.mode.highlighter() else {
            return;
        };

        let highlighter = highlighter_rc.borrow();
        let Some(highlighter) = highlighter.as_ref() else {
            return;
        };

        let Some(tree) = highlighter.tree() else {
            return;
        };

        // The new byte range in the updated text after the edit
        let new_end = edit_range.start + new_text.len();
        self.display_map.update_fold_candidates_for_edit(
            tree,
            edit_range.start..new_end,
            &self.text,
        );
    }
}

impl EntityInputHandler for InputState {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        adjusted_range.replace(self.range_to_utf16(&range));
        Some(self.text.slice(range).to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let selection = self.active_selection();
        Some(UTF16Selection {
            range: self.range_to_utf16(&(selection.start..selection.end)),
            reversed: selection.reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.ime_marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(&range.clone().into()))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.ime_marked_range = None;
    }

    /// Replace text in range.
    ///
    /// - If the new text is invalid, it will not be replaced.
    /// - If `range_utf16` is not provided, the current selected range will be used.
    /// - If there are multiple cursors, text will be inserted at all cursors.
    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }

        self.pause_blink_cursor(cx);

        let is_external_edit = range_utf16.is_some() || self.ime_marked_range.is_some();
        let is_single_cursor = self.selections.is_single();

        // For external edits or single cursor, use the simple path
        if is_external_edit || is_single_cursor {
            let selection = self.active_selection_range();
            let range = range_utf16
                .as_ref()
                .map(|r| self.range_from_utf16(r))
                .or(self.ime_marked_range.as_ref().map(|s| s.start..s.end))
                .unwrap_or(selection);

            let old_text = self.text.clone();
            self.text.replace(range.clone(), new_text);

            if self.mode.is_single_line() {
                let pending_text = self.text.to_string();
                if !self.is_valid_input(&pending_text, cx) {
                    self.text = old_text;
                    return;
                }
            }

            let operation_type = if new_text.is_empty() {
                OperationType::Delete
            } else if range.is_empty() {
                OperationType::Insert
            } else {
                OperationType::Delete
            };

            let new_range = range.start..range.start + new_text.len();
            self.push_history(
                &old_text,
                range.clone(),
                new_range,
                new_text,
                operation_type,
                None,
            );

            let new_offset = range.start + new_text.len();
            self.selections.replace_all(vec![Selection::new(
                self.selections.active().id,
                new_offset,
                new_offset,
            )]);

            if let Some(diagnostics) = self.mode.diagnostics_mut() {
                diagnostics.reset(&self.text)
            }
            self.display_map
                .on_text_changed(&self.text, &(0..self.text.len()), &self.text, cx);
            self.mode.update_highlighter(None, &self.text, true, cx);
            self.lsp.update(&self.text, window, cx);
            self.ime_marked_range.take();
            self.update_preferred_column();
            self.update_search(cx);
            self.mode.update_auto_grow(&self.display_map);
            cx.emit(InputEvent::Change);
            cx.notify();
            return;
        }

        // Multi-cursor edit path
        let edits: Vec<Selection> = self.selections.iter().cloned().collect();

        // Sort edits DESC by position (right-to-left processing)
        let mut sorted_edits = edits;
        sorted_edits.sort_by(|a, b| b.start.cmp(&a.start));

        let insert_len = new_text.len();
        let num_edits = sorted_edits.len();
        let all_collapsed = sorted_edits.iter().all(|e| e.is_collapsed());

        // Determine operation type
        let operation_type = if all_collapsed && !new_text.is_empty() {
            OperationType::Insert
        } else {
            OperationType::Delete
        };

        // Start grouping for this operation
        self.history.start_grouping();

        let old_text = self.text.clone();
        let operation_id = self.operation_id_counter;

        // Apply edits (right-to-left to avoid position shifts)
        // For selections: replace range with new_text
        // For collapsed cursors: insert new_text at position
        for edit in &sorted_edits {
            if edit.is_collapsed() {
                // Collapsed cursor: just insert
                if !new_text.is_empty() {
                    self.text.insert(edit.start, new_text);
                }
            } else {
                // Selection: replace selected text with new_text
                self.text.replace(edit.start..edit.end, new_text);
            }
        }

        let mut new_selections = Vec::with_capacity(num_edits);

        // Create history changes with SIMPLE cursor positions
        // For undo ASC: delete from old_range.start for new_text.len()
        // For redo DESC: insert at old_range.start
        // Calculate final cursor positions for display (same formula as redo)
        for (i, edit) in sorted_edits.iter().enumerate() {
            // Store simple cursor position after this edit
            let simple_cursor_pos = if new_text.is_empty() {
                edit.start // After delete, cursor is at start
            } else {
                edit.start + insert_len // After insert, cursor is at start + insert_len
            };
            let new_range = Selection::new(edit.id, simple_cursor_pos, simple_cursor_pos);

            self.push_history(
                &old_text,
                Selection::new(edit.id, edit.start, edit.end),
                new_range,
                new_text,
                operation_type,
                Some(operation_id),
            );

            // Calculate final cursor position for display
            // inserts_at_or_before = count of inserts at or before this edit's original position
            // deletions_before = sum of deletion lengths before this edit
            let inserts_at_or_before = num_edits - i; // Since sorted_edits is DESC
            let deletions_before: usize = sorted_edits[i + 1..]
                .iter()
                .filter(|e| e.start < edit.start)
                .map(|e| e.end - e.start)
                .sum();
            let final_pos = edit.start + inserts_at_or_before * insert_len - deletions_before;

            let mut new_sel = Selection::new(edit.id, final_pos, final_pos);
            new_sel.column_anchor = None;
            new_selections.push(new_sel);
        }

        // End grouping
        self.operation_id_counter += 1;
        self.history.end_grouping();

        // Sort new_selections by cursor_id to maintain order
        new_selections.sort_by_key(|s| s.id);

        // Single-line validation
        if self.mode.is_single_line() {
            let pending_text = self.text.to_string();
            if !self.is_valid_input(&pending_text, cx) {
                self.text = old_text;
                return;
            }

            if !self.mask_pattern.is_none() {
                let mask_text = self.mask_pattern.mask(&pending_text);
                self.text = Rope::from(mask_text.as_str());
            }
        }

        // Update selections
        self.selections.replace_all(new_selections);
        self.selections.merge_overlapping();

        // Update UI
        if let Some(diagnostics) = self.mode.diagnostics_mut() {
            diagnostics.reset(&self.text)
        }
        self.display_map
            .on_text_changed(&self.text, &(0..self.text.len()), &self.text, cx);
        self.mode.update_highlighter(None, &self.text, true, cx);
        self.update_fold_candidates();
        self.lsp.update(&self.text, window, cx);
        self.ime_marked_range.take();
        self.update_preferred_column();
        self.update_search(cx);
        self.mode.update_auto_grow(&self.display_map);
        if !self.silent_replace_text {
            let range = self.active_selection_range();
            self.handle_completion_trigger(&range, new_text, window, cx);
        }
        cx.emit(InputEvent::Change);
        cx.notify();
    }

    /// Mark text is the IME temporary insert on typing.
    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        self.lsp.reset();

        let selection = self.active_selection_range();
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.ime_marked_range.as_ref().map(|range| {
                let range = self.range_to_utf16(&(range.start..range.end));
                self.range_from_utf16(&range)
            }))
            .unwrap_or_else(|| selection);

        let old_text = self.text.clone();
        self.text.replace(range.clone(), new_text);

        if self.mode.is_single_line() {
            let pending_text = self.text.to_string();
            if !self.is_valid_input(&pending_text, cx) {
                self.text = old_text;
                return;
            }
        }

        if let Some(diagnostics) = self.mode.diagnostics_mut() {
            diagnostics.reset(&self.text)
        }
        self.display_map
            .adjust_folds_for_edit(&old_text, &range, new_text);
        self.display_map
            .on_text_changed(&self.text, &range, &Rope::from(new_text), cx);

        // Create highlighter edits
        let changed_len = new_text.len() as isize - range.len() as isize;
        let new_end = (range.end as isize + changed_len) as usize;
        let start_pos = self.text.offset_to_point(range.start);
        let old_end_pos = self.text.offset_to_point(range.end);
        let new_end_pos = self.text.offset_to_point(new_end);
        let edit = tree_sitter::InputEdit {
            start_byte: range.start,
            old_end_byte: range.end,
            new_end_byte: new_end,
            start_position: start_pos,
            old_end_position: old_end_pos,
            new_end_position: new_end_pos,
        };
        self.mode
            .update_highlighter(Some(&[edit]), &self.text, true, cx);
        self.update_fold_candidates_incremental(&range, new_text);
        self.lsp.update(&self.text, window, cx);
        if new_text.is_empty() {
            // Cancel selection, when cancel IME input.
            self.set_cursor_to(range.start);
            self.ime_marked_range = None;
        } else {
            self.ime_marked_range = Some(Selection::new(
                CursorId::default(),
                range.start,
                range.start + new_text.len(),
            ));
            let new_range = new_selected_range_utf16
                .as_ref()
                .map(|range_utf16| self.range_from_utf16(range_utf16))
                .map(|new_range| new_range.start + range.start..new_range.end + range.start)
                .unwrap_or_else(|| {
                    let new_offset = range.start + new_text.len();
                    new_offset..new_offset
                });
            self.set_selection(new_range.start, new_range.end);
        }
        self.mode.update_auto_grow(&self.display_map);
        self.history.start_grouping();
        // For IME input, new_selections is the same as old_selections since the selection
        // is set separately based on new_selected_range_utf16
        let operation_type = if !new_text.is_empty() {
            OperationType::Insert
        } else {
            OperationType::Delete
        };
        let new_range = range.start..range.start + new_text.len();
        self.push_history(
            &old_text,
            range.clone(),
            new_range,
            new_text,
            operation_type,
            None,
        );
        cx.notify();
    }

    /// Used to position IME candidates.
    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let line_height = last_layout.line_height;
        let line_number_width = last_layout.line_number_width;
        let range = self.range_from_utf16(&range_utf16);

        let mut start_origin = None;
        let mut end_origin = None;
        let line_number_origin = point(line_number_width, px(0.));
        let mut y_offset = last_layout.visible_top;
        let mut index_offset = last_layout.visible_range_offset.start;

        for line in last_layout.lines.iter() {
            if start_origin.is_some() && end_origin.is_some() {
                break;
            }

            if start_origin.is_none() {
                if let Some(p) =
                    line.position_for_index(range.start.saturating_sub(index_offset), last_layout)
                {
                    start_origin = Some(p + point(px(0.), y_offset));
                }
            }

            if end_origin.is_none() {
                if let Some(p) =
                    line.position_for_index(range.end.saturating_sub(index_offset), last_layout)
                {
                    end_origin = Some(p + point(px(0.), y_offset));
                }
            }

            index_offset += line.len() + 1;
            y_offset += line.size(line_height).height;
        }

        let start_origin = start_origin.unwrap_or_default();
        let mut end_origin = end_origin.unwrap_or_default();
        // Ensure at same line.
        end_origin.y = start_origin.y;

        Some(Bounds::from_corners(
            bounds.origin + line_number_origin + start_origin,
            // + line_height for show IME panel under the cursor line.
            bounds.origin + line_number_origin + point(end_origin.x, end_origin.y + line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let last_layout = self.last_layout.as_ref()?;
        let line_point = self.last_bounds?.localize(&point)?;
        let offset = last_layout.visible_range_offset.start;

        for line in last_layout.lines.iter() {
            if let Some(utf8_index) = line.index_for_position(line_point, last_layout) {
                return Some(self.offset_to_utf16(offset + utf8_index));
            }
        }

        None
    }
}

impl Focusable for InputState {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for InputState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self._pending_update {
            self.mode.update_highlighter(None, &self.text, false, cx);
            self.update_fold_candidates();
            self.lsp.update(&self.text, window, cx);
            self._pending_update = false;
        }

        div()
            .id("input-state")
            .flex_1()
            .when(self.mode.is_multi_line(), |this| this.h_full())
            .flex_grow()
            .overflow_x_hidden()
            .child(TextElement::new(cx.entity().clone()).placeholder(self.placeholder.clone()))
            .children(self.diagnostic_popover.clone())
            .children(self.context_menu.as_ref().map(|menu| menu.render()))
            .children(self.hover_popover.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::HistoryItem;
    use crate::theme::Theme;
    use AddCursorBelow;
    use gpui::{TestAppContext, VisualTestContext};

    /// Helper to create an InputState in a window for testing
    fn create_input_in_window(cx: &mut TestAppContext) -> gpui::WindowHandle<InputState> {
        cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                // Set up the theme first
                cx.set_global(Theme::default());
                // Initialize input keybindings
                init(cx);

                cx.new(|cx| InputState::new(window, cx))
            })
            .unwrap()
        })
    }

    /// Parse cursor specifications and return (text, cursor_offsets).
    /// Splits input by newlines and trims leading/trailing whitespace from each line.
    /// Uses `|` to mark cursor positions.
    fn parse_cursor_spec(input: &str) -> (String, Vec<usize>) {
        let mut full_text = String::new();
        let mut cursor_offsets = Vec::new();
        let mut non_empty_lines = Vec::new();

        // Collect non-empty lines
        for line in input.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                non_empty_lines.push(trimmed);
            }
        }

        for (line_idx, trimmed) in non_empty_lines.iter().enumerate() {
            let mut cursor_positions_in_line = Vec::new();
            let mut text_without_cursor = String::new();

            for (_byte_idx, ch) in trimmed.char_indices() {
                if ch == '|' {
                    cursor_positions_in_line.push(text_without_cursor.len());
                } else {
                    text_without_cursor.push(ch);
                }
            }

            if line_idx > 0 {
                full_text.push('\n');
            }
            let line_start = full_text.len();

            for pos in cursor_positions_in_line {
                cursor_offsets.push(line_start + pos);
            }

            full_text.push_str(&text_without_cursor);
        }
        full_text.push('\n');

        (full_text, cursor_offsets)
    }

    /// Set up input state with text and cursor positions from a visual representation.
    /// Use `|` to mark cursor positions. Lines are auto-trimmed.
    ///
    /// # Example
    /// ```ignore
    /// setup_cursors(cx, input, r#"
    ///     |hello
    ///     world|
    /// "#);
    /// ```
    fn setup_cursors(cx: &mut VisualTestContext, input: &Entity<InputState>, spec: &str) {
        let (full_text, cursor_offsets) = parse_cursor_spec(spec);

        cx.update(|_window, cx| {
            input.update(cx, |state, cx| {
                state.mode = state.mode.clone().multi_line(true);
                state.text = Rope::from_str(&full_text);
                state.display_map.set_text(&state.text, cx);
                state
                    .display_map
                    .on_text_changed(&state.text, &(0..state.text.len()), &state.text, cx);
                state.last_layout = None;

                let mut selections = Vec::new();
                for offset in cursor_offsets {
                    let id = state.selections.generate_id();
                    selections.push(Selection::new(id, offset, offset));
                }
                state.selections.replace_all(selections);
                cx.notify();
            });
        });
    }

    /// Assert the current state matches expected text and cursor positions.
    /// Use `|` to mark expected cursor positions. Lines are auto-trimmed.
    ///
    /// # Panics
    /// Panics if text or cursor positions do not match expected values.
    ///
    /// # Example
    /// ```ignore
    /// assert_cursors(cx, input, r#"
    ///     |hello
    ///     world|
    /// "#);
    /// ```
    #[track_caller]
    fn assert_cursors(cx: &mut VisualTestContext, input: &Entity<InputState>, spec: &str) {
        let (expected_text, expected_cursor_offsets) = parse_cursor_spec(spec);

        let actual_text = input.read_with(&*cx, |state, _| state.text.to_string());
        assert_eq!(
            actual_text, expected_text,
            "Text mismatch:\nExpected: {:?}\nActual: {:?}",
            expected_text, actual_text
        );

        let mut actual_cursors: Vec<usize> = input.read_with(&*cx, |state, _| {
            state.selections.iter().map(|s| s.cursor_offset()).collect()
        });
        actual_cursors.sort();

        let mut expected_cursors = expected_cursor_offsets;
        expected_cursors.sort();

        assert_eq!(
            actual_cursors, expected_cursors,
            "Cursor positions mismatch:\nExpected: {:?}\nActual: {:?}",
            expected_cursors, actual_cursors
        );
    }

    #[gpui::test]
    fn test_multi_cursor_insert_text(cx: &mut TestAppContext) {
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        setup_cursors(&mut cx, &input, "|hello |world|");

        // Insert ">>>" at all cursors
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.replace_text_in_range(None, ">>>", window, cx);
            });
        });

        assert_cursors(&mut cx, &input, ">>>|hello >>>|world>>>|");
    }

    #[gpui::test]
    fn test_multi_cursor_delete_backward(cx: &mut TestAppContext) {
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        setup_cursors(&mut cx, &input, "|hello| world|");

        // Delete backwards at all cursors
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.backspace(&Backspace, window, cx);
            });
        });

        // 1) nothing to delete
        // 2) deletes 'o'
        // 3) 'd'
        assert_cursors(&mut cx, &input, "|hell| worl|");
    }

    #[gpui::test]
    fn test_multi_cursor_delete_forward(cx: &mut TestAppContext) {
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        setup_cursors(&mut cx, &input, "hello| |world");

        // Delete forward at both cursors
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.delete(&Delete, window, cx);
            });
        });

        // Overlapping cursors should merge into
        assert_cursors(&mut cx, &input, "hello|orld");
    }

    #[gpui::test]
    fn test_multi_cursor_multiline_insert_and_delete(cx: &mut TestAppContext) {
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        setup_cursors(
            &mut cx,
            &input,
            r#"
            |1
            |2
            |3
        "#,
        );

        // Insert "a" at all cursors
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.replace_text_in_range(None, "a", window, cx);
            });
        });

        // Verify history after insert:
        // Should have 3 Changes (one per cursor), each with cursor_id, old_text="", new_text="a"
        cx.update(|_window, cx| {
            input.update(cx, |state, _cx| {
                let undos = state.history.undos();
                // The history stores individual changes, grouped by version
                assert_eq!(
                    undos.len(),
                    3,
                    "Should have 3 changes after insert (one per cursor)"
                );

                // All changes should have the same version (grouped together)
                let version = undos[0].version();
                for change in undos {
                    assert_eq!(
                        change.version(),
                        version,
                        "All changes should have the same version"
                    );
                    assert_eq!(change.old_text, "", "Old text should be empty for insert");
                    assert_eq!(change.new_text, "a", "New text should be 'a'");
                    // old_range.start == old_range.end (collapsed cursor before insert)
                    assert!(
                        change.old_range.is_collapsed(),
                        "Old range should be collapsed"
                    );
                    // new_range is also collapsed (simple cursor position after insert)
                    // For undo ASC: we delete from old_range.start for new_text.len()
                    assert!(
                        change.new_range.is_collapsed(),
                        "new_range should be collapsed (simple cursor position)"
                    );
                }
            });
        });

        // Now delete backwards (backspace) at all cursors
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.backspace(&Backspace, window, cx);
            });
        });

        // Verify history after delete:
        // Should have 6 changes now (3 for insert + 3 for delete)
        cx.update(|_window, cx| {
            input.update(cx, |state, _cx| {
                let undos = state.history.undos();
                // Print all changes for debugging
                for (i, change) in undos.iter().enumerate() {
                    eprintln!(
                        "Change {}: old_range={:?} old_text={:?} new_range={:?} new_text={:?} version={}",
                        i, change.old_range, change.old_text, change.new_range, change.new_text, change.version()
                    );
                }
                assert_eq!(undos.len(), 6, "Should have 6 changes total (3 insert + 3 delete)");

                // First 3 are insert changes (old_text="")
                let insert_changes = &undos[0..3];
                for change in insert_changes {
                    assert_eq!(change.old_text, "", "Insert: old_text should be empty");
                    assert_eq!(change.new_text, "a", "Insert: new_text should be 'a'");
                }

                // Last 3 are delete changes (new_text="")
                let delete_changes = &undos[3..6];
                for change in delete_changes {
                    assert_eq!(change.old_text, "a", "Delete: old_text should be 'a'");
                    assert_eq!(change.new_text, "", "Delete: new_text should be empty");
                }
            });
        });

        // Expected: back to original, cursors before each number
        assert_cursors(
            &mut cx,
            &input,
            r#"
            |1
            |2
            |3
        "#,
        );
    }

    #[gpui::test]
    fn test_add_cursor_below_preserves_column(cx: &mut TestAppContext) {
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        // Set up text with cursor in middle of first row
        setup_cursors(
            &mut cx,
            &input,
            r#"
            ab|cd
            abcd
        "#,
        );

        // Add cursor below
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.add_cursor_below(&AddCursorBelow, window, cx);
            });
        });

        // Both cursors should be at position 2 (middle of their respective lines)
        assert_cursors(
            &mut cx,
            &input,
            r#"
            ab|cd
            ab|cd
        "#,
        );
    }

    #[gpui::test]
    fn test_multi_cursor_undo_redo_restores_selections(cx: &mut TestAppContext) {
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        setup_cursors(
            &mut cx,
            &input,
            r#"
            |1
            |2
            |3
        "#,
        );

        // Insert "a" at all cursors
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.replace_text_in_range(None, "a", window, cx);
            });
        });

        // Log history after insert
        cx.update(|_window, cx| {
            input.update(cx, |state, _cx| {
                eprintln!("=== History after insert ===");
                for (i, change) in state.history.undos().iter().enumerate() {
                    eprintln!(
                        "Change {}: cursor_id={:?} old_range={:?} new_range={:?} old_text={:?} new_text={:?}",
                        i, change.old_range.id, change.old_range.start..change.old_range.end,
                        change.new_range.start..change.new_range.end,
                        change.old_text, change.new_text
                    );
                }
                eprintln!("Current text: {:?}", state.text.to_string());
                eprintln!("Current selections: {:?}", state.selections.iter().map(|s| s.start).collect::<Vec<_>>());
            });
        });

        // Check text after insert
        assert_cursors(
            &mut cx,
            &input,
            r#"
            a|1
            a|2
            a|3
        "#,
        );

        // Undo
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.undo(&Undo, window, cx);
            });
        });

        // Check cursor positions are restored after undo
        assert_cursors(
            &mut cx,
            &input,
            r#"
            |1
            |2
            |3
        "#,
        );

        // Redo
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.redo(&Redo, window, cx);
            });
        });

        // Check cursor positions are restored after redo
        assert_cursors(
            &mut cx,
            &input,
            r#"
            a|1
            a|2
            a|3
        "#,
        );
    }

    #[gpui::test]
    fn test_multi_cursor_undo_redo_different_line_lengths(cx: &mut TestAppContext) {
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        // Set up text with cursors at end of each line
        // Lines have different lengths: abc123, abc12345, abc1234567
        setup_cursors(
            &mut cx,
            &input,
            r#"
            abc123|
            abc12345|
            abc1234567|
        "#,
        );

        // Insert "a" at all cursors
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.replace_text_in_range(None, "a", window, cx);
            });
        });

        // Check text after insert
        assert_cursors(
            &mut cx,
            &input,
            r#"
            abc123a|
            abc12345a|
            abc1234567a|
        "#,
        );

        // Undo
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.undo(&Undo, window, cx);
            });
        });

        // Check cursor positions are restored after undo
        assert_cursors(
            &mut cx,
            &input,
            r#"
            abc123|
            abc12345|
            abc1234567|
        "#,
        );

        // Redo
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.redo(&Redo, window, cx);
            });
        });

        // Check cursor positions are restored after redo
        assert_cursors(
            &mut cx,
            &input,
            r#"
            abc123a|
            abc12345a|
            abc1234567a|
        "#,
        );
    }

    #[gpui::test]
    fn test_multi_cursor_undo_multiple_inserts(cx: &mut TestAppContext) {
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        // Set up multi-cursor
        setup_cursors(
            &mut cx,
            &input,
            r#"
            |1
            |2
            |3
        "#,
        );

        // Type multiple characters (simulating typing "abc")
        for ch in ['a', 'b', 'c'] {
            cx.update(|window, cx| {
                input.update(cx, |state, cx| {
                    state.replace_text_in_range(None, &ch.to_string(), window, cx);
                });
            });
        }

        // Check final state
        assert_cursors(
            &mut cx,
            &input,
            r#"
            abc|1
            abc|2
            abc|3
        "#,
        );

        // Undo once - all operations are grouped by time-based grouping (1 second interval)
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.undo(&Undo, window, cx);
            });
        });

        // After undoing, should have original cursors restored
        assert_cursors(
            &mut cx,
            &input,
            r#"
            |1
            |2
            |3
        "#,
        );

        // Redo once
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.redo(&Redo, window, cx);
            });
        });

        assert_cursors(
            &mut cx,
            &input,
            r#"
            abc|1
            abc|2
            abc|3
        "#,
        );
    }

    #[gpui::test]
    fn test_multi_cursor_indent_outdent(cx: &mut TestAppContext) {
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        setup_cursors(
            &mut cx,
            &input,
            r#"
            1|2
            1|2
        "#,
        );

        // Indent at all cursors (should insert spaces at cursor position, use block=false for inline indent)
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.indent(false, window, cx);
            });
        });

        // Both lines should have spaces inserted at cursor position
        assert_cursors(
            &mut cx,
            &input,
            r#"
            1  |2
            1  |2
        "#,
        );

        // Outdent to return to original
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.outdent(false, window, cx);
            });
        });

        assert_cursors(
            &mut cx,
            &input,
            r#"
            1|2
            1|2
        "#,
        );
    }

    #[gpui::test]
    fn test_block_indent_outdent_with_selection(cx: &mut TestAppContext) {
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        // Set up a selection that spans multiple lines
        cx.update(|_, cx| {
            input.update(cx, |state, cx| {
                state.mode = state.mode.clone().multi_line(true);
                state.text = Rope::from_str("line1\nline2\nline3");
                state.display_map.set_text(&state.text, cx);
                state
                    .display_map
                    .on_text_changed(&state.text, &(0..state.text.len()), &state.text, cx);
                state.last_layout = None;

                // Create a selection from start of line 1 to end of line 3
                let id = state.selections.generate_id();
                let sel = Selection::new(id, 0, 17); // "line1\nline2\nline3" is 17 chars
                state.selections.replace_all(vec![sel]);
                cx.notify();
            });
        });

        // Verify initial selection spans all lines
        cx.update(|_window, cx| {
            input.update(cx, |state, _| {
                let sel = state.selections.active();
                assert_eq!(sel.start, 0);
                assert_eq!(sel.end, 17);
            });
        });

        // Block indent
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.indent(true, window, cx);
            });
        });

        // All three lines should be indented
        cx.update(|_window, cx| {
            input.read_with(cx, |state, _| {
                assert_eq!(state.text.to_string(), "  line1\n  line2\n  line3");
            });
        });

        // Block outdent
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.outdent(true, window, cx);
            });
        });

        // Should be back to original
        cx.update(|_window, cx| {
            input.read_with(cx, |state, _| {
                assert_eq!(state.text.to_string(), "line1\nline2\nline3");
            });
        });
    }

    #[gpui::test]
    fn test_redo_clears_after_new_edit(cx: &mut TestAppContext) {
        // Test the scenario: write -> undo -> write -> undo -> redo
        // Redo should only redo the most recent edit, not all previous edits
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        setup_cursors(
            &mut cx,
            &input,
            r#"
            abc|
            abc|
            abc|
        "#,
        );

        // First edit: insert "1"
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.replace_text_in_range(None, "1", window, cx);
            });
        });
        assert_cursors(
            &mut cx,
            &input,
            r#"
            abc1|
            abc1|
            abc1|
        "#,
        );

        // Undo first edit
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.undo(&Undo, window, cx);
            });
        });
        assert_cursors(
            &mut cx,
            &input,
            r#"
            abc|
            abc|
            abc|
        "#,
        );

        // Second edit: insert "2" (this should clear the redo stack)
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.replace_text_in_range(None, "2", window, cx);
            });
        });
        assert_cursors(
            &mut cx,
            &input,
            r#"
            abc2|
            abc2|
            abc2|
        "#,
        );

        // Undo second edit
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.undo(&Undo, window, cx);
            });
        });
        assert_cursors(
            &mut cx,
            &input,
            r#"
            abc|
            abc|
            abc|
        "#,
        );

        // Redo should only redo the second edit ("2"), not the first edit ("1")
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.redo(&Redo, window, cx);
            });
        });
        assert_cursors(
            &mut cx,
            &input,
            r#"
            abc2|
            abc2|
            abc2|
        "#,
        );

        // Another redo should do nothing (no more in redo stack)
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.redo(&Redo, window, cx);
            });
        });
        // Should still be the same
        assert_cursors(
            &mut cx,
            &input,
            r#"
            abc2|
            abc2|
            abc2|
        "#,
        );
    }

    #[gpui::test]
    fn test_undo_separate_groups_after_wait(cx: &mut TestAppContext) {
        // Test that edits separated by time are in separate undo groups
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        setup_cursors(
            &mut cx,
            &input,
            r#"
            abc|
            abc|
            abc|
        "#,
        );

        // First edit: insert "1"
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.replace_text_in_range(None, "1", window, cx);
            });
        });
        assert_cursors(
            &mut cx,
            &input,
            r#"
            abc1|
            abc1|
            abc1|
        "#,
        );

        // Simulate waiting by moving last_changed_at back in time (more than group_interval)
        cx.update(|_, cx| {
            input.update(cx, |state, _| {
                state
                    .history
                    .simulate_time_passed(std::time::Duration::from_secs(2));
            });
        });

        // Second edit: insert "2"
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.replace_text_in_range(None, "2", window, cx);
            });
        });
        assert_cursors(
            &mut cx,
            &input,
            r#"
            abc12|
            abc12|
            abc12|
        "#,
        );

        // Undo should only undo the second edit ("2")
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.undo(&Undo, window, cx);
            });
        });
        assert_cursors(
            &mut cx,
            &input,
            r#"
            abc1|
            abc1|
            abc1|
        "#,
        );

        // Undo again should undo the first edit ("1")
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.undo(&Undo, window, cx);
            });
        });
        assert_cursors(
            &mut cx,
            &input,
            r#"
            abc|
            abc|
            abc|
        "#,
        );
    }

    #[gpui::test]
    fn test_insert_delete_separate_undo_groups(cx: &mut TestAppContext) {
        // Test that insert and delete operations create separate undo groups,
        // and redo only restores the last operation group (not all previous)
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        // Start with empty input
        setup_cursors(&mut cx, &input, "|");

        // 1. Write "123" (INSERT operation, version 1)
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.replace_text_in_range(None, "123", window, cx);
            });
        });
        assert_cursors(&mut cx, &input, "123|");

        // 2. Write "?" (INSERT operation, version 1, same operation type, grouped by time)
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.replace_text_in_range(None, "?", window, cx);
            });
        });
        assert_cursors(&mut cx, &input, "123?|");

        // 3. Delete backwards (DELETE operation, version 2, different operation type, new version)
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.backspace(&Backspace, window, cx);
            });
        });
        assert_cursors(&mut cx, &input, "123|");

        // 4. Write "?" (INSERT operation, version 3, different operation type, new version)
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.replace_text_in_range(None, "?", window, cx);
            });
        });
        assert_cursors(&mut cx, &input, "123?|");

        // 5. Delete backwards (DELETE operation, version 4, different operation type, new version)
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.backspace(&Backspace, window, cx);
            });
        });
        assert_cursors(&mut cx, &input, "123|");

        // 6. Write "?" (INSERT operation, version 5, different operation type, new version)
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.replace_text_in_range(None, "?", window, cx);
            });
        });
        assert_cursors(&mut cx, &input, "123?|");

        // 7. Undo, should undo the last INSERT (version 5), giving us "123"
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.undo(&Undo, window, cx);
            });
        });
        assert_cursors(&mut cx, &input, "123|");

        // 8. Redo, should restore the last INSERT, giving us "123?" (not "123???")
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.redo(&Redo, window, cx);
            });
        });
        assert_cursors(&mut cx, &input, "123?|");
    }

    #[gpui::test]
    fn test_multi_cursor_word_movement(cx: &mut TestAppContext) {
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        // Set up multi-line input with multi-cursors
        cx.update(|_, cx| {
            input.update(cx, |state, _| {
                state.mode = state.mode.clone().multi_line(true);
            });
        });

        setup_cursors(
            &mut cx,
            &input,
            r#"
            on|e two three
            one t|wo three
            on|e two three
        "#,
        );

        // 1) Move to next word. Each cursor should move to start of next word
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.move_to_next_word(&MoveToNextWord, window, cx);
            });
        });

        assert_cursors(
            &mut cx,
            &input,
            r#"
            one |two three
            one two |three
            one |two three
        "#,
        );

        // 2) Move to previous word. Each cursor should move to start of previous word
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.move_to_previous_word(&MoveToPreviousWord, window, cx);
            });
        });

        assert_cursors(
            &mut cx,
            &input,
            r#"
            |one two three
            one |two three
            |one two three
        "#,
        );

        // 3) Move to end. All cursors should move to end of document
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.move_to_end(&MoveToEnd, window, cx);
            });
        });

        // All cursors at end of document
        {
            let mut actual_cursors: Vec<usize> = input.read_with(&cx, |state, _| {
                state.selections.iter().map(|s| s.cursor_offset()).collect()
            });
            actual_cursors.sort();
            assert_eq!(
                actual_cursors,
                vec![42, 42, 42],
                "All cursors should be at end of document (position 42)"
            );
        }

        // 4) Move to start. All cursors should move to position 0 (start of document)
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.move_to_start(&MoveToStart, window, cx);
            });
        });

        // All cursors at start of document (all at position 0)
        {
            let mut actual_cursors: Vec<usize> = input.read_with(&cx, |state, _| {
                state.selections.iter().map(|s| s.cursor_offset()).collect()
            });
            actual_cursors.sort();
            assert_eq!(
                actual_cursors,
                vec![0, 0, 0],
                "All cursors should be at start of document (position 0)"
            );
        }
    }

    #[gpui::test]
    fn test_multi_cursor_selection_commands(cx: &mut TestAppContext) {
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        // Set up multi-line input with multi-cursors
        cx.update(|_, cx| {
            input.update(cx, |state, _cx| {
                state.mode = state.mode.clone().multi_line(true);
            });
        });

        // Initial state with 3 cursors in the middle of each line
        setup_cursors(
            &mut cx,
            &input,
            r#"
            on|e two three
            one t|wo three
            on|e two three
        "#,
        );

        // 1) Select to start of line. Each cursor should select to its line start
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.select_to_start_of_line(&SelectToStartOfLine, window, cx);
            });
        });
        // Verify: each selection extends from cursor to line start
        assert_cursors(
            &mut cx,
            &input,
            r#"
            |one two three
            |one two three
            |one two three
        "#,
        );

        // Cancel selections for next test
        setup_cursors(
            &mut cx,
            &input,
            r#"
            on|e two three
            one t|wo three
            on|e two three
        "#,
        );

        // 2) Select to end of line. Each cursor should select to its line end
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.select_to_end_of_line(&SelectToEndOfLine, window, cx);
            });
        });
        // Verify: each selection extends from cursor to line end (before \n)
        assert_cursors(
            &mut cx,
            &input,
            r#"
            one two thre|e
            one two thre|e
            one two thre|e
        "#,
        );

        // Cancel selections for next test
        setup_cursors(
            &mut cx,
            &input,
            r#"
            on|e two three
            one t|wo three
            on|e two three
        "#,
        );

        // 3) Select to start (document). Only active cursor remains, selecting to position 0
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.select_to_start(&SelectToStart, window, cx);
            });
        });
        // Verify: only the active cursor remains, with selection from 0 to original position
        {
            let cursors: Vec<usize> = input.read_with(&cx, |state, _| {
                state.selections.iter().map(|s| s.cursor_offset()).collect()
            });
            assert_eq!(
                cursors,
                vec![0],
                "Only active cursor should remain at document start after selecting to start"
            );
        }

        // Cancel selections for next test
        setup_cursors(
            &mut cx,
            &input,
            r#"
            on|e two three
            one t|wo three
            on|e two three
        "#,
        );

        // 4) Select to end (document). Only active cursor remains, selecting to document end
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.select_to_end(&SelectToEnd, window, cx);
            });
        });
        // Verify: only the active cursor remains, with selection from original position to document end (42)
        {
            let selections: Vec<(usize, usize)> = input.read_with(&cx, |state, _| {
                state.selections.iter().map(|s| (s.start, s.end)).collect()
            });
            // Only one selection should remain, ending at position 42 (document end)
            assert_eq!(
                selections.len(),
                1,
                "Only active selection should remain after selecting to end"
            );
            assert_eq!(
                selections[0].1, 42,
                "Selection should end at document end (42)"
            );
            // The first cursor was at position 2 ("on|e two three")
            assert_eq!(
                selections[0].0, 2,
                "Selection should start at first cursor's original position (2)"
            );
        }
    }

    #[gpui::test]
    fn test_select_left_extends_selection(cx: &mut TestAppContext) {
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        // Start with a single cursor at position 5 (after "hello|")
        setup_cursors(&mut cx, &input, "hello| world");

        // Press shift+left to select "o"
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.select_left(&SelectLeft, window, cx);
            });
        });

        // Expected: "hell|o| world". Selection from 4 to 5 (reversed, cursor at 4)
        let selection = input.read_with(&cx, |state, _| {
            state.selections.iter().next().unwrap().clone()
        });
        assert_eq!(
            (selection.start, selection.end, selection.reversed),
            (4, 5, true),
            "After first shift+left: should select 'o' (4-5, reversed)"
        );

        // Press shift+left again to extend to "l"
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.select_left(&SelectLeft, window, cx);
            });
        });

        // Expected: "hel|lo| world". Selection from 3 to 5 (reversed, cursor at 3)
        let selection = input.read_with(&cx, |state, _| {
            state.selections.iter().next().unwrap().clone()
        });
        assert_eq!(
            (selection.start, selection.end, selection.reversed),
            (3, 5, true),
            "After second shift+left: should select 'lo' (3-5, reversed)"
        );

        // Press shift+left again to extend to "e"
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.select_left(&SelectLeft, window, cx);
            });
        });

        // Expected: "he|llo| world". Selection from 2 to 5 (reversed, cursor at 2)
        let selection = input.read_with(&cx, |state, _| {
            state.selections.iter().next().unwrap().clone()
        });
        assert_eq!(
            (selection.start, selection.end, selection.reversed),
            (2, 5, true),
            "After third shift+left: should select 'llo' (2-5, reversed)"
        );
    }

    #[gpui::test]
    fn test_select_right_extends_selection(cx: &mut TestAppContext) {
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        // Start with a single cursor at position 2 (after "he|llo world")
        setup_cursors(&mut cx, &input, "he|llo world");

        // Press shift+right to select "l"
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.select_right(&SelectRight, window, cx);
            });
        });

        // Expected: "he|l|lo world". Selection from 2 to 3 (normal, cursor at 3)
        let selection = input.read_with(&cx, |state, _| {
            state.selections.iter().next().unwrap().clone()
        });
        assert_eq!(
            (selection.start, selection.end, selection.reversed),
            (2, 3, false),
            "After first shift+right: should select 'l' (2-3, normal)"
        );

        // Press shift+right again to extend to "ll"
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.select_right(&SelectRight, window, cx);
            });
        });

        // Expected: "he|ll|o world". Selection from 2 to 4 (normal, cursor at 4)
        let selection = input.read_with(&cx, |state, _| {
            state.selections.iter().next().unwrap().clone()
        });
        assert_eq!(
            (selection.start, selection.end, selection.reversed),
            (2, 4, false),
            "After second shift+right: should select 'll' (2-4, normal)"
        );

        // Press shift+right again to extend to "llo"
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.select_right(&SelectRight, window, cx);
            });
        });

        // Expected: "he|llo| world". Selection from 2 to 5 (normal, cursor at 5)
        let selection = input.read_with(&cx, |state, _| {
            state.selections.iter().next().unwrap().clone()
        });
        assert_eq!(
            (selection.start, selection.end, selection.reversed),
            (2, 5, false),
            "After third shift+right: should select 'llo' (2-5, normal)"
        );
    }

    #[gpui::test]
    fn test_select_and_type_replaces_selection(cx: &mut TestAppContext) {
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        // Start with a single cursor at position 2 (after "he|llo world")
        setup_cursors(&mut cx, &input, "he|llo world");

        // Press shift+right twice to select "ll"
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.select_right(&SelectRight, window, cx);
                state.select_right(&SelectRight, window, cx);
            });
        });

        // Verify selection is from 2 to 4
        let selection = input.read_with(&cx, |state, _| {
            state.selections.iter().next().unwrap().clone()
        });
        assert_eq!(
            (selection.start, selection.end),
            (2, 4),
            "After two shift+right: should select 'll' (2-4)"
        );

        // Type "1" to replace the selection
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.replace_text_in_range(None, "1", window, cx);
            });
        });

        // Expected: "he1o world"
        assert_cursors(&mut cx, &input, "he1|o world");

        // Undo should restore original text
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.undo(&Undo, window, cx);
            });
        });

        assert_cursors(&mut cx, &input, "hell|o world");
    }

    #[gpui::test]
    fn test_multi_cursor_replace_selection(cx: &mut TestAppContext) {
        // Test: multi-cursor with selections, then insert to replace
        let window = create_input_in_window(cx);
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input = window.root(&mut cx).unwrap();

        // Set up cursors at each character
        setup_cursors(
            &mut cx,
            &input,
            r#"
            |a
            |b
            |c
        "#,
        );

        // Expand selection right to select each character
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.select_right(&SelectRight, window, cx);
            });
        });

        // Verify selections: [a], [b], [c] are selected
        cx.update(|_window, cx| {
            input.update(cx, |state, _| {
                let selections: Vec<_> =
                    state.selections.iter().map(|s| (s.start, s.end)).collect();
                assert_eq!(
                    selections,
                    vec![(0, 1), (2, 3), (4, 5)],
                    "Each character should be selected"
                );
            });
        });

        // Insert "x" to replace all selections
        cx.update(|window, cx| {
            input.update(cx, |state, cx| {
                state.replace_text_in_range(None, "x", window, cx);
            });
        });

        // Expected: x\nx\nx
        assert_cursors(
            &mut cx,
            &input,
            r#"
            x|
            x|
            x|
        "#,
        );
    }
}
