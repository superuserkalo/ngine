use rune::tok::{self, space, type_};
use rune::{Align, Color, Cx, El, OverlaySpec, ScreenEdge, el, oklch, row, text};

pub const SIZE: (f32, f32) = (320.0, 48.0);
const PILL_HEIGHT: f32 = 36.0;
const DOT: f32 = 8.0;
const EDGE_MARGIN: f32 = 24.0;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Toast {
    pub text: String,
}

pub fn spec() -> OverlaySpec<Toast> {
    OverlaySpec {
        name: "ngine-saved",
        size: SIZE,
        edge: ScreenEdge::Top,
        margin: EDGE_MARGIN,
        align: Align::Center,
        inset: 0.0,
        interactive: false,
        view,
    }
}

pub fn view(toast: &Toast, _: &mut Cx<Toast>) -> El {
    let theme = tok::theme(true);
    row().key("toast").w_full().h_full().center().child(
        row()
            .key("toast-pill")
            .role("status")
            .label(toast.text.clone())
            .h(PILL_HEIGHT)
            .px(space::S3 + space::S1)
            .gap(space::S2)
            .center()
            .radius(PILL_HEIGHT / 2.0)
            .fill(oklch(0.2, 0.004, 265.0))
            .border(1.0, Color::WHITE.alpha(0.12))
            .child(
                el().key("toast-dot")
                    .size(DOT, DOT)
                    .radius(DOT / 2.0)
                    .fill(tok::accent(4, true))
                    .a11y_hidden(),
            )
            .child(
                text(toast.text.clone())
                    .key("toast-text")
                    .font(type_::LABEL)
                    .color(theme.text)
                    .a11y_hidden(),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn the_overlay_shows_the_saved_length() {
        let toast = Toast {
            text: "Replay saved · 31 s".into(),
        };
        let mut runtime = Runtime::new(toast, view, SIZE, Env::new(true, 2.0));
        runtime.frame(0.0);
        let shown = runtime
            .elements()
            .into_iter()
            .find(|element| element.key.as_deref() == Some("toast-text"))
            .and_then(|element| element.text);
        assert_eq!(shown.as_deref(), Some("Replay saved · 31 s"));
    }
}
