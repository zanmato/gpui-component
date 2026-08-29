use anyhow::Result;
use gpui::{App, Context, SharedString, Task, Window};
use instant::Duration;
use lsp_types::{InlayHint, InlayHintLabel};
use ropey::Rope;
use std::ops::Range;

use crate::input::{EditorMode, InputBaseState, Lsp, RopeExt};

/// Inlay hint provider: short labels rendered inside the code, such as
/// parameter names and inferred types.
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_inlayHint
pub trait InlayHintProvider {
    /// Fetches the inlay hints for the given byte range.
    ///
    /// textDocument/inlayHint
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_inlayHint
    fn inlay_hints(
        &self,
        text: &Rope,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<InlayHint>>>;
}

/// Flatten a hint's label — a plain string or an array of parts — into the
/// text to render, honoring the padding flags as single spaces.
fn hint_text(hint: &InlayHint) -> SharedString {
    let label = match &hint.label {
        InlayHintLabel::String(label) => label.clone(),
        InlayHintLabel::LabelParts(parts) => parts.iter().map(|part| part.value.as_str()).collect(),
    };
    let padding_left = hint.padding_left.unwrap_or(false);
    let padding_right = hint.padding_right.unwrap_or(false);
    SharedString::from(format!(
        "{}{}{}",
        if padding_left { " " } else { "" },
        label,
        if padding_right { " " } else { "" }
    ))
}

impl Lsp {
    /// Enable or disable rendering of inlay hints. Enabled by default;
    /// disabling clears the cached hints.
    pub fn set_inlay_hints_enabled(&mut self, enabled: bool) {
        self.inlay_hints_enabled = enabled;
        if !enabled {
            self.inlay_hints.clear();
        }
    }

    /// Whether inlay hints are rendered.
    pub fn has_inlay_hints_enabled(&self) -> bool {
        self.inlay_hints_enabled
    }

    /// The cached hints on `line`, as (byte offset within the line, text to
    /// render) sorted by offset. Called per visible line at layout time.
    pub(crate) fn inlay_hint_splices(
        &self,
        text: &Rope,
        line: usize,
    ) -> Vec<(usize, SharedString)> {
        if self.inlay_hints.is_empty() {
            return Vec::new();
        }

        let line_start = text.line_start_offset(line);
        let line_end = text.line_end_offset(line);
        let lo = self
            .inlay_hints
            .partition_point(|(position, _)| (position.line as usize) < line);
        self.inlay_hints[lo..]
            .iter()
            .take_while(|(position, _)| position.line as usize == line)
            .map(|(position, label)| {
                let offset = text.position_to_offset(position).min(line_end);
                (offset - line_start, label.clone())
            })
            .collect()
    }

