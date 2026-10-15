use gpui::{
    AnyElement, App, AppContext as _, AvailableSpace, Bounds, Context, Element, ElementId, Empty,
    Entity, HighlightStyle, IntoElement, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled, StyledText, WeakEntity, Window, deferred, div, point,
    prelude::FluentBuilder, px,
};
use lsp_types::{ParameterLabel, SignatureInformation};

use crate::{
    ActiveTheme,
    input::{
        EditorState, RopeExt as _, SignatureHelpState,
        popovers::{editor_popover, render_markdown},
    },
    label::Label,
    v_flex,
};

const MAX_WIDTH: Pixels = px(480.);
const MAX_HEIGHT: Pixels = px(320.);
const POPOVER_GAP: Pixels = px(4.);
/// The closest the popover comes to the window's edges.
const WINDOW_MARGIN: Pixels = px(8.);

/// The popover showing the active call's signature, anchored above the
/// cursor, with the active parameter emphasized.
pub struct SignatureHelpPopover {
    editor: WeakEntity<EditorState>,
    state: SignatureHelpState,
    open: bool,
}

impl SignatureHelpPopover {
    /// NOTE: This element should not be created from EditorState::new,
    /// unless that will stack overflow.
    pub(crate) fn new(editor: Entity<EditorState>, _: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self {
            editor: editor.downgrade(),
            state: SignatureHelpState::default(),
            open: false,
        })
    }

    pub(crate) fn show(&mut self, state: SignatureHelpState, cx: &mut Context<Self>) {
        self.state = state;
        self.open = true;
        cx.notify();
    }

    pub(crate) fn hide(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    /// The byte range of the active parameter inside the signature label.
    fn active_parameter_range(
        signature: &SignatureInformation,
        index: Option<usize>,
    ) -> Option<std::ops::Range<usize>> {
        let parameter = signature.parameters.as_ref()?.get(index?)?;
        match &parameter.label {
            ParameterLabel::Simple(text) => {
                let start = signature.label.find(text.as_str())?;
                Some(start..start + text.len())
            }
            ParameterLabel::LabelOffsets([start, end]) => {
                // Label offsets are UTF-16 code units into the label.
                let label = crate::input::Rope::from(signature.label.as_str());
                Some(
                    label.offset_utf16_to_offset(*start as usize)
                        ..label.offset_utf16_to_offset(*end as usize),
                )
            }
        }
    }
}

impl Render for SignatureHelpPopover {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return Empty.into_any_element();
        }
        let Some(help) = self.state.help.clone() else {
            return Empty.into_any_element();
        };
        let Some(index) = self.state.active_signature() else {
            return Empty.into_any_element();
        };
        let Some(signature) = help.signatures.get(index).cloned() else {
            return Empty.into_any_element();
        };
        let Some(editor) = self.editor.upgrade() else {
            return Empty.into_any_element();
        };
        let Some((cursor_bounds, line_height)) = editor.read(cx).cursor_layout() else {
            return Empty.into_any_element();
        };

        let highlights = Self::active_parameter_range(&signature, self.state.active_parameter())
            .into_iter()
            .map(|range| {
                (
                    range,
                    HighlightStyle {
                        color: Some(cx.theme().blue),
                        font_weight: Some(gpui::FontWeight::BOLD),
                        ..Default::default()
                    },
                )
            })
            .collect::<Vec<_>>();
        let documentation = signature.documentation.as_ref().map(|doc| match doc {
            lsp_types::Documentation::String(text) => text.clone(),
            lsp_types::Documentation::MarkupContent(markup) => markup.value.clone(),
        });
        let overloads = help.signatures.len();
        let editor = editor.read(cx);

        SignatureHelpPlacement {
            cursor: Bounds::new(
                editor.scroll_offset() + cursor_bounds.origin,
                gpui::size(cursor_bounds.size.width, line_height),
            ),
            keep_above: editor.completion_menu_state().open,
            content: Box::new(move |window, cx| {
                v_flex()
                    .gap_1()
                    .child(
                        StyledText::new(signature.label.clone())
                            .with_highlights(highlights.clone()),
                    )
                    .when(overloads > 1, |this| {
                        this.child(
                            Label::new(SharedString::from(format!(
                                "{} of {overloads} overloads, ctrl-shift-up/down to cycle",
                                index + 1
                            )))
                            .text_color(cx.theme().muted_foreground),
                        )
                    })
                    .when_some(documentation.clone(), |this, documentation| {
                        this.child(render_markdown("signature-doc", documentation, window, cx))
                    })
                    .into_any_element()
            }),
        }
        .into_any_element()
    }
}

/// Places the popover next to the cursor after measuring it: above the
/// cursor when it fits there, otherwise on the side with more room, its
/// height capped to that room so the rest scrolls instead of leaving the
/// window.
struct SignatureHelpPlacement {
    /// The cursor's line, in window coordinates.
    cursor: Bounds<Pixels>,
    /// The completion menu opens below the cursor, so while it is open the
    /// popover stays above rather than covering it.
    keep_above: bool,
    content: Box<dyn Fn(&mut Window, &mut App) -> AnyElement>,
}

