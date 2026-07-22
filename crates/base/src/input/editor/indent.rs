use crate::input::InputModeKind;
use crate::input::{
    Indent, IndentInline, InputBaseState, Outdent, OutdentInline, RopeExt, cursor::CursorSelection,
    element::TextElement, layout::LastLayout, mode::LayoutMode,
};
use gpui::{
    Bounds, Context, Hsla, Path, PathBuilder, Pixels, SharedString, TextRun, TextStyle, Window,
    point, px,
};
use ropey::RopeSlice;
use std::ops::Range;

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

    fn next_tab_stop(&self, column: usize) -> usize {
        column + self.tab_size - column % self.tab_size
    }

    fn prev_tab_stop(&self, column: usize) -> usize {
        column.saturating_sub(if column % self.tab_size == 0 {
            self.tab_size
        } else {
            column % self.tab_size
        })
    }

    fn indent_string(&self, column: usize) -> String {
        if column == self.tab_size {
            return self.to_string().to_string();
        }

        if self.hard_tabs {
            format!(
                "{}{}",
                "\t".repeat(column / self.tab_size),
                " ".repeat(column % self.tab_size)
            )
        } else {
            " ".repeat(column)
        }
    }
}

fn leading_ws_bytes(line: &RopeSlice) -> usize {
    line.chars()
        .take_while(|ch| matches!(ch, ' ' | '\t'))
        .map(char::len_utf8)
        .sum()
}

/// Map a byte offset from the original document through sorted, disjoint edits.
fn map_offset(offset: usize, edits: &[(Range<usize>, String)]) -> usize {
    let mut delta = 0isize;

    for (range, replacement) in edits {
        if offset <= range.start {
            break;
        }

        let edit_delta = replacement.len() as isize - range.len() as isize;
        if offset >= range.end {
            delta += edit_delta;
        } else {
            return (range.start as isize + delta + replacement.len() as isize) as usize;
        }
    }

    (offset as isize + delta) as usize
}

impl LayoutMode {
    /// Whether this layout indents blocks.
    ///
    /// Callers gate this on the input being multi-line: indenting a one-line
    /// text field has nothing to indent.
    #[inline]
    pub(super) fn is_indentable(&self) -> bool {
        matches!(
            self,
            LayoutMode::PlainText { .. } | LayoutMode::CodeEditor { .. }
        )
    }

    #[inline]
    pub(super) fn has_indent_guides(&self) -> bool {
        match self {
            LayoutMode::CodeEditor { indent_guides, .. } => *indent_guides,
            _ => false,
        }
    }

    #[inline]
    pub(super) fn tab_size(&self) -> TabSize {
        match self {
            LayoutMode::PlainText { tab, .. } => *tab,
            LayoutMode::CodeEditor { tab, .. } => *tab,
            _ => TabSize::default(),
        }
    }
}

