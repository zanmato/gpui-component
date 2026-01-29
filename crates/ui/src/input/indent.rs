use gpui::{
    Bounds, Context, Hsla, Path, PathBuilder, Pixels, SharedString, TextRun, TextStyle, Window,
    point, px,
};
use ropey::RopeSlice;
use std::cmp::Reverse;
use std::collections::HashSet;

use crate::{
    RopeExt,
    input::{
        CursorId, Indent, IndentInline, InputState, LastLayout, Outdent, OutdentInline, Selection,
        element::TextElement, mode::InputMode,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndentDirection {
    Indent,
    Outdent,
}

#[derive(Debug, Copy, Clone)]
pub struct TabSize {
    /// Default is 2
    pub tab_size: usize,
    /// Set true to use `\t` as tab indent, default is false
    pub hard_tabs: bool,
}

impl Default for TabSize {
    fn default() -> Self {
        Self {
            tab_size: 2,
            hard_tabs: false,
        }
    }
}

impl TabSize {
    pub(super) fn to_string(&self) -> SharedString {
        if self.hard_tabs {
            "\t".into()
        } else {
            " ".repeat(self.tab_size).into()
        }
    }

    /// Count the indent size of the line in spaces.
    pub fn indent_count(&self, line: &RopeSlice) -> usize {
        let mut count = 0;
        for ch in line.chars() {
            match ch {
                '\t' => count += self.tab_size,
                ' ' => count += 1,
                _ => break,
            }
        }

        count
    }
}

impl InputMode {
    #[inline]
    pub(super) fn is_indentable(&self) -> bool {
        match self {
            InputMode::PlainText { multi_line, .. } | InputMode::CodeEditor { multi_line, .. } => {
                *multi_line
            }
            _ => false,
        }
    }

    #[inline]
    pub(super) fn has_indent_guides(&self) -> bool {
        match self {
            InputMode::CodeEditor {
                indent_guides,
                multi_line,
                ..
            } => *indent_guides && *multi_line,
            _ => false,
        }
    }

    #[inline]
    pub(super) fn tab_size(&self) -> TabSize {
        match self {
            InputMode::PlainText { tab, .. } => *tab,
            InputMode::CodeEditor { tab, .. } => *tab,
            _ => TabSize::default(),
        }
    }
}

impl TextElement {
    /// Measure the indent width in pixels for given column count.
    fn measure_indent_width(&self, style: &TextStyle, column: usize, window: &Window) -> Pixels {
        let font_size = style.font_size.to_pixels(window.rem_size());
        let layout = window.text_system().shape_line(
            SharedString::from(" ".repeat(column)),
            font_size,
            &[TextRun {
                len: column,
                font: style.font(),
                color: Hsla::default(),
                background_color: None,
                strikethrough: None,
                underline: None,
            }],
            None,
        );

        layout.width
    }

    pub(super) fn layout_indent_guides(
        &self,
        state: &InputState,
        bounds: &Bounds<Pixels>,
        last_layout: &LastLayout,
        text_style: &TextStyle,
        window: &mut Window,
    ) -> Option<Path<Pixels>> {
        if !state.mode.has_indent_guides() {
            return None;
        }

        let indent_width =
            self.measure_indent_width(text_style, state.mode.tab_size().tab_size, window);

        let tab_size = state.mode.tab_size();
        let line_height = last_layout.line_height;
        let visible_range = last_layout.visible_range.clone();
        let mut builder = PathBuilder::stroke(px(1.));
        let mut offset_y = last_layout.visible_top;
        let mut last_indents = vec![];

        for buffer_line in visible_range {
            // visible_range contains buffer lines (not display rows)
            let line_index = buffer_line - last_layout.visible_range.start;
            let Some(line_layout) = last_layout.lines.get(line_index) else {
                continue;
            };

            // Skip hidden (folded) lines
            if state.display_map.is_buffer_line_hidden(buffer_line) {
                continue;
            }

            let line = state.text.slice_line(buffer_line);
            let mut current_indents = vec![];
            if line.len() > 0 {
                let indent_count = tab_size.indent_count(&line);
                for offset in (0..indent_count).step_by(tab_size.tab_size) {
                    let x = if indent_count > 0 {
                        indent_width * offset as f32 / tab_size.tab_size as f32
                    } else {
                        px(0.)
                    };

                    let pos = point(x + last_layout.line_number_width, offset_y);

                    builder.move_to(pos);
                    builder.line_to(point(pos.x, pos.y + line_height));
                    current_indents.push(pos.x);
                }
            } else if last_indents.len() > 0 {
                for x in &last_indents {
                    let pos = point(*x, offset_y);
                    builder.move_to(pos);
                    builder.line_to(point(pos.x, pos.y + line_height));
                }
                current_indents = last_indents.clone();
            }

            offset_y += line_layout.wrapped_lines.len() * line_height;
            last_indents = current_indents;
        }

        builder.translate(bounds.origin);
        let path = builder.build().unwrap();
        Some(path)
    }
}

impl InputState {
    /// Set whether to show indent guides in code editor mode, default is true.
    ///
    /// Only for [`InputMode::CodeEditor`] mode.
    pub fn indent_guides(mut self, indent_guides: bool) -> Self {
        debug_assert!(self.mode.is_code_editor() && self.mode.is_multi_line());
        if let InputMode::CodeEditor {
            indent_guides: l, ..
        } = &mut self.mode
        {
            *l = indent_guides;
        }
        self
    }

    /// Set indent guides in code editor mode.
    ///
    /// Only for [`InputMode::CodeEditor`] mode.
    pub fn set_indent_guides(
        &mut self,
        indent_guides: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        debug_assert!(self.mode.is_code_editor());
        if let InputMode::CodeEditor {
            indent_guides: l, ..
        } = &mut self.mode
        {
            *l = indent_guides;
        }
        cx.notify();
    }

    /// Set the tab size for the input.
    ///
    /// Only for [`InputMode::PlainText`] and [`InputMode::CodeEditor`] mode with multi_line.
    pub fn tab_size(mut self, tab: TabSize) -> Self {
        debug_assert!(self.mode.is_multi_line() || self.mode.is_code_editor());
        match &mut self.mode {
            InputMode::PlainText { tab: t, .. } => *t = tab,
            InputMode::CodeEditor { tab: t, .. } => *t = tab,
            _ => {}
        }
        self
    }

    pub(super) fn indent_inline(
        &mut self,
        _: &IndentInline,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // First, try to accept inline completion if present
        if self.accept_inline_completion(window, cx) {
            return;
        }
        self.indent(false, window, cx);
    }

    pub(super) fn indent_block(&mut self, _: &Indent, window: &mut Window, cx: &mut Context<Self>) {
        self.indent(true, window, cx);
    }

    pub(super) fn outdent_inline(
        &mut self,
        _: &OutdentInline,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.outdent(false, window, cx);
    }

    pub(super) fn outdent_block(
        &mut self,
        _: &Outdent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.outdent(true, window, cx);
    }

    pub(super) fn indent(&mut self, block: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_indent(IndentDirection::Indent, block, window, cx);
    }

    pub(super) fn outdent(&mut self, block: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_indent(IndentDirection::Outdent, block, window, cx);
    }

    fn apply_indent(
        &mut self,
        direction: IndentDirection,
        block: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.mode.is_indentable() {
            cx.propagate();
            return;
        }

        let tab_indent = self.mode.tab_size().to_string();
        let tab_indent_len = tab_indent.len();

        let has_non_collapsed_selection = self.selections.iter().any(|s| !s.is_collapsed());
        let use_block = has_non_collapsed_selection || block;

        if use_block {
            self.apply_block_indent(direction, &tab_indent, tab_indent_len, window, cx);
        } else {
            self.apply_inline_indent(direction, &tab_indent, tab_indent_len, window, cx);
        }

        cx.notify();
    }

    fn apply_block_indent(
        &mut self,
        direction: IndentDirection,
        tab_indent: &str,
        tab_indent_len: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Collect selection data: (id, start, end, start_row, end_row)
        let mut selection_data: Vec<(CursorId, usize, usize, usize, usize)> =
            Vec::with_capacity(self.selections.len());
        let mut lines: HashSet<usize> = HashSet::new();

        for selection in self.selections.iter() {
            let start_point = self.text.offset_to_point(selection.start);
            let end_point = self.text.offset_to_point(selection.end);
            selection_data.push((
                selection.id,
                selection.start,
                selection.end,
                start_point.row,
                end_point.row,
            ));
            for row in start_point.row..=end_point.row {
                lines.insert(row);
            }
        }

        let mut lines_vec: Vec<_> = lines.into_iter().collect();
        lines_vec.sort_by_key(|&row| Reverse(row));

        // Track which lines were actually modified (relevant for outdent)
        let mut modified_lines: HashSet<usize> = HashSet::new();

        for row in &lines_vec {
            let line_start = self.text.line_start_offset(*row);

            let should_modify = match direction {
                IndentDirection::Indent => true,
                IndentDirection::Outdent => {
                    // Check if line starts with tab_indent
                    line_start + tab_indent_len <= self.text.len()
                        && self.text.slice(line_start..line_start + tab_indent_len) == tab_indent
                }
            };

            if should_modify {
                let (range, text) = match direction {
                    IndentDirection::Indent => (line_start..line_start, tab_indent),
                    IndentDirection::Outdent => (line_start..line_start + tab_indent_len, ""),
                };
                self.replace_text_in_range_silent(
                    Some(self.range_to_utf16(&range)),
                    text,
                    window,
                    cx,
                );
                modified_lines.insert(*row);
            }
        }

        // Update selections
        let mut new_selections: Vec<Selection> = Vec::with_capacity(self.selections.len());
        for (selection_id, original_start, original_end, start_row, end_row) in selection_data {
            let lines_modified = (start_row..=end_row)
                .filter(|&row| modified_lines.contains(&row))
                .count();
            let start_offset = if modified_lines.contains(&start_row) {
                tab_indent_len
            } else {
                0
            };
            let end_offset = lines_modified * tab_indent_len;

            let (new_start, new_end) = match direction {
                IndentDirection::Indent => {
                    (original_start + start_offset, original_end + end_offset)
                }
                IndentDirection::Outdent => (
                    original_start.saturating_sub(start_offset),
                    original_end.saturating_sub(end_offset),
                ),
            };

            let mut new_selection = Selection::new(selection_id, new_start, new_end);
            new_selection.column_anchor = None;
            new_selections.push(new_selection);
        }
        self.selections.replace_all(new_selections);
    }

    fn apply_inline_indent(
        &mut self,
        direction: IndentDirection,
        tab_indent: &str,
        tab_indent_len: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // For outdent, we need to track the remove start position
        // Edit data: (id, cursor_offset, remove_start for outdent or None for indent)
        let mut edit_data: Vec<(CursorId, usize, Option<usize>)> =
            Vec::with_capacity(self.selections.len());

        for selection in self.selections.iter() {
            let cursor_offset = selection.cursor_offset();
            match direction {
                IndentDirection::Indent => {
                    edit_data.push((selection.id, cursor_offset, None));
                }
                IndentDirection::Outdent => {
                    let start = cursor_offset.saturating_sub(tab_indent_len);
                    if start + tab_indent_len <= self.text.len() {
                        let slice = self.text.slice(start..start + tab_indent_len);
                        if slice == tab_indent {
                            edit_data.push((selection.id, cursor_offset, Some(start)));
                        }
                    }
                }
            }
        }

        // Sort by position desc
        edit_data.sort_by_key(|(_, offset, _)| Reverse(*offset));

        // Apply changes
        for (_, cursor_or_remove_start, maybe_remove_start) in &edit_data {
            let (range, text) = match direction {
                IndentDirection::Indent => {
                    (*cursor_or_remove_start..*cursor_or_remove_start, tab_indent)
                }
                IndentDirection::Outdent => (
                    maybe_remove_start.unwrap()..maybe_remove_start.unwrap() + tab_indent_len,
                    "",
                ),
            };
            self.replace_text_in_range_silent(Some(self.range_to_utf16(&range)), text, window, cx);
        }

        // Update selections
        let mut new_selections: Vec<Selection> = Vec::with_capacity(self.selections.len());
        for (selection_id, cursor_offset, _) in &edit_data {
            let edits_at_or_before = edit_data
                .iter()
                .filter(|(_, offset, _)| *offset <= *cursor_offset)
                .count();
            let offset_delta = edits_at_or_before * tab_indent_len;

            let new_offset = match direction {
                IndentDirection::Indent => cursor_offset + offset_delta,
                IndentDirection::Outdent => cursor_offset.saturating_sub(offset_delta),
            };

            let mut new_selection = Selection::new(*selection_id, new_offset, new_offset);
            new_selection.column_anchor = None;
            new_selections.push(new_selection);
        }
        self.selections.replace_all(new_selections);
    }
}

#[cfg(test)]
mod tests {
    use ropey::RopeSlice;

    use super::TabSize;

    #[test]
    fn test_tab_size() {
        let tab = TabSize {
            tab_size: 2,
            hard_tabs: false,
        };
        assert_eq!(tab.to_string(), "  ");
        let tab = TabSize {
            tab_size: 4,
            hard_tabs: false,
        };
        assert_eq!(tab.to_string(), "    ");

        let tab = TabSize {
            tab_size: 2,
            hard_tabs: true,
        };
        assert_eq!(tab.to_string(), "\t");
        let tab = TabSize {
            tab_size: 4,
            hard_tabs: true,
        };
        assert_eq!(tab.to_string(), "\t");
    }

    #[test]
    fn test_tab_size_indent_count() {
        let tab = TabSize {
            tab_size: 4,
            hard_tabs: false,
        };
        assert_eq!(tab.indent_count(&RopeSlice::from("abc")), 0);
        assert_eq!(tab.indent_count(&RopeSlice::from("  abc")), 2);
        assert_eq!(tab.indent_count(&RopeSlice::from("    abc")), 4);
        assert_eq!(tab.indent_count(&RopeSlice::from("\tabc")), 4);
        assert_eq!(tab.indent_count(&RopeSlice::from("  \tabc")), 6);
        assert_eq!(tab.indent_count(&RopeSlice::from(" \t abc  ")), 6);
        assert_eq!(tab.indent_count(&RopeSlice::from("abc")), 0);
    }
}
