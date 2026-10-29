use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, Empty, Entity, Hsla, IntoElement, Render, Styled, WeakEntity,
    Window,
};

use crate::{
    ActiveTheme as _,
    highlighter::DiagnosticEntry,
    input::{
        EditorState,
        popovers::{Popover, render_markdown},
    },
};

pub struct DiagnosticPopover {
    state: WeakEntity<EditorState>,
    pub(crate) diagnostic: Rc<DiagnosticEntry>,
}

impl DiagnosticPopover {
    pub fn new(
        diagnostic: &DiagnosticEntry,
        state: Entity<EditorState>,
        cx: &mut App,
    ) -> Entity<Self> {
        let diagnostic = Rc::new(diagnostic.clone());

        cx.new(|_| Self {
            diagnostic,
            state: state.downgrade(),
        })
    }
}

/// Themes give the status backgrounds as translucent tints meant to sit under
/// text, so the popover lays its tint over the opaque popover color, or the
/// code beneath would show through.
fn opaque_background(tint: Hsla, cx: &App) -> Hsla {
    let theme = cx.theme();
    theme.background.blend(theme.popover).blend(tint)
}

impl Render for DiagnosticPopover {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(state) = self.state.upgrade() else {
            return Empty.into_any_element();
        };
        let message = self.diagnostic.message.clone();

        let (border, bg, fg) = (
            crate::highlighter::diagnostic_border(self.diagnostic.severity, cx),
            opaque_background(
                crate::highlighter::diagnostic_background(self.diagnostic.severity, cx),
                cx,
            ),
            crate::highlighter::diagnostic_foreground(self.diagnostic.severity, cx),
        );

        Popover::new(
            "diagnostic-popover",
            state,
            self.diagnostic.range.clone(),
            move |window, cx| render_markdown("message", message.clone(), window, cx),
        )
        .px_1()
        .py_0p5()
        .bg(bg)
        .text_color(fg)
        .border_1()
        .border_color(border)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;
    use crate::highlighter::{DiagnosticSeverity, diagnostic_background};

    #[gpui::test]
    fn the_background_hides_the_text_beneath(cx: &mut TestAppContext) {
        cx.update(crate::init);
        cx.update(|cx| {
            for severity in [
                DiagnosticSeverity::Error,
                DiagnosticSeverity::Warning,
                DiagnosticSeverity::Info,
                DiagnosticSeverity::Hint,
            ] {
                let tint = diagnostic_background(severity, cx);
                assert!(tint.a < 1., "{severity:?} has a translucent tint");
                assert_eq!(opaque_background(tint, cx).a, 1., "{severity:?}");
            }
        });
    }
}
