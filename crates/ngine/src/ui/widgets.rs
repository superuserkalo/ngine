use rune::tok::{TextStyle, Theme, motion, radius, space, type_};
use rune::{
    Align, Color, Drag, El, Icon, Phase, ScrollSpec, Side, TextEdit, Tx, col, el, icon, row, text,
    text_input,
};

use super::icons;
use super::palette::Palette;
use crate::app::{Ngine, Popup};

pub const TITLE: TextStyle = TextStyle::new(15.0, 600.0);
pub const HEADING: TextStyle = TextStyle::new(14.0, 600.0);
pub const TIMER: TextStyle = TextStyle::new(22.0, 500.0).features("tnum");
pub const CAPTION: TextStyle = TextStyle::new(11.5, 400.0);
pub const STRONG: TextStyle = TextStyle::new(13.0, 600.0);
pub const KEYCAP: TextStyle = TextStyle::new(11.0, 500.0);
pub const NUMBERS: TextStyle = TextStyle::new(12.0, 500.0).features("tnum");

pub const CONTROL: f32 = 30.0;
pub const LARGE_CONTROL: f32 = 34.0;
pub const ICON: f32 = 15.0;
const ROW_HEIGHT: f32 = 42.0;
const SWITCH: (f32, f32) = (36.0, 20.0);
const KNOB: f32 = 16.0;
const SLIDER_BAR: f32 = 4.0;
const SEGMENT_PAD: f32 = 2.0;
const KEYCAP_SIZE: f32 = 20.0;
const MENU_ITEM: f32 = 30.0;
const MENU_WIDTH: f32 = 220.0;
const POPUP_GAP: f32 = 4.0;
const DIALOG_WIDTH: f32 = 380.0;
const LIST_HEIGHT: f32 = 300.0;
const METER: f32 = 4.0;
const DISABLED: f32 = 0.42;

#[derive(Clone, Copy)]
pub struct Paint {
    pub t: Theme,
    pub accent: Color,
    pub on_accent: Color,
    pub danger: Color,
    pub warning: Color,
    pub good: Color,
    pub sidebar: Color,
    pub raised: Color,
    pub outline: Color,
    pub faint: Color,
    pub hair: f32,
}

impl Paint {
    pub fn from(palette: &Palette, scale: f32) -> Self {
        let t = palette.theme();
        Self {
            t,
            accent: palette.accent,
            on_accent: palette.on_accent,
            danger: palette.recording,
            warning: palette.warning,
            good: palette.good,
            sidebar: palette.sidebar,
            raised: palette.raised,
            outline: palette.border,
            faint: palette.muted.alpha(0.72),
            hair: 1.0 / scale,
        }
    }

    pub fn dark(scale: f32) -> Self {
        Self::from(&Palette::dark(), scale)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Primary,
    Danger,
    Plain,
    Quiet,
}

/// A push button. One that cannot be pressed says why when the pointer rests
/// on it, and assistive technology reads the reason as its description.
pub struct Button<'a> {
    key: &'a str,
    label: &'a str,
    icon: Option<&'static Icon>,
    look: Look,
    blocked: Option<String>,
    keys: Vec<String>,
    height: f32,
    help: Option<String>,
    only_icon: bool,
}

impl<'a> Button<'a> {
    pub fn new(key: &'a str, label: &'a str) -> Self {
        Self {
            key,
            label,
            icon: None,
            look: Look::Plain,
            blocked: None,
            keys: Vec::new(),
            height: CONTROL,
            help: None,
            only_icon: false,
        }
    }

    pub fn icon(mut self, icon: &'static Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn only_icon(mut self, icon: &'static Icon) -> Self {
        self.icon = Some(icon);
        self.only_icon = true;
        self
    }

    pub fn look(mut self, look: Look) -> Self {
        self.look = look;
        self
    }

    pub fn large(mut self) -> Self {
        self.height = LARGE_CONTROL;
        self
    }

