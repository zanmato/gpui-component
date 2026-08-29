use gpui::{
    App, AppContext as _, Context, Empty, Entity, HighlightStyle, IntoElement, ParentElement,
    Pixels, Point, Render, SharedString, Styled, StyledText, WeakEntity, Window, deferred, div,
    prelude::FluentBuilder, px,
};
use lsp_types::{ParameterLabel, SignatureHelp, SignatureInformation};

use crate::{
    ActiveTheme,
    input::{
        EditorState, RopeExt as _,
        popovers::{editor_popover, render_markdown},
    },
    label::Label,
    v_flex,
};

const MAX_WIDTH: Pixels = px(480.);
const POPOVER_GAP: Pixels = px(4.);

/// The popover showing the active call's signature, anchored above the
/// cursor, with the active parameter emphasized.
pub struct SignatureHelpPopover {
    editor: WeakEntity<EditorState>,
    help: Option<SignatureHelp>,
    open: bool,
}

impl SignatureHelpPopover {
    /// NOTE: This element should not be created from EditorState::new,
    /// unless that will stack overflow.
    pub(crate) fn new(editor: Entity<EditorState>, _: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self {
            editor: editor.downgrade(),
            help: None,
            open: false,
        })
    }

    pub(crate) fn show(&mut self, help: SignatureHelp, cx: &mut Context<Self>) {
        self.help = Some(help);
        self.open = true;
        cx.notify();
    }

    pub(crate) fn hide(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    /// Where the popover's bottom-left corner goes: just above the cursor.
    fn origin(&self, cx: &App) -> Option<Point<Pixels>> {
        let editor = self.editor.upgrade()?;
        let editor = editor.read(cx);
        let (cursor_bounds, _) = editor.cursor_layout()?;
        let scroll_origin = editor.scroll_offset();
        Some(scroll_origin + cursor_bounds.origin - editor.input_bounds().origin)
    }

    fn active_signature(help: &SignatureHelp) -> Option<&SignatureInformation> {
        let index = help.active_signature.unwrap_or(0) as usize;
        help.signatures
            .get(index)
            .or_else(|| help.signatures.first())
    }

    /// The byte range of the active parameter inside the signature label.
    fn active_parameter_range(
        help: &SignatureHelp,
        signature: &SignatureInformation,
    ) -> Option<std::ops::Range<usize>> {
        let index = signature.active_parameter.or(help.active_parameter)? as usize;
        let parameter = signature.parameters.as_ref()?.get(index)?;
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return Empty.into_any_element();
        }
        let Some(help) = self.help.clone() else {
            return Empty.into_any_element();
        };
        let Some(signature) = Self::active_signature(&help).cloned() else {
            return Empty.into_any_element();
        };
        let Some(pos) = self.origin(cx) else {
            return Empty.into_any_element();
        };
        let Some(editor) = self.editor.upgrade() else {
            return Empty.into_any_element();
        };

        let container_height = editor.read(cx).input_bounds().size.height;
        let highlights = Self::active_parameter_range(&help, &signature)
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
        let extra_signatures = help.signatures.len().saturating_sub(1);

        deferred(
            div()
                .absolute()
                .left(pos.x)
                .bottom(container_height - pos.y + POPOVER_GAP)
                .child(
                    editor_popover("signature-help", cx)
                        .max_w(MAX_WIDTH)
                        .px_2()
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    StyledText::new(signature.label.clone())
                                        .with_highlights(highlights),
                                )
                                .when(extra_signatures > 0, |this| {
                                    this.child(
                                        Label::new(SharedString::from(format!(
                                            "+{extra_signatures} more overloads"
                                        )))
                                        .text_color(cx.theme().muted_foreground),
                                    )
                                })
                                .when_some(documentation, |this, documentation| {
                                    this.child(render_markdown(
                                        "signature-doc",
                                        documentation,
                                        window,
                                        cx,
                                    ))
                                }),
                        ),
                ),
        )
        .into_any_element()
    }
}
