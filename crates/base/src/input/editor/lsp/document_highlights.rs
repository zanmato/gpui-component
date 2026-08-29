use anyhow::Result;
use gpui::{App, Context, Task};
use instant::Duration;
use lsp_types::{DocumentHighlight, DocumentHighlightKind};
use ropey::Rope;
use std::ops::Range;

use crate::input::{EditorMode, InputBaseState, RopeExt};

/// How long the cursor must rest on a symbol before its occurrences are
/// fetched.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Document highlight provider: the occurrences of the symbol under the
/// cursor, distinguishing reads from writes.
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_documentHighlight
pub trait DocumentHighlightProvider {
    /// Fetches the highlights for the symbol at the given byte offset.
    ///
    /// Unlike the other providers this takes no `Window`: the request is
    /// issued from cursor movement, which runs without one.
    ///
    /// textDocument/documentHighlight
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_documentHighlight
    fn document_highlights(
        &self,
        text: &Rope,
        offset: usize,
        cx: &mut App,
    ) -> Task<Result<Vec<DocumentHighlight>>>;
}

impl InputBaseState<EditorMode> {
    /// Schedule a debounced occurrences fetch for the symbol under the
    /// cursor. Called whenever the cursor moves; rescheduling drops the
    /// previous pending fetch.
    pub(crate) fn schedule_document_highlights(&mut self, cx: &mut Context<Self>) {
        if self.extras.lsp.document_highlight_provider.is_none() {
            if !self.extras.lsp.document_highlights.is_empty() {
                self.extras.lsp.document_highlights.clear();
                cx.notify();
            }
            return;
        }

        let offset = self.cursor();
        let version = self.document_version;
        let executor = cx.background_executor().clone();
        self.extras.lsp._document_highlight_task = cx.spawn(async move |editor, cx| {
            executor.timer(DEBOUNCE).await;

            let task = editor
                .update(cx, |editor, cx| {
                    if editor.cursor() != offset || editor.document_version != version {
                        return None;
                    }
                    let provider = editor.extras.lsp.document_highlight_provider.clone()?;
                    Some(provider.document_highlights(&editor.text, offset, cx))
                })
                .ok()
                .flatten();
            let Some(task) = task else {
                return;
            };
            let Ok(highlights) = task.await else {
                return;
            };

            editor
                .update(cx, |editor, cx| {
                    if editor.document_version != version {
                        return;
                    }
                    let mut resolved: Vec<(Range<usize>, DocumentHighlightKind)> = highlights
                        .iter()
                        .filter_map(|highlight| {
                            let start = editor.text.position_to_offset(&highlight.range.start);
                            let end = editor.text.position_to_offset(&highlight.range.end);
                            (start < end).then(|| {
                                (
                                    start..end,
                                    highlight.kind.unwrap_or(DocumentHighlightKind::TEXT),
                                )
                            })
                        })
                        .collect();
                    resolved.sort_by_key(|(range, _)| range.start);
                    editor.extras.lsp.document_highlights = resolved;
                    cx.notify();
                })
                .ok();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::build_editor;
    use super::*;
    use gpui::TestAppContext;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct SymbolOccurrences {
        requests: Rc<RefCell<Vec<usize>>>,
    }

    impl DocumentHighlightProvider for SymbolOccurrences {
        fn document_highlights(
            &self,
            _: &Rope,
            offset: usize,
            _: &mut App,
        ) -> Task<Result<Vec<DocumentHighlight>>> {
            self.requests.borrow_mut().push(offset);
            // "count" occurs after an emoji, so byte and UTF-16 offsets
            // diverge: line 0 of "🌍 count\ncount = count" holds one
            // occurrence at UTF-16 columns 3..8.
            Task::ready(Ok(vec![
                DocumentHighlight {
                    range: lsp_types::Range::new(
                        lsp_types::Position::new(0, 3),
                        lsp_types::Position::new(0, 8),
                    ),
                    kind: Some(DocumentHighlightKind::READ),
                },
                DocumentHighlight {
                    range: lsp_types::Range::new(
                        lsp_types::Position::new(1, 0),
                        lsp_types::Position::new(1, 5),
                    ),
                    kind: Some(DocumentHighlightKind::WRITE),
                },
            ]))
        }
    }

    #[gpui::test]
    fn cursor_rest_fetches_occurrences_and_edits_clear_them(cx: &mut TestAppContext) {
        let (editor, mut cx) = build_editor(cx);
        let requests: Rc<RefCell<Vec<usize>>> = Rc::default();

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_value("🌍 count\ncount = count", window, cx);
                editor.extras.lsp.document_highlight_provider = Some(Rc::new(SymbolOccurrences {
                    requests: requests.clone(),
                }));
            });
        });

        // Two rapid moves coalesce into one debounced fetch.
        cx.update(|_, cx| {
            editor.update(cx, |editor, cx| {
                editor.move_to(1, None, cx);
                editor.move_to("🌍 c".len(), None, cx);
            });
        });
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        assert_eq!(requests.borrow().len(), 1);

        cx.update(|_, cx| {
            let highlights = &editor.read(cx).extras.lsp.document_highlights;
            // UTF-16 columns 3..8 on the emoji line resolve to the byte
            // range of "count".
            assert_eq!(
                highlights,
                &vec![
                    ("🌍 ".len().."🌍 count".len(), DocumentHighlightKind::READ),
                    (
                        "🌍 count\n".len().."🌍 count\ncount".len(),
                        DocumentHighlightKind::WRITE
                    ),
                ]
            );
        });

        // An edit clears the now-stale ranges.
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.insert("x", window, cx);
                assert!(editor.extras.lsp.document_highlights.is_empty());
            });
        });
    }
}