    pub fn blocked(mut self, why: Option<String>) -> Self {
        self.blocked = why;
        self
    }

    pub fn keys(mut self, keys: Option<Vec<String>>) -> Self {
        self.keys = keys.unwrap_or_default();
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn build_tx(self, p: &Paint, on_press: impl Fn(&mut Ngine, &mut Tx) + 'static) -> El {
        let (fill, ink) = match self.look {
            Look::Primary => (p.accent, p.on_accent),
            Look::Danger => (p.danger, p.on_accent),
            Look::Plain => (p.t.control_off, p.t.text),
            Look::Quiet => (Color::TRANSPARENT, p.t.text_secondary),
        };
        let large = self.height > CONTROL;
        let mut base = row()
            .key(self.key)
            .role("button")
            .label(self.label)
            .h(self.height)
            .center()
            .gap(if large {
                space::S2 + 2.0
            } else {
                space::S1 + 2.0
            })
            .radius(if large { radius::LG } else { radius::MD })
            .fill(fill);
        base = if self.only_icon {
            base.w(self.height)
        } else {
            base.px(if large { space::S6 - 4.0 } else { space::S3 })
        };
        if let Some(glyph) = self.icon {
            let size = if large { ICON + 3.0 } else { ICON };
            base = base.child(icon(glyph, size).color(ink).a11y_hidden());
        }
        if !self.only_icon {
            let font = if large { STRONG } else { type_::LABEL };
            base = base.child(text(self.label).font(font).color(ink).a11y_hidden());
        }
        if !self.keys.is_empty() {
            base = base.child(keycaps(p, &format!("{}-keys", self.key), &self.keys));
        }
        if let Some(why) = self.blocked {
            base.disabled(true)
                .focusable(false)
                .opacity(DISABLED)
                .description(why.clone())
                .help(why)
        } else {
            let help = self
                .help
                .or_else(|| self.only_icon.then(|| self.label.to_owned()));
            let pressable = base
                .hover(p.t.text.alpha(0.07))
                .press(Color::BLACK.alpha(0.16))
                .focusable(true)
                .on_press_tx(on_press);
            if let Some(help) = help {
                pressable.help(help)
            } else {
                pressable
            }
        }
    }

    pub fn build(self, p: &Paint, on_press: impl Fn(&mut Ngine) + 'static) -> El {
        self.build_tx(p, move |app, _| on_press(app))
    }
}

pub fn keycap(p: &Paint, key: String, cap: &str) -> El {
    el().key(key)
        .min_w(KEYCAP_SIZE)
        .h(KEYCAP_SIZE)
        .px(5.0)
        .center()
        .radius(5.0)
        .fill(p.t.text.alpha(0.09))
        .border(p.hair, p.t.text.alpha(0.14))
        .child(text(cap).font(KEYCAP).color(p.t.text))
}

/// The keys of a hotkey, each drawn as the cap of a key.
pub fn keycaps(p: &Paint, key: &str, caps: &[String]) -> El {
    let mut line = row()
        .key(key)
        .role("group")
        .label(caps.join(" "))
        .gap(3.0)
        .center_y();
    for (index, cap) in caps.iter().enumerate() {
        line = line.child(keycap(p, format!("{key}-{index}"), cap).a11y_hidden());
    }
    line
}

pub fn caption(p: &Paint, key: &str, words: impl Into<String>) -> El {
    text(words)
        .key(key)
        .font(CAPTION)
        .color(p.t.text_secondary)
        .wrap()
}

pub fn glyph(p: &Paint, key: &str, glyph: &'static Icon, size: f32) -> El {
    icon(glyph, size)
        .key(key)
        .color(p.t.text_secondary)
        .a11y_hidden()
}

pub fn switch(p: &Paint, key: &str, on: bool) -> El {
    let (width, height) = SWITCH;
    let inset = (height - KNOB) / 2.0;
    el().key(key)
        .size(width, height)
        .radius(height / 2.0)
        .fill(if on { p.accent } else { p.t.control_off })
        .motion(motion::SNAPPY)
        .a11y_hidden()
        .child(
            el().key(format!("{key}-knob"))
                .size(KNOB, KNOB)
                .absolute(if on { width - KNOB - inset } else { inset }, inset)
                .radius(KNOB / 2.0)
                .fill(p.t.knob)
                .shadow(1.0, 3.0, p.t.knob_shadow),
        )
}

/// A switch that stands alone, with its label read by assistive technology.
pub fn lone_switch(
    p: &Paint,
    key: &str,
    label: &str,
    on: bool,
    flip: impl Fn(&mut Ngine) + 'static,
) -> El {
    row()
        .key(key)
        .role("switch")
        .label(label)
        .checked(on)
        .radius(SWITCH.1 / 2.0)
        .focusable(true)
        .on_press(flip)
        .child(switch(p, &format!("{key}-switch"), on))
}

pub struct Choice {
    pub id: &'static str,
    pub label: String,
}

impl Choice {
    pub fn new(id: &'static str, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
        }
    }
}

