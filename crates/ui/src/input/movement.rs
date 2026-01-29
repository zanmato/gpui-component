use gpui::{Context, Window};

use crate::input::selection::TextSelector;
use crate::input::{
    AddCursorAbove, AddCursorBelow, InputState, MoveDown, MoveEnd, MoveHome, MoveLeft,
    MovePageDown, MovePageUp, MoveRight, MoveToEnd, MoveToNextWord, MoveToPreviousWord,
    MoveToStart, MoveUp, RopeExt as _, Selection,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoveDirection {
    Left,
    Right,
    Up,
    Down,
}

/// Vertical direction for cursor operations (above/below).
#[derive(Clone, Copy, PartialEq, Eq)]
enum VerticalDirection {
    Above,
    Below,
}

/// Direction for collapsing selections before vertical movement.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CollapseDirection {
    /// Collapse to start of selection (for moving up)
    ToStart,
    /// Collapse to end of selection (for moving down)
    ToEnd,
}

impl InputState {
    /// Called after moving the cursor. Updates preferred_column if we know where the cursor now is.
    pub(super) fn update_preferred_column(&mut self) {
        let Some(last_layout) = &self.last_layout else {
            self.preferred_column = None;
            return;
        };

        let point = self.text.offset_to_point(self.cursor());
        let row = point.row.saturating_sub(last_layout.visible_range.start);
        let Some(line) = last_layout.lines.get(row) else {
            self.preferred_column = None;
            return;
        };

        let Some(pos) = line.position_for_index(point.column, last_layout) else {
            self.preferred_column = None;
            return;
        };

        self.preferred_column = Some((pos.x, point.column));
    }

    /// Move all cursors based on the given direction.
    ///
    /// For single-cursor movements to a specific offset, use `move_to_offset` instead.
    pub(crate) fn move_to(
        &mut self,
        direction: MoveDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match direction {
            MoveDirection::Left => {
                let new_selections: Vec<Selection> = self
                    .selections
                    .iter()
                    .map(|sel| {
                        let new_offset = if sel.is_collapsed() {
                            self.previous_boundary(sel.cursor_offset())
                        } else {
                            sel.start
                        };
                        let mut new_sel = sel.clone();
                        new_sel.place_at(new_offset, sel.column_anchor);
                        new_sel
                    })
                    .collect();

                self.selections.replace_all(new_selections);
                self.finish_move(self.cursor(), Some(MoveDirection::Left), cx);
            }
            MoveDirection::Right => {
                let new_selections: Vec<Selection> = self
                    .selections
                    .iter()
                    .map(|sel| {
                        let new_offset = if sel.is_collapsed() {
                            self.next_boundary(sel.cursor_offset())
                        } else {
                            sel.end
                        };
                        let mut new_sel = sel.clone();
                        new_sel.place_at(new_offset, sel.column_anchor);
                        new_sel
                    })
                    .collect();

                self.selections.replace_all(new_selections);
                self.finish_move(self.cursor(), Some(MoveDirection::Right), cx);
            }
            MoveDirection::Up => {
                if self.mode.is_single_line() {
                    return;
                }
                self.move_vertical(-1, Some(CollapseDirection::ToStart), window, cx);
            }
            MoveDirection::Down => {
                if self.mode.is_single_line() {
                    return;
                }
                self.move_vertical(1, Some(CollapseDirection::ToEnd), window, cx);
            }
        }
    }

    /// Move the cursor to the given offset (for single-cursor operations).
    ///
    /// The offset is the UTF-8 offset.
    ///
    /// Ensure the offset use self.next_boundary or self.previous_boundary to get the correct offset.
    pub(crate) fn move_to_offset(
        &mut self,
        offset: usize,
        direction: Option<MoveDirection>,
        cx: &mut Context<Self>,
    ) {
        // Clear all cursors except the active one when moving to a specific offset
        self.selections.remove_all_but_active();
        let offset = offset.clamp(0, self.text.len());
        self.set_cursor_to(offset);
        self.finish_move(offset, direction, cx);
    }

    /// Common post-move operations: scroll, pause blink, update preferred column, etc.
    fn finish_move(
        &mut self,
        offset: usize,
        direction: Option<MoveDirection>,
        cx: &mut Context<Self>,
    ) {
        self.scroll_to(offset, direction, cx);
        self.pause_blink_cursor(cx);
        self.update_preferred_column();
        self.hide_context_menu(cx);
        self.clear_inline_completion(cx);
        cx.notify();
    }