    pub(crate) fn update_inlay_hints(
        &mut self,
        text: &Rope,
        version: u64,
        window: &mut Window,
        cx: &mut Context<InputBaseState<EditorMode>>,
    ) {
        if !self.inlay_hints_enabled {
            return;
        }
        let Some(provider) = self.inlay_hint_provider.clone() else {
            return;
        };

        // Fetch the whole document; results are cached and filtered per
        // visible line at layout time (mirrors `update_semantic_tokens`), so
        // a scroll never needs a refetch.
        let text = text.clone();
        let range = 0..text.len();
        let input_state = cx.entity();

        // debounce timer 100ms
        self._inlay_hint_task = cx.spawn_in(window, async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;

            let task_result = cx
                .update(|window, cx| provider.inlay_hints(&text, range, window, cx))
                .ok();

            if let Some(task) = task_result {
                if let Ok(hints) = task.await {
                    let mut decoded: Vec<(lsp_types::Position, SharedString)> = hints
                        .iter()
                        .map(|hint| (hint.position, hint_text(hint)))
                        .collect();
                    decoded.sort_by_key(|(position, _)| *position);

                    let _ = input_state.update(cx, |input_state, cx| {
                        if input_state.document_version() != version {
                            return;
                        }
                        if decoded != input_state.extras.lsp.inlay_hints {
                            input_state.extras.lsp.inlay_hints = decoded;
                            cx.notify();
                        }
                    });
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::build_editor;
    use super::*;
    use gpui::TestAppContext;
    use lsp_types::{InlayHintLabelPart, Position};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[test]
    fn test_hint_text_concatenates_parts_and_padding() {
        let hint = |label: InlayHintLabel, left: bool, right: bool| InlayHint {
            position: Position::new(0, 0),
            label,
            kind: None,
            text_edits: None,
            tooltip: None,
            padding_left: left.then_some(true),
            padding_right: right.then_some(true),
            data: None,
        };

        assert_eq!(
            hint_text(&hint(InlayHintLabel::String("x:".into()), false, true)).as_ref(),
            "x: "
        );
        let parts = InlayHintLabel::LabelParts(vec![
            InlayHintLabelPart {
                value: "name".into(),
                ..Default::default()
            },
            InlayHintLabelPart {
                value: ":".into(),
                ..Default::default()
            },
        ]);
        assert_eq!(hint_text(&hint(parts, true, false)).as_ref(), " name:");
    }

    struct CountingHints {
        fetches: Rc<Cell<usize>>,
        hints: RefCell<Vec<InlayHint>>,
    }

    impl InlayHintProvider for CountingHints {
        fn inlay_hints(
            &self,
            _: &Rope,
            _: Range<usize>,
            _: &mut Window,
            _: &mut App,
        ) -> Task<Result<Vec<InlayHint>>> {
            self.fetches.set(self.fetches.get() + 1);
            Task::ready(Ok(self.hints.borrow().clone()))
        }
    }

    fn hint_at(line: u32, character: u32, label: &str) -> InlayHint {
        InlayHint {
            position: Position::new(line, character),
            label: InlayHintLabel::String(label.into()),
            kind: None,
            text_edits: None,
            tooltip: None,
            padding_left: None,
            padding_right: None,
            data: None,
        }
    }

    #[gpui::test]
    fn edits_debounce_into_one_fetch_and_cache_by_line(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);
        let fetches = Rc::new(Cell::new(0));
        let provider = Rc::new(CountingHints {
            fetches: fetches.clone(),
            // 世/界 are 3 UTF-8 bytes but one UTF-16 unit each: character 3
            // on line 1 lands after "世界x" (byte 7 within the line).
            hints: RefCell::new(vec![
                hint_at(0, 3, ": int"),
                hint_at(1, 3, "len:"),
                hint_at(1, 0, "n:"),
            ]),
        });

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.extras.lsp.inlay_hint_provider = Some(provider.clone());
                // Several edits in quick succession…
                editor.set_value("aaa\n世界x(", window, cx);
                editor.set_value("bbb\n世界x(", window, cx);
                editor.set_value("ccc\n世界x(y", window, cx);
            });
        });
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();

        // …collapse into a single debounced fetch.
        assert_eq!(fetches.get(), 1);

        cx.update(|_, cx| {
            let editor = editor.read(cx);
            let text = editor.text().clone();
            assert_eq!(
                editor.extras.lsp.inlay_hint_splices(&text, 0),
                vec![(3, SharedString::from(": int"))]
            );
            // Sorted by offset, UTF-16 characters resolved to bytes.
            assert_eq!(
                editor.extras.lsp.inlay_hint_splices(&text, 1),
                vec![
                    (0, SharedString::from("n:")),
                    (7, SharedString::from("len:"))
                ]
            );
        });
    }

    #[gpui::test]
    fn refetch_replaces_hints_and_disabling_clears(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);
        let fetches = Rc::new(Cell::new(0));
        let provider = Rc::new(CountingHints {
            fetches: fetches.clone(),
            hints: RefCell::new(vec![hint_at(0, 1, "x")]),
        });

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.extras.lsp.inlay_hint_provider = Some(provider.clone());
                editor.set_value("aa", window, cx);
            });
        });
        // Let the debounce elapse and the first result land, then edit: the
        // refetch scheduled by the edit replaces it with the fresh data.
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                provider.hints.borrow_mut()[0] = hint_at(0, 2, "fresh");
                editor.set_value("aab", window, cx);
            });
        });
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();

        cx.update(|_, cx| {
            let editor = editor.read(cx);
            let text = editor.text().clone();
            assert_eq!(
                editor.extras.lsp.inlay_hint_splices(&text, 0),
                vec![(2, SharedString::from("fresh"))]
            );
        });

        // Disabling clears the cache and stops future fetches.
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.extras.lsp.set_inlay_hints_enabled(false);
                assert!(editor.extras.lsp.inlay_hints.is_empty());
                editor.set_value("aabc", window, cx);
            });
        });
        let fetches_before = fetches.get();
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        assert_eq!(fetches.get(), fetches_before);
    }
}