pub struct Segments<'a> {
    pub key: &'a str,
    pub label: &'a str,
    pub choices: Vec<Choice>,
    pub selected: Option<usize>,
    pub segment_width: f32,
}

pub fn segmented(
    p: &Paint,
    segments: Segments<'_>,
    pick: impl Fn(&mut Ngine, usize) + Clone + 'static,
) -> El {
    let Segments {
        key,
        label,
        choices,
        selected,
        segment_width,
    } = segments;
    let inner = CONTROL - 2.0 * SEGMENT_PAD;
    let mut strip = row().key(format!("{key}-segments"));
    let mut left = SEGMENT_PAD;
    let mut indicator_left = None;
    for (index, choice) in choices.into_iter().enumerate() {
        let chosen = selected == Some(index);
        if chosen {
            indicator_left = Some(left);
        }
        left += segment_width;
        let ink = if chosen { p.t.text } else { p.t.text_secondary };
        let pick = pick.clone();
        strip = strip.child(
            el().key(format!("{key}.{}", choice.id))
                .role("segment")
                .label(choice.label.clone())
                .checked(chosen)
                .size(segment_width, inner)
                .center()
                .radius(radius::SM)
                .focusable(true)
                .hover(if chosen {
                    Color::TRANSPARENT
                } else {
                    p.t.hover
                })
                .on_press(move |app: &mut Ngine| pick(app, index))
                .child(
                    text(choice.label)
                        .font(type_::LABEL)
                        .color(ink)
                        .a11y_hidden(),
                ),
        );
    }
    let mut control = el()
        .key(key)
        .role("segmented")
        .label(label)
        .focus_group()
        .size(left + SEGMENT_PAD, CONTROL)
        .padding(SEGMENT_PAD)
        .radius(radius::MD)
        .fill(p.t.control_track)
        .motion(motion::SNAPPY);
    if let Some(indicator_left) = indicator_left {
        control = control.child(
            el().key(format!("{key}-indicator"))
                .size(segment_width, inner)
                .absolute(indicator_left, SEGMENT_PAD)
                .radius(radius::SM)
                .fill(p.t.indicator)
                .shadow(1.0, 3.0, p.t.shadow)
                .a11y_hidden(),
        );
    }
    control.child(strip)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the fraction is clamped to 0..=1, so the rounded product is between 0 and the last stop"
)]
fn nearest_stop(fraction: f32, stops: u16) -> u16 {
    let last = stops.saturating_sub(1);
    (fraction.clamp(0.0, 1.0) * f32::from(last)).round() as u16
}

pub struct Stops<'a> {
    pub key: &'a str,
    pub label: &'a str,
    pub index: u16,
    pub count: u16,
    pub value: String,
    pub width: f32,
}