    /// Move all cursors vertically by the given number of lines while preserving column if possible.
    ///
    /// move_lines: Number of lines to move vertically (positive for down, negative for up).
    /// collapse: Optional direction to collapse selections before moving (ToStart for up, ToEnd for down).
    pub(super) fn move_vertical(
        &mut self,
        move_lines: isize,
        collapse: Option<CollapseDirection>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mode.is_single_line() {
            return;
        }

        self.pause_blink_cursor(cx);

        let text = &self.text;
        let line_count = text.lines_len();

        let new_selections: Vec<Selection> = self
            .selections
            .iter()
            .filter_map(|sel| {
                // Handle collapse if needed
                let (effective_offset, column_anchor) = match collapse {
                    Some(CollapseDirection::ToStart) if !sel.is_collapsed() => (
                        self.previous_boundary(sel.start.saturating_sub(1)),
                        sel.column_anchor,
                    ),
                    Some(CollapseDirection::ToEnd) if !sel.is_collapsed() => (
                        self.next_boundary(sel.end.saturating_sub(1)),
                        sel.column_anchor,
                    ),
                    _ => (sel.cursor_offset(), sel.column_anchor),
                };

                let cursor_point = text.offset_to_point(effective_offset);

                // Use the column from column_anchor if available, otherwise use current column
                let preferred_column = column_anchor.unwrap_or(cursor_point.column);

                // Convert buffer row to display row (skipping folded), move, convert back
                let display_row = self
                    .display_map
                    .buffer_line_to_display_row_range(cursor_point.row)
                    .map(|r| r.start)
                    .unwrap_or(0);
                let max_display_row =
                    self.display_map.display_row_count().saturating_sub(1);
                let target_display_row = if move_lines < 0 {
                    display_row.saturating_sub(move_lines.unsigned_abs())
                } else {
                    (display_row + move_lines as usize).min(max_display_row)
                };
                let target_row =
                    self.display_map.display_row_to_buffer_line(target_display_row);

                // Clamp to valid row range
                if target_row >= line_count {
                    return None;
                }

                // Get the target column, clamping to line length
                let line = text.slice_line(target_row);
                let target_column = preferred_column.min(line.len());

                let line_start = text.line_start_offset(target_row);
                let new_offset = line_start + target_column;

                let mut new_sel = sel.clone();
                new_sel.place_at(new_offset, Some(preferred_column));
                Some(new_sel)
            })
            .collect();

        self.selections.replace_all(new_selections);

        // Update preferred_column from active cursor
        self.update_preferred_column();
        self.hide_context_menu(cx);
        self.clear_inline_completion(cx);
        cx.notify();
    }

    pub(super) fn left(&mut self, _: &MoveLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(MoveDirection::Left, window, cx);
    }

    pub(super) fn right(&mut self, _: &MoveRight, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(MoveDirection::Right, window, cx);
    }

    pub(super) fn up(&mut self, action: &MoveUp, window: &mut Window, cx: &mut Context<Self>) {
        if self.handle_action_for_context_menu(Box::new(action.clone()), window, cx) {
            return;
        }
        self.move_vertical(-1, Some(CollapseDirection::ToStart), window, cx);
    }

    pub(super) fn down(&mut self, action: &MoveDown, window: &mut Window, cx: &mut Context<Self>) {
        if self.handle_action_for_context_menu(Box::new(action.clone()), window, cx) {
            return;
        }
        self.move_vertical(1, Some(CollapseDirection::ToEnd), window, cx);
    }

    pub(super) fn page_up(&mut self, _: &MovePageUp, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode.is_single_line() {
            return;
        }

        let Some(last_layout) = &self.last_layout else {
            return;
        };

        let display_lines = (self.input_bounds.size.height / last_layout.line_height) as isize;
        self.move_vertical(-display_lines, None, window, cx);
    }

    pub(super) fn page_down(
        &mut self,
        _: &MovePageDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mode.is_single_line() {
            return;
        }

        let Some(last_layout) = &self.last_layout else {
            return;
        };

        let display_lines = (self.input_bounds.size.height / last_layout.line_height) as isize;
        self.move_vertical(display_lines, None, window, cx);
    }

    pub(super) fn home(&mut self, _: &MoveHome, _: &mut Window, cx: &mut Context<Self>) {
        let new_selections: Vec<Selection> = self
            .selections
            .iter()
            .map(|sel| {
                let line_start = self.start_of_line_at(sel.cursor_offset());
                let mut new_sel = sel.clone();
                new_sel.place_at(line_start, Some(0));
                new_sel
            })
            .collect();

        self.selections.replace_all(new_selections);
        self.finish_move(self.cursor(), None, cx);
    }

    pub(super) fn end(&mut self, _: &MoveEnd, _: &mut Window, cx: &mut Context<Self>) {
        let new_selections: Vec<Selection> = self
            .selections
            .iter()
            .map(|sel| {
                let cursor_offset = sel.cursor_offset();
                let cursor_point = self.text.offset_to_point(cursor_offset);
                let line_end = self.end_of_line_at(cursor_offset);
                let column = cursor_point.column;
                let mut new_sel = sel.clone();
                new_sel.place_at(line_end, Some(column));
                new_sel
            })
            .collect();

        self.selections.replace_all(new_selections);
        self.finish_move(self.cursor(), None, cx);
    }