impl SignatureHelpPlacement {
    fn popover(&self, max_height: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
        let max_width = MAX_WIDTH.min(window.bounds().size.width - WINDOW_MARGIN * 2);
        deferred(
            editor_popover("signature-help", cx)
                .max_w(max_width)
                .max_h(max_height)
                .overflow_y_scroll()
                .px_2()
                .child((self.content)(window, cx)),
        )
        .into_any_element()
    }
}

#[derive(Debug, PartialEq)]
struct Side {
    above: bool,
    /// The height available on that side.
    room: Pixels,
}

fn choose_side(
    cursor: Bounds<Pixels>,
    window: gpui::Size<Pixels>,
    height: Pixels,
    keep_above: bool,
) -> Side {
    let space_above = (cursor.top() - POPOVER_GAP - WINDOW_MARGIN).max(px(0.));
    let space_below = (window.height - cursor.bottom() - POPOVER_GAP - WINDOW_MARGIN).max(px(0.));
    if height <= space_above || keep_above || space_above >= space_below {
        Side {
            above: true,
            room: space_above,
        }
    } else {
        Side {
            above: false,
            room: space_below,
        }
    }
}

/// The popover's top-left corner, kept inside the window horizontally.
fn popover_origin(
    cursor: Bounds<Pixels>,
    window: gpui::Size<Pixels>,
    size: gpui::Size<Pixels>,
    above: bool,
) -> gpui::Point<Pixels> {
    let y = if above {
        cursor.top() - POPOVER_GAP - size.height
    } else {
        cursor.bottom() + POPOVER_GAP
    };
    let x = cursor
        .left()
        .min(window.width - WINDOW_MARGIN - size.width)
        .max(WINDOW_MARGIN);
    point(x, y)
}

impl IntoElement for SignatureHelpPlacement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

struct PlacementLayout {
    origin: gpui::Point<Pixels>,
    popover: AnyElement,
}

impl Element for SignatureHelpPlacement {
    type RequestLayoutState = PlacementLayout;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        let window_size = window.bounds().size;
        let mut popover = self.popover(MAX_HEIGHT, window, cx);
        let mut size = popover.layout_as_root(AvailableSpace::min_size(), window, cx);

        let side = choose_side(self.cursor, window_size, size.height, self.keep_above);
        if size.height > side.room {
            popover = self.popover(side.room, window, cx);
            size = popover.layout_as_root(AvailableSpace::min_size(), window, cx);
        }

        let layout_id = div().into_any_element().request_layout(window, cx);
        (
            layout_id,
            PlacementLayout {
                origin: popover_origin(self.cursor, window_size, size, side.above),
                popover,
            },
        )
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        window.with_absolute_element_offset(layout.origin, |window| {
            layout.popover.prepaint(window, cx);
        })
    }

    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        layout.popover.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    const WINDOW: gpui::Size<Pixels> = gpui::Size {
        width: px(1000.),
        height: px(800.),
    };

    fn cursor_at(x: f32, y: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(2.), px(20.)))
    }

    #[test]
    fn opens_above_when_it_fits() {
        let side = choose_side(cursor_at(100., 400.), WINDOW, px(120.), false);
        assert!(side.above);
        assert_eq!(side.room, px(400.) - POPOVER_GAP - WINDOW_MARGIN);
    }

    #[test]
    fn flips_below_when_the_room_above_is_short() {
        let side = choose_side(cursor_at(100., 60.), WINDOW, px(300.), false);
        assert!(!side.above);
        assert_eq!(side.room, px(800. - 80.) - POPOVER_GAP - WINDOW_MARGIN);
    }

    #[test]
    fn stays_above_over_an_open_completion_menu() {
        let side = choose_side(cursor_at(100., 60.), WINDOW, px(300.), true);
        assert!(side.above);
        assert_eq!(side.room, px(60.) - POPOVER_GAP - WINDOW_MARGIN);
    }

    #[test]
    fn takes_the_larger_side_when_neither_fits() {
        let side = choose_side(cursor_at(100., 500.), WINDOW, px(900.), false);
        assert!(side.above, "500px above beats 280px below");
    }

    #[test]
    fn origin_stays_inside_the_window() {
        let popover = size(px(480.), px(100.));
        let above = popover_origin(cursor_at(900., 400.), WINDOW, popover, true);
        assert_eq!(above.x, px(1000. - 480.) - WINDOW_MARGIN);
        assert_eq!(above.y, px(400. - 100.) - POPOVER_GAP);

        let below = popover_origin(cursor_at(2., 60.), WINDOW, popover, false);
        assert_eq!(below.x, WINDOW_MARGIN);
        assert_eq!(below.y, px(80.) + POPOVER_GAP);
    }
}