impl<M: InputModeKind> TextElement<M> {
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
        state: &InputBaseState<M>,
        bounds: &Bounds<Pixels>,
        last_layout: &LastLayout,
        text_style: &TextStyle,
        window: &mut Window,
    ) -> Option<Path<Pixels>> {
        if !state.is_multi_line() || !state.mode.has_indent_guides() {
            return None;
        }

        let indent_width =
            self.measure_indent_width(text_style, state.mode.tab_size().tab_size, window);

        let tab_size = state.mode.tab_size();
        let line_height = last_layout.line_height;
        let mut builder = PathBuilder::stroke(px(1.));
        let mut offset_y = last_layout.visible_top;
        let mut last_indents = vec![];

        for (&buffer_line, line_layout) in last_layout
            .visible_buffer_lines
            .iter()
            .zip(last_layout.lines.iter())
        {
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

/// Indent guides are a code-editor affordance.
impl InputBaseState<crate::input::EditorMode> {
    /// Set whether to show indent guides, default is true.
    #[doc(hidden)]
    pub fn indent_guides(mut self, indent_guides: bool) -> Self {
        if let LayoutMode::CodeEditor {
            indent_guides: l, ..
        } = &mut self.mode
        {
            *l = indent_guides;
        }
        self
    }

    /// Set indent guides at runtime.
    pub fn set_indent_guides(
        &mut self,
        indent_guides: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let LayoutMode::CodeEditor {
            indent_guides: l, ..
        } = &mut self.mode
        {
            *l = indent_guides;
        }
        cx.notify();
    }
}

impl<M: InputModeKind> InputBaseState<M> {
    pub(super) fn indent_inline(
        &mut self,
        _: &IndentInline,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // First, try to accept inline completion if present
        if M::accept_inline_completion(self, window, cx) {
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

    /// Apply an indent or outdent across all selections as one batch edit.
    ///
    /// A batch keeps the whole operation a single undo transaction (instead of
    /// one push per line) and restores the correct multi-selection extents on
    /// undo/redo.
    fn apply_indent(
        &mut self,
        direction: IndentDirection,
        block: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_editable() || !self.is_multi_line() || !self.mode.is_indentable() {
            cx.propagate();
            return;
        }

        // Non-collapsed selections and explicit block operations indent whole lines.
        let has_non_collapsed = self.selections.iter().any(|sel| !sel.is_collapsed());
        let use_block = has_non_collapsed || block;

        let before: Vec<CursorSelection> = self.selections.iter().copied().collect();

        let (edits, new_selections) = if use_block {
            self.compute_block_indent(direction)
        } else {
            self.compute_inline_indent(direction)
        };

        if edits.is_empty() {
            return;
        }

        self.undo_manager.begin_transaction();
        self.replace_text_in_ranges(&edits, window, cx);
        self.selections.replace_all(new_selections);
        let after: Vec<CursorSelection> = self.selections.iter().copied().collect();
        self.undo_manager.record_selections(before, after);
        self.undo_manager.commit_transaction();

        self.scroll_to(self.cursor(), None, cx);
        cx.notify();
    }

    /// Build the per-line edits and resulting selections for a block
    /// indent/outdent across every selection.
    fn compute_block_indent(
        &self,
        direction: IndentDirection,
    ) -> (Vec<(Range<usize>, String)>, Vec<CursorSelection>) {
        let mut selection_data: Vec<CursorSelection> = Vec::with_capacity(self.selections.len());
        let mut rows: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for sel in self.selections.iter() {
            let start_row = self.text.offset_to_point(sel.start).row;
            let end_row = self.text.offset_to_point(sel.end).row;
            selection_data.push(*sel);
            for row in start_row..=end_row {
                rows.insert(row);
            }
        }

        let mut rows: Vec<usize> = rows.into_iter().collect();
        rows.sort_unstable();

        let tab_size = self.mode.tab_size();
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        for row in rows {
            let line_start = self.text.line_start_offset(row);
            let line = self.text.slice_line(row);
            let old_bytes = leading_ws_bytes(&line);
            let column = tab_size.indent_count(&line);
            let new_column = match direction {
                IndentDirection::Indent => tab_size.next_tab_stop(column),
                IndentDirection::Outdent => tab_size.prev_tab_stop(column),
            };
            let old_indent = self.text.slice(line_start..line_start + old_bytes);
            let new_indent = tab_size.indent_string(new_column);

            if new_column != column || old_indent != new_indent.as_str() {
                edits.push((line_start..line_start + old_bytes, new_indent));
            }
        }

        let mut new_selections: Vec<CursorSelection> = Vec::with_capacity(selection_data.len());
        for mut selection in selection_data {
            // A selection boundary at the start of a replaced indent stays
            // there, so a whole-line selection keeps its column-zero anchor. A
            // lone caret inside the indentation instead lands after the new
            // indent, which is where the user expects to keep typing.
            let caret_in_indent = if selection.is_collapsed() {
                edits
                    .iter()
                    .find(|(range, _)| {
                        range.start <= selection.start && selection.start <= range.end
                    })
                    .map(|(range, text)| map_offset(range.start, &edits) + text.len())
            } else {
                None
            };
            if let Some(offset) = caret_in_indent {
                selection.start = offset;
                selection.end = offset;
            } else {
                selection.start = map_offset(selection.start, &edits);
                selection.end = map_offset(selection.end, &edits);
            }
            selection.column_anchor = None;
            new_selections.push(selection);
        }

        (edits, new_selections)
    }

    /// Build the per-cursor edits and resulting cursors for a collapsed inline
    /// indent/outdent.
    fn compute_inline_indent(
        &self,
        direction: IndentDirection,
    ) -> (Vec<(Range<usize>, String)>, Vec<CursorSelection>) {
        let tab_size = self.mode.tab_size();
        let mut candidates: Vec<(Range<usize>, String)> = Vec::with_capacity(self.selections.len());
        for sel in self.selections.iter() {
            let cursor = sel.cursor_offset();
            let row = self.text.offset_to_point(cursor).row;
            let line_start = self.text.line_start_offset(row);
            let line = self.text.slice_line(row);
            let old_bytes = leading_ws_bytes(&line);
            let column = tab_size.indent_count(&line);

            match direction {
                IndentDirection::Indent if cursor <= line_start + old_bytes => {
                    let new_indent = tab_size.indent_string(tab_size.next_tab_stop(column));
                    candidates.push((line_start..line_start + old_bytes, new_indent));
                }
                IndentDirection::Indent => {
                    let cursor_column = self
                        .text
                        .slice(line_start..cursor)
                        .chars()
                        .fold(0, |column, ch| {
                            column + if ch == '\t' { tab_size.tab_size } else { 1 }
                        });
                    let text = if tab_size.hard_tabs {
                        "\t".to_string()
                    } else {
                        " ".repeat(tab_size.next_tab_stop(cursor_column) - cursor_column)
                    };
                    candidates.push((cursor..cursor, text));
                }
                IndentDirection::Outdent => {
                    let new_indent = tab_size.indent_string(tab_size.prev_tab_stop(column));
                    let old_indent = self.text.slice(line_start..line_start + old_bytes);
                    if column != tab_size.prev_tab_stop(column) || old_indent != new_indent.as_str()
                    {
                        candidates.push((line_start..line_start + old_bytes, new_indent));
                    }
                }
            }
        }

        // Build disjoint edits, dropping any that would overlap a previous one.
        candidates.sort_by_key(|(range, _)| range.start);
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        let mut previous: Option<Range<usize>> = None;
        for (range, text) in candidates {
            if let Some(previous) = &previous {
                if range.start < previous.end
                    || (range.is_empty() && previous.is_empty() && range.start == previous.start)
                {
                    continue;
                }
            }
            previous = Some(range.clone());
            edits.push((range, text));
        }

        let mut new_selections: Vec<CursorSelection> = Vec::with_capacity(self.selections.len());
        for sel in self.selections.iter() {
            let cursor = sel.cursor_offset();
            let mut new_offset = map_offset(cursor, &edits);
            // Unlike a selection boundary at the start of a replaced indent,
            // an inline caret follows whitespace inserted at its own position.
            if let Some((_, text)) = edits
                .iter()
                .find(|(range, _)| range.is_empty() && range.start == cursor)
            {
                new_offset += text.len();
            }
            let mut selection = CursorSelection::new(sel.id, new_offset, new_offset);
            selection.column_anchor = None;
            new_selections.push(selection);
        }

        (edits, new_selections)
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
enum IndentDirection {
    Indent,
    Outdent,
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use ropey::RopeSlice;

    use super::{TabSize, leading_ws_bytes, map_offset};

    #[test]
    fn test_tab_stops() {
        let tab = TabSize {
            tab_size: 4,
            hard_tabs: false,
        };
        assert_eq!(tab.next_tab_stop(0), 4);
        assert_eq!(tab.next_tab_stop(3), 4);
        assert_eq!(tab.next_tab_stop(4), 8);
        assert_eq!(tab.prev_tab_stop(0), 0);
        assert_eq!(tab.prev_tab_stop(3), 0);
        assert_eq!(tab.prev_tab_stop(4), 0);
        assert_eq!(tab.prev_tab_stop(7), 4);
        assert_eq!(tab.prev_tab_stop(8), 4);
    }

    #[test]
    fn test_indent_string() {
        let spaces = TabSize {
            tab_size: 4,
            hard_tabs: false,
        };
        assert_eq!(spaces.indent_string(0), "");
        assert_eq!(spaces.indent_string(6), "      ");

        let tabs = TabSize {
            tab_size: 4,
            hard_tabs: true,
        };
        assert_eq!(tabs.indent_string(0), "");
        assert_eq!(tabs.indent_string(4), "\t");
        assert_eq!(tabs.indent_string(6), "\t  ");
        assert_eq!(tabs.indent_string(8), "\t\t");
    }

    #[test]
    fn test_leading_ws_bytes() {
        assert_eq!(leading_ws_bytes(&RopeSlice::from("\t  abc")), 3);
        assert_eq!(leading_ws_bytes(&RopeSlice::from("abc")), 0);
    }

    #[test]
    fn test_map_offset() {
        let edits: Vec<(Range<usize>, String)> = vec![(2..5, "x".into())];
        assert_eq!(map_offset(1, &edits), 1);
        assert_eq!(map_offset(2, &edits), 2);
        assert_eq!(map_offset(3, &edits), 3);
        assert_eq!(map_offset(5, &edits), 3);
        assert_eq!(map_offset(8, &edits), 6);

        // CursorSelection boundaries at an insertion point stay before the edit.
        let insertion = vec![(2..2, "    ".into())];
        assert_eq!(map_offset(2, &insertion), 2);
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

/// Tab size only means something where there is more than one line to indent.
impl<M: crate::input::MultiLineMode> InputBaseState<M> {
    /// Set the tab size for the input.
    #[doc(hidden)]
    pub fn tab_size(mut self, tab: TabSize) -> Self {
        match &mut self.mode {
            LayoutMode::PlainText { tab: t, .. } => *t = tab,
            LayoutMode::CodeEditor { tab: t, .. } => *t = tab,
            _ => {}
        }
        self
    }

    pub fn set_tab_size(&mut self, tab: TabSize, cx: &mut Context<Self>) {
        match &mut self.mode {
            LayoutMode::PlainText { tab: value, .. }
            | LayoutMode::CodeEditor { tab: value, .. } => *value = tab,
            _ => {}
        }
        cx.notify();
    }
}