/// A slider over a few stops. Its value is read out next to it.
pub fn slider(p: &Paint, stops: Stops<'_>, set: impl Fn(&mut Ngine, u16) + Clone + 'static) -> El {
    let Stops {
        key,
        label,
        index,
        count,
        value,
        width,
    } = stops;
    let last = f32::from(count.saturating_sub(1).max(1));
    let travel = width - KNOB;
    let fraction = f32::from(index) / last;
    let stop_at = move |x: f32| nearest_stop((x - KNOB / 2.0) / travel, count);
    let (drag_set, step_set) = (set.clone(), set);
    let bar_top = (SWITCH.1 - SLIDER_BAR) / 2.0;
    let track = el()
        .key(key)
        .role("slider")
        .label(label)
        .range_value(f64::from(index), 0.0, f64::from(last))
        .range_step(1.0)
        .a11y_value(value.clone())
        .focusable(true)
        .size(width, SWITCH.1)
        .motion(motion::SNAPPY)
        .on_step(move |app: &mut Ngine, steps, _| {
            let stepped = i32::from(index).saturating_add(steps);
            let bounded = stepped.clamp(0, i32::from(count.saturating_sub(1)));
            step_set(app, u16::try_from(bounded).unwrap_or(index));
        })
        .on_drag(
            move |app: &mut Ngine, drag: &mut Drag, tx| match drag.phase {
                Phase::Down => {
                    drag.data = f32::from(index);
                    drag_set(app, stop_at(drag.x));
                }
                Phase::Move => {
                    drag_set(app, stop_at(drag.x));
                    tx.instant();
                }
                Phase::Up => {
                    if drag.cancelled() {
                        drag_set(app, nearest_stop(drag.data / last, count));
                    }
                }
            },
        )
        .child(
            el().key(format!("{key}-track"))
                .size(width, SLIDER_BAR)
                .absolute(0.0, bar_top)
                .radius(SLIDER_BAR / 2.0)
                .fill(p.t.control_off)
                .a11y_hidden(),
        )
        .child(
            el().key(format!("{key}-fill"))
                .size(KNOB / 2.0 + fraction * travel, SLIDER_BAR)
                .absolute(0.0, bar_top)
                .radius(SLIDER_BAR / 2.0)
                .fill(p.accent)
                .a11y_hidden(),
        )
        .child(
            el().key(format!("{key}-knob"))
                .size(KNOB, KNOB)
                .absolute(fraction * travel, (SWITCH.1 - KNOB) / 2.0)
                .radius(KNOB / 2.0)
                .fill(p.t.knob)
                .shadow(1.0, 3.0, p.t.knob_shadow)
                .a11y_hidden(),
        );
    row()
        .key(format!("{key}-control"))
        .gap(space::S2)
        .center_y()
        .child(track)
        .child(
            text(value)
                .key(format!("{key}-value"))
                .font(NUMBERS)
                .color(p.t.text_secondary)
                .w(40.0)
                .a11y_hidden(),
        )
}

/// A level meter: how loud a source is now, or an empty bar when nothing is
/// being captured.
pub fn meter(p: &Paint, key: &str, label: &str, level: Option<u8>) -> El {
    let shown = f32::from(level.unwrap_or(0).min(100)) / 100.0;
    let colour = match level.unwrap_or(0) {
        0..=79 => p.good,
        80..=94 => p.warning,
        _ => p.danger,
    };
    let value = level.map_or_else(
        || "Shown while capturing".to_owned(),
        |level| format!("{level} %"),
    );
    el().key(key)
        .role("meter")
        .label(label)
        .range_value(f64::from(level.unwrap_or(0)), 0.0, 100.0)
        .a11y_value(value.clone())
        .help(value)
        .w_full()
        .h(METER)
        .radius(METER / 2.0)
        .fill(p.t.control_off)
        .child(
            el().key(format!("{key}-fill"))
                .w_frac(shown)
                .h(METER)
                .radius(METER / 2.0)
                .fill(colour)
                .motion(motion::SNAPPY)
                .a11y_hidden(),
        )
}

