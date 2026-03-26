use std::ops::Range;
use std::time::Duration;

use anyhow::Result;
use gpui::{App, Context, Hsla, Task, Window};
use ropey::Rope;

use crate::input::{EditorMode, InputBaseState, Lsp, RopeExt};

pub trait SelectionRangeProvider {
    /// Returns hierarchical selection ranges for the given position.
    /// The ranges should be ordered from smallest to largest, with each parent
    /// encompassing the previous range.
    ///
    /// textDocument/selectionRange
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_selectionRange
    fn selection_ranges(
        &self,
        text: &Rope,
        position: lsp_types::Position,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<lsp_types::SelectionRange>>>;
}

impl Lsp {
    /// Get the current selection range that intersects with the visible range (0-based row).
    ///
    /// `color` is the editor's active-line color, supplied by the caller since
    /// the base crate has no theme of its own.
    ///
    /// Returns byte range and color.
    pub(crate) fn selection_range_for_range(
        &self,
        text: &Rope,
        visible_range: &Range<usize>,
        color: Option<Hsla>,
    ) -> Option<(Range<usize>, Hsla)> {
        let Some((range, _)) = &self.selection_range else {
            return None;
        };

        let Some(color) = color else {
            return None;
        };

        let start = text.position_to_offset(&range.start);
        let end = text.position_to_offset(&range.end);

        // Convert visible line range to byte offsets
        let visible_start = text.line_start_offset(visible_range.start);
        let visible_end = text.line_end_offset(visible_range.end.saturating_sub(1));

        // Clamp the selection range to visible boundaries
        let clamped_start = start.max(visible_start);
        let clamped_end = end.min(visible_end).max(clamped_start);

        Some((clamped_start..clamped_end, color))
    }
}

impl InputBaseState<EditorMode> {
    /// Handle selection range LSP request.
    pub(crate) fn handle_selection_ranges(
        &mut self,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selecting {
            return;
        }

        // Don't show selection range highlights when user has a manual selection
        if !self.active_selection().is_empty() {
            self.extras.lsp.clear_selection_range();
            return;
        }

        let Some(provider) = self.extras.lsp.selection_range_provider.clone() else {
            return;
        };

        let position = self.text.offset_to_position(offset);
        let task = provider.selection_ranges(&self.text, position, window, cx);
        let editor = cx.entity();
        let should_delay = self.extras.lsp.selection_range.is_none();
        self.extras.lsp._selection_range_task = cx.spawn(async move |_, cx| {
            if should_delay {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }

            let selection_range_opt = task.await?;

            editor.update(cx, |editor, cx| {
                // Extract the largest range from the hierarchy for highlighting
                let new_range = selection_range_opt.map(|sr| {
                    // Find the outermost (largest) range by traversing parents
                    fn find_largest(mut sr: &lsp_types::SelectionRange) -> lsp_types::Range {
                        let mut largest = sr.range;
                        while let Some(parent) = &sr.parent {
                            largest = parent.range;
                            sr = parent;
                        }
                        largest
                    }
                    (find_largest(&sr), ())
                });

                // Only update if the range actually changed
                if new_range != editor.extras.lsp.selection_range {
                    editor.extras.lsp.selection_range = new_range;

                    cx.emit(crate::input::InputEvent::SelectionRangeChange {
                        range: new_range.unwrap_or_default().0,
                    });

                    cx.notify();
                }
            });

            Ok(())
        });
    }
}
