//! A visual row whose offsets remain in source UTF-8 bytes.
use gpui::{App, Pixels, Point, ShapedLine, SharedString, TextAlign, Window, point, px};
use std::ops::Range;

pub(crate) struct InlineFragment {
    pub(crate) range: Range<usize>,
    pub(crate) x: Pixels,
    pub(crate) width: Pixels,
    pub(crate) text: Option<ShapedLine>,
}

pub(crate) struct InputLine {
    pub(crate) len: usize,
    pub(crate) width: Pixels,
    pub(crate) text: SharedString,
    content: Content,
}
// Keep the ordinary shaped row inline: boxing it would add an allocation to
// every existing plain-text row. Only token rows allocate fragment storage.
#[allow(clippy::large_enum_variant)]
enum Content {
    Text(ShapedLine),
    Inline(Vec<InlineFragment>),
}
impl From<ShapedLine> for InputLine {
    fn from(line: ShapedLine) -> Self {
        Self {
            len: line.len,
            width: line.width,
            text: line.text.clone(),
            content: Content::Text(line),
        }
    }
}
impl InputLine {
    pub(crate) fn inline(text: SharedString, fragments: Vec<InlineFragment>) -> Self {
        let width = fragments.last().map_or(px(0.), |f| f.x + f.width);
        Self {
            len: text.len(),
            text,
            width,
            content: Content::Inline(fragments),
        }
    }
    /// The bytes whose glyphs lie within `x`, found by binary search over
    /// the glyph positions, which shaping leaves in visual order. Rows of
    /// inline tokens, and a range that bidirectional text turns around,
    /// give the whole row.
    pub(crate) fn byte_range_for_x(&self, x: &Range<Pixels>) -> Range<usize> {
        let Content::Text(line) = &self.content else {
            return 0..self.len;
        };
        // The byte index of the first glyph at or right of `target`.
        let index_at = |target: Pixels| {
            let run = line.runs.partition_point(|run| {
                run.glyphs
                    .last()
                    .is_some_and(|glyph| glyph.position.x < target)
            });
            line.runs
                .get(run)
                .and_then(|run| {
                    let glyph = run
                        .glyphs
                        .partition_point(|glyph| glyph.position.x < target);
                    run.glyphs.get(glyph).map(|glyph| glyph.index)
                })
                .unwrap_or(self.len)
        };
        let range = index_at(x.start)..index_at(x.end);
        if range.start > range.end {
            0..self.len
        } else {
            range
        }
    }

    /// [`Self::x_for_index`] for each of `indices`, which must not decrease,
    /// in one walk over the glyphs that stops at the last index.
    /// `x_for_index` walks the glyphs from the start of the row on every
    /// call, so asking it for every marked character of a long row is
    /// quadratic.
    pub(crate) fn xs_for_sorted_indices(&self, indices: &[usize]) -> Vec<Pixels> {
        let Content::Text(line) = &self.content else {
            // Rows of inline tokens are short; they keep the plain walk.
            return indices.iter().map(|&ix| self.x_for_index(ix)).collect();
        };
        // `x_for_index` gives the first glyph, in shaped order, whose index is
        // at least `ix`: the first glyph whose running maximum index reaches
        // `ix`. The running maximum never decreases, so ascending indices are
        // answered in order as the walk goes.
        let mut xs = Vec::with_capacity(indices.len());
        let mut pending = indices.iter().peekable();
        let mut max_index = 0;
        for glyph in line.runs.iter().flat_map(|run| &run.glyphs) {
            max_index = max_index.max(glyph.index);
            while pending.next_if(|&&ix| ix <= max_index).is_some() {
                xs.push(glyph.position.x);
            }
            if pending.peek().is_none() {
                return xs;
            }
        }
        xs.extend(pending.map(|_| line.width));
        xs
    }
    pub(crate) fn x_for_index(&self, ix: usize) -> Pixels {
        match &self.content {
            Content::Text(line) => line.x_for_index(ix),
            Content::Inline(fragments) => {
                for f in fragments {
                    if ix < f.range.end {
                        return f.x
                            + f.text.as_ref().map_or(
                                if ix <= f.range.start { px(0.) } else { f.width },
                                |line| line.x_for_index(ix.saturating_sub(f.range.start)),
                            );
                    }
                }
                self.width
            }
        }
    }
    pub(crate) fn closest_index_for_x(&self, x: Pixels) -> usize {
        match &self.content {
            Content::Text(line) => line.closest_index_for_x(x),
            Content::Inline(fragments) => {
                for f in fragments {
                    if x <= f.x + f.width {
                        return f.text.as_ref().map_or(
                            if x - f.x < f.width / 2. {
                                f.range.start
                            } else {
                                f.range.end
                            },
                            |line| f.range.start + line.closest_index_for_x(x - f.x),
                        );
                    }
                }
                self.len
            }
        }
    }
    pub(crate) fn index_for_x(&self, x: Pixels) -> Option<usize> {
        match &self.content {
            Content::Text(line) => line.index_for_x(x),
            Content::Inline(_) => {
                (x >= px(0.) && x <= self.width).then(|| self.closest_index_for_x(x))
            }
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        &self,
        pos: Point<Pixels>,
        height: Pixels,
        align: TextAlign,
        width: Option<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.paint_pass(pos, height, align, width, false, window, cx);
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_background(
        &self,
        pos: Point<Pixels>,
        height: Pixels,
        align: TextAlign,
        width: Option<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.paint_pass(pos, height, align, width, true, window, cx);
    }
    #[allow(clippy::too_many_arguments)]
    fn paint_pass(
        &self,
        pos: Point<Pixels>,
        height: Pixels,
        align: TextAlign,
        width: Option<Pixels>,
        background: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let paint = |line: &ShapedLine, pos, align, width, window: &mut Window, cx: &mut App| {
            if background {
                let _ = line.paint_background(pos, height, align, width, window, cx);
            } else {
                let _ = line.paint(pos, height, align, width, window, cx);
            }
        };
        match &self.content {
            Content::Text(line) => paint(line, pos, align, width, window, cx),
            Content::Inline(fragments) => {
                let remaining = (width.unwrap_or(self.width) - self.width).max(px(0.));
                let offset = match align {
                    TextAlign::Right => remaining,
                    TextAlign::Center => remaining / 2.,
                    _ => px(0.),
                };
                for f in fragments {
                    if let Some(line) = &f.text {
                        paint(
                            line,
                            pos + point(offset + f.x, px(0.)),
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                    }
                }
            }
        }
    }
}