pub struct Field<'a> {
    pub key: &'a str,
    pub label: &'a str,
    pub edit: &'a TextEdit,
    pub placeholder: &'a str,
    pub icon: Option<&'static Icon>,
}

/// A one-line text field. `access` finds its text in the app; `changed` runs
/// after every edit.
pub fn field(
    p: &Paint,
    field: &Field<'_>,
    access: impl for<'s> Fn(&'s mut Ngine) -> &'s mut TextEdit + 'static,
    changed: impl Fn(&mut Ngine) + 'static,
) -> El {
    let editor = text_input(field.edit, type_::BODY)
        .key(field.key)
        .label(field.label)
        .single_line()
        .color(p.t.text)
        .placeholder(field.placeholder, p.faint)
        .grow(1.0)
        .min_w(0.0)
        .on_edit(access)
        .on_input_change(move |app: &mut Ngine, _| changed(app));
    let mut frame = row()
        .key(format!("{}-box", field.key))
        .h(CONTROL)
        .px(space::S2)
        .gap(space::S1 + 2.0)
        .radius(radius::MD)
        .fill(p.t.control_track)
        .border(p.hair, p.outline)
        .focus_within_ring(1.5, -1.0, p.accent)
        .center_y();
    if let Some(glyph) = field.icon {
        frame = frame.child(icon(glyph, ICON).color(p.faint).a11y_hidden());
    }
    frame.child(editor)
}

pub fn tag(p: &Paint, key: &str, words: &str, colour: Color) -> El {
    el().key(key)
        .label(words)
        .h(18.0)
        .px(6.0)
        .center()
        .radius(5.0)
        .fill(colour.alpha(0.16))
        .child(text(words).font(KEYCAP).color(colour.alpha(0.95)))
        .border(p.hair, colour.alpha(0.28))
}

/// One row of a settings card: what it is, what choosing it means, and the
/// control.
pub fn setting(p: &Paint, key: &str, label: &str, hint: Option<String>, control: El) -> El {
    let hint = hint.map(|hint| caption(p, &format!("{key}-hint"), hint));
    labelled(p, key, label, hint, control)
}

pub fn troubled_setting(p: &Paint, key: &str, label: &str, trouble: String, control: El) -> El {
    let hint = caption(p, &format!("{key}-hint"), trouble)
        .role("alert")
        .color(p.danger);
    labelled(p, key, label, Some(hint), control)
}

fn labelled(p: &Paint, key: &str, label: &str, hint: Option<El>, control: El) -> El {
    let mut words = col()
        .key(format!("{key}-words"))
        .grow(1.0)
        .min_w(0.0)
        .gap(2.0)
        .child(
            text(label)
                .key(format!("{key}-label"))
                .font(type_::BODY)
                .color(p.t.text)
                .wrap(),
        );
    if let Some(hint) = hint {
        words = words.child(hint);
    }
    row()
        .key(key)
        .min_h(ROW_HEIGHT)
        .px(space::S3 + 2.0)
        .py(space::S2)
        .gap(space::S3)
        .center_y()
        .child(words)
        .child(control)
}

pub fn toggle(
    p: &Paint,
    key: &str,
    label: &str,
    hint: Option<String>,
    on: bool,
    flip: impl Fn(&mut Ngine) + 'static,
) -> El {
    setting(p, key, label, hint, switch(p, &format!("{key}-switch"), on))
        .role("switch")
        .label(label)
        .checked(on)
        .radius(radius::MD)
        .hover(p.t.hover)
        .press(p.t.press)
        .focusable(true)
        .on_press(flip)
}