    pub(super) fn move_to_start(
        &mut self,
        _: &MoveToStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_selections: Vec<Selection> = self
            .selections
            .iter()
            .map(|sel| {
                let mut new_sel = sel.clone();
                new_sel.place_at(0, None);
                new_sel
            })
            .collect();

        self.selections.replace_all(new_selections);
        self.finish_move(0, None, cx);
    }

    pub(super) fn move_to_end(&mut self, _: &MoveToEnd, _: &mut Window, cx: &mut Context<Self>) {
        let doc_end = self.text.len();
        let new_selections: Vec<Selection> = self
            .selections
            .iter()
            .map(|sel| {
                let mut new_sel = sel.clone();
                new_sel.place_at(doc_end, None);
                new_sel
            })
            .collect();

        self.selections.replace_all(new_selections);
        self.finish_move(doc_end, None, cx);
    }

    pub(super) fn move_to_previous_word(
        &mut self,
        _: &MoveToPreviousWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.text.clone();
        let new_selections: Vec<Selection> = self
            .selections
            .iter()
            .map(|sel| {
                let new_offset = TextSelector::previous_word_start_at(&text, sel.cursor_offset());
                let mut new_sel = sel.clone();
                new_sel.place_at(new_offset, None);
                new_sel
            })
            .collect();

        self.selections.replace_all(new_selections);
        self.finish_move(self.cursor(), None, cx);
    }

    pub(super) fn move_to_next_word(
        &mut self,
        _: &MoveToNextWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.text.clone();
        let new_selections: Vec<Selection> = self
            .selections
            .iter()
            .map(|sel| {
                let new_offset = TextSelector::next_word_start_at(&text, sel.cursor_offset());
                let mut new_sel = sel.clone();
                new_sel.place_at(new_offset, None);
                new_sel
            })
            .collect();

        self.selections.replace_all(new_selections);
        self.finish_move(self.cursor(), None, cx);
    }

    /// Add a cursor above each existing cursor.
    ///
    /// For each existing cursor, this method finds the line above and
    /// attempts to place a new cursor at the same column position.
    pub(super) fn add_cursor_above(
        &mut self,
        _: &AddCursorAbove,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_cursor_vertical(VerticalDirection::Above, window, cx);
    }

    /// Add a cursor below each existing cursor.
    ///
    /// For each existing cursor, this method finds the line below and
    /// attempts to place a new cursor at the same column position.
    pub(super) fn add_cursor_below(
        &mut self,
        _: &AddCursorBelow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_cursor_vertical(VerticalDirection::Below, window, cx);
    }

    /// Add cursors above or below existing cursors.
    fn add_cursor_vertical(
        &mut self,
        direction: VerticalDirection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mode.is_single_line() {
            return;
        }

        let existing_selections: Vec<_> = self
            .selections
            .iter()
            .map(|s| (s.cursor_offset(), s.column_anchor))
            .collect();

        // Collect existing cursor offsets to avoid duplicates
        let existing_offsets: std::collections::HashSet<_> =
            self.selections.iter().map(|s| s.cursor_offset()).collect();

        let mut new_cursors = Vec::with_capacity(existing_selections.len());

        // Process each existing selection
        for (cursor_pos, column_anchor) in existing_selections {
            // Find the offset in the specified direction
            if let Some(offset) = self.find_offset_with_column(cursor_pos, column_anchor, direction)
            {
                // Only add if there's not already a cursor at this position
                if !existing_offsets.contains(&offset) {
                    let id = self.selections.generate_id();
                    let mut new_cursor = Selection::new(id, offset, offset);
                    // Preserve column anchor from this cursor
                    new_cursor.column_anchor = column_anchor;
                    new_cursors.push(new_cursor);
                }
            }
        }

        for cursor in new_cursors {
            self.selections.add(cursor);
        }
        cx.notify();
    }

    /// Find the offset at the same column on the line above or below, using column_anchor to preserve visual column.
    fn find_offset_with_column(
        &self,
        cursor_offset: usize,
        column_anchor: Option<usize>,
        direction: VerticalDirection,
    ) -> Option<usize> {
        let text = &self.text;
        let point = text.offset_to_point(cursor_offset);
        let line_count = text.lines_len();

        let target_row = match direction {
            VerticalDirection::Above => {
                if point.row == 0 {
                    return None;
                }
                point.row.saturating_sub(1)
            }
            VerticalDirection::Below => {
                if point.row + 1 >= line_count {
                    return None;
                }
                point.row + 1
            }
        };

        // Use the column from column_anchor if available, otherwise fall back to current column
        let preferred_column = column_anchor.unwrap_or(point.column);

        // Get the target column, clamping to line length
        let line = text.slice_line(target_row);
        let target_column = preferred_column.min(line.len());

        // Calculate offset directly: line_start_offset + column
        let line_start = text.line_start_offset(target_row);
        Some(line_start + target_column)
    }
}
