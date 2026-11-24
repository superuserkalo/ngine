use rune::tok::{self, Theme};
use rune::{Color, oklch};

/// The named colours every screen is drawn with. A desktop theme recolours
/// ngine by handing [`super::widgets::Paint::from`] another `Palette`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub background: Color,
    pub sidebar: Color,
    pub surface: Color,
    pub raised: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub on_accent: Color,
    pub recording: Color,
    pub warning: Color,
    pub good: Color,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            background: oklch(0.185, 0.004, 265.0),
            sidebar: oklch(0.208, 0.004, 265.0),
            surface: oklch(0.232, 0.005, 265.0),
            raised: oklch(0.262, 0.006, 265.0),
            border: Color::WHITE.alpha(0.08),
            text: oklch(0.955, 0.003, 265.0),
            muted: oklch(0.72, 0.008, 265.0),
            accent: tok::accent(0, true),
            on_accent: Color::WHITE,
            recording: oklch(0.64, 0.2, 25.0),
            warning: oklch(0.8, 0.14, 80.0),
            good: tok::accent(4, true),
        }
    }

    /// The control colours Rune's widgets read, made from these roles.
    pub fn theme(&self) -> Theme {
        let base = tok::theme(true);
        Theme {
            bg: self.background,
            card: self.surface,
            card_border: self.border,
            text: self.text,
            text_secondary: self.muted,
            separator: self.border,
            control_track: self.text.alpha(0.05),
            control_off: self.text.alpha(0.2),
            indicator: self.raised,
            hover: self.text.alpha(0.03),
            press: self.text.alpha(0.05),
            ..base
        }
    }
}