pub fn card(p: &Paint, key: &str, rows: Vec<El>) -> El {
    let last = rows.len().saturating_sub(1);
    let mut card = col()
        .key(key)
        .padding(space::S1)
        .fill(p.t.card)
        .radius(radius::LG)
        .border(p.hair, p.t.card_border);
    for (index, entry) in rows.into_iter().enumerate() {
        card = card.child(entry);
        if index < last {
            card = card.child(
                el().key(format!("{key}-separator-{index}"))
                    .h(p.hair)
                    .mx(space::S3)
                    .fill(p.t.separator)
                    .a11y_hidden(),
            );
        }
    }
    card
}

pub fn section_title(p: &Paint, key: &str, title: &str) -> El {
    text(title)
        .key(key)
        .role("heading")
        .level(2)
        .font(type_::SECTION)
        .color(p.t.text_secondary)
}

pub fn section(p: &Paint, key: &str, title: &str, rows: Vec<El>) -> El {
    col()
        .key(key)
        .gap(space::S2)
        .child(section_title(p, &format!("{key}-title"), title).ml(space::S1))
        .child(card(p, &format!("{key}-card"), rows))
}

pub fn empty_state(
    p: &Paint,
    key: &str,
    glyph: &'static Icon,
    title: &str,
    words: String,
    action: Option<El>,
) -> El {
    let mut state = col()
        .key(key)
        .w_full()
        .grow(1.0)
        .min_h(260.0)
        .center()
        .gap(space::S2)
        .child(
            el().key(format!("{key}-icon"))
                .size(48.0, 48.0)
                .radius(24.0)
                .fill(p.t.control_track)
                .center()
                .mb(space::S1)
                .child(icon(glyph, 22.0).color(p.t.text_secondary))
                .a11y_hidden(),
        )
        .child(
            text(title)
                .key(format!("{key}-title"))
                .font(HEADING)
                .color(p.t.text),
        );
    if !words.is_empty() {
        state = state.child(
            caption(p, &format!("{key}-words"), words)
                .max_w(360.0)
                .align(Align::Center),
        );
    }
    if let Some(action) = action {
        state = state.child(
            el().key(format!("{key}-action"))
                .mt(space::S2)
                .child(action),
        );
    }
    state
}

/// What a floating surface needs to know to place itself and to close.
pub struct Floating<'a> {
    pub key: &'a str,
    pub anchor: &'a str,
    pub side: Side,
    pub align: Align,
}

fn close(app: &mut Ngine) {
    app.close_popup();
}

/// Mounts `surface` above everything else, beside its anchor. A press
/// outside it or Escape closes it, and focus goes back to the anchor.
pub fn float(layer: &mut Vec<El>, floating: &Floating<'_>, surface: El) {
    layer.push(
        el().key(format!("{}-scrim", floating.key))
            .role("none")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .on_press(close),
    );
    layer.push(
        surface
            .modal(true, close)
            .focus_return(floating.anchor)
            .anchored_fit(floating.anchor, floating.side, floating.align, POPUP_GAP),
    );
}

/// Mounts `surface` above everything else with its corner at `at`, kept
/// inside `window`. A press outside it or Escape closes it.
pub fn float_at(layer: &mut Vec<El>, key: &str, at: (f32, f32), window: (f32, f32), surface: El) {
    let x = at.0.min(window.0 - MENU_WIDTH - POPUP_GAP).max(0.0);
    let y = at.1.min(window.1 - 6.0 * MENU_ITEM).max(0.0);
    layer.push(
        el().key(format!("{key}-scrim"))
            .role("none")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .on_press(close),
    );
    layer.push(surface.modal(true, close).absolute(x, y));
}

pub fn surface(p: &Paint, key: &str, role: &'static str, label: &str) -> El {
    col()
        .key(key)
        .role(role)
        .label(label)
        .padding(space::S1)
        .radius(10.0)
        .fill(p.raised)
        .border(p.hair, p.outline)
        .shadow(8.0, 24.0, p.t.shadow)
        .items_stretch()
}

pub fn menu_heading(p: &Paint, key: &str, words: &str) -> El {
    text(words)
        .key(key)
        .font(type_::SECTION)
        .color(p.faint)
        .px(space::S2)
        .pt(space::S2)
        .pb(space::S1)
}

pub fn menu_rule(p: &Paint, key: &str) -> El {
    el().key(key)
        .h(p.hair)
        .my(space::S1)
        .fill(p.t.separator)
        .a11y_hidden()
}

pub struct MenuItem<'a> {
    pub key: &'a str,
    pub label: &'a str,
    pub detail: Option<String>,
    pub icon: Option<&'static Icon>,
    pub chosen: Option<bool>,
    pub danger: bool,
    pub blocked: Option<String>,
}

impl<'a> MenuItem<'a> {
    pub fn new(key: &'a str, label: &'a str) -> Self {
        Self {
            key,
            label,
            detail: None,
            icon: None,
            chosen: None,
            danger: false,
            blocked: None,
        }
    }

    pub fn icon(mut self, icon: &'static Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn chosen(mut self, chosen: bool) -> Self {
        self.chosen = Some(chosen);
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn blocked(mut self, why: Option<String>) -> Self {
        self.blocked = why;
        self
    }

    /// One row of a menu or a list of choices. `role` is `menuitem` in a
    /// menu and `option` in a list of choices.
    pub fn build(
        self,
        p: &Paint,
        role: &'static str,
        on_press: impl Fn(&mut Ngine, &mut Tx) + 'static,
    ) -> El {
        let ink = if self.danger { p.danger } else { p.t.text };
        let mut item = row()
            .key(self.key)
            .role(role)
            .label(self.label)
            .min_h(MENU_ITEM)
            .px(space::S2)
            .gap(space::S2)
            .center_y()
            .radius(radius::SM);
        if let Some(chosen) = self.chosen {
            item = item.checked(chosen).child(
                icon(&icons::CHECK, ICON)
                    .key(format!("{}-check", self.key))
                    .color(p.accent)
                    .opacity(if chosen { 1.0 } else { 0.0 })
                    .a11y_hidden(),
            );
        }
        if let Some(glyph) = self.icon {
            let tint = if self.danger {
                p.danger
            } else {
                p.t.text_secondary
            };
            item = item.child(icon(glyph, ICON).color(tint).a11y_hidden());
        }
        item = item.child(
            text(self.label)
                .key(format!("{}-label", self.key))
                .font(type_::BODY)
                .color(ink)
                .truncate()
                .grow(1.0)
                .min_w(0.0)
                .a11y_hidden(),
        );
        let detail = self.blocked.clone().or(self.detail);
        if let Some(detail) = detail {
            item = item.child(
                text(detail)
                    .key(format!("{}-detail", self.key))
                    .font(CAPTION)
                    .color(p.faint)
                    .ml(space::S3),
            );
        }
        match self.blocked {
            Some(why) => item
                .disabled(true)
                .focusable(false)
                .opacity(0.55)
                .description(why),
            None => item
                .focusable(true)
                .focus_highlight(p.t.text.alpha(0.07))
                .press(Color::BLACK.alpha(0.12))
                .on_press_tx(on_press),
        }
    }
}

pub fn menu(p: &Paint, key: &str, label: &str, items: Vec<El>) -> El {
    surface(p, key, "menu", label)
        .focus_group()
        .min_w(MENU_WIDTH)
        .children(items)
}

pub struct Option_ {
    pub label: String,
    pub detail: Option<String>,
    pub blocked: Option<String>,
}

impl Option_ {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: None,
            blocked: None,
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

pub struct Select<'a> {
    pub key: &'a str,
    pub label: &'a str,
    pub options: Vec<Option_>,
    pub selected: Option<usize>,
    pub width: f32,
}

/// A button that shows the current choice and opens the list of the others.
pub fn select(
    p: &Paint,
    app: &Ngine,
    layer: &mut Vec<El>,
    select: Select<'_>,
    pick: impl Fn(&mut Ngine, usize) + Clone + 'static,
) -> El {
    let Select {
        key,
        label,
        options,
        selected,
        width,
    } = select;
    let open = app.popup == Some(Popup::Choices(key.to_owned()));
    let current = selected
        .and_then(|index| options.get(index))
        .map_or_else(|| "Choose…".to_owned(), |option| option.label.clone());
    let popup_key = key.to_owned();
    let trigger = row()
        .key(key)
        .role("combobox")
        .label(label)
        .a11y_value(current.clone())
        .expanded(open)
        .w(width)
        .h(CONTROL)
        .pl(space::S3)
        .pr(space::S2)
        .gap(space::S2)
        .center_y()
        .radius(radius::MD)
        .fill(p.t.control_off)
        .hover(p.t.text.alpha(0.05))
        .press(Color::BLACK.alpha(0.14))
        .focusable(true)
        .on_press(move |app: &mut Ngine| {
            let wanted = Popup::Choices(popup_key.clone());
            if app.popup.as_ref() == Some(&wanted) {
                app.close_popup();
            } else {
                app.open_popup(wanted);
            }
        })
        .child(
            text(current)
                .key(format!("{key}-current"))
                .font(type_::LABEL)
                .color(p.t.text)
                .truncate()
                .grow(1.0)
                .min_w(0.0)
                .a11y_hidden(),
        )
        .child(
            icon(&icons::CHEVRON_DOWN, ICON)
                .color(p.t.text_secondary)
                .a11y_hidden(),
        );
    if open {
        let detailed = options.iter().any(|option| option.detail.is_some());
        let items = options
            .into_iter()
            .enumerate()
            .map(|(index, option)| {
                let pick = pick.clone();
                let item_key = format!("{key}.{index}");
                let mut item =
                    MenuItem::new(&item_key, &option.label).chosen(selected == Some(index));
                item.detail = option.detail;
                item.blocked = option.blocked;
                item.build(p, "option", move |app, _| {
                    app.close_popup();
                    pick(app, index);
                })
            })
            .collect::<Vec<_>>();
        let list = surface(p, &format!("{key}-choices"), "listbox", label)
            .focus_group()
            .min_w(width.max(if detailed { 260.0 } else { 180.0 }))
            .max_h(LIST_HEIGHT)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .children(items);
        let floating = Floating {
            key,
            anchor: key,
            side: Side::Bottom,
            align: Align::End,
        };
        float(layer, &floating, list);
    }
    trigger
}

/// A question in the middle of the window that must be answered or closed.
pub fn dialog(
    p: &Paint,
    layer: &mut Vec<El>,
    key: &str,
    title: &str,
    words: String,
    actions: Vec<El>,
) {
    let panel = col()
        .key(key)
        .role("dialog")
        .label(title)
        .w(DIALOG_WIDTH)
        .padding(space::S6 - 4.0)
        .gap(space::S2)
        .radius(14.0)
        .fill(p.raised)
        .border(p.hair, p.outline)
        .shadow(16.0, 48.0, p.t.shadow)
        .modal(true, close)
        .child(
            text(title)
                .key(format!("{key}-title"))
                .font(HEADING)
                .color(p.t.text)
                .wrap(),
        )
        .child(caption(p, &format!("{key}-words"), words))
        .child(
            row()
                .key(format!("{key}-actions"))
                .gap(space::S2)
                .mt(space::S3)
                .justify_end()
                .children(actions),
        );
    layer.push(
        col()
            .key(format!("{key}-scrim"))
            .role("none")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .center()
            .fill(Color::BLACK.alpha(0.5))
            .on_press(close)
            .child(panel),
    );
}
