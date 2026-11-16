use rune::tok::{self, TextStyle, Theme, motion, radius, space, type_};
use rune::{Color, Drag, El, Phase, TextEdit, col, el, oklch, row, text, text_input};

use crate::app::{Field, Ngine};

pub const HEADING: TextStyle = TextStyle::new(15.0, 600.0);
pub const TIMER: TextStyle = TextStyle::new(28.0, 600.0).features("tnum");
pub const CAPTION: TextStyle = TextStyle::new(11.5, 400.0);
pub const STRONG: TextStyle = TextStyle::new(13.0, 600.0);

const CONTROL_HEIGHT: f32 = 30.0;
const ROW_HEIGHT: f32 = 44.0;
const SWITCH: (f32, f32) = (38.0, 22.0);
const KNOB: f32 = 18.0;
const SLIDER_WIDTH: f32 = 180.0;
const SLIDER_BAR: f32 = 4.0;
const SEGMENT_PAD: f32 = 2.0;

#[derive(Clone, Copy)]
pub struct Paint {
    pub t: Theme,
    pub accent: Color,
    pub danger: Color,
    pub warning: Color,
    pub good: Color,
    pub hair: f32,
}

impl Paint {
    pub fn dark(scale: f32) -> Self {
        Self {
            t: tok::theme(true),
            accent: tok::accent(0, true),
            danger: oklch(0.64, 0.2, 25.0),
            warning: oklch(0.8, 0.14, 80.0),
            good: tok::accent(4, true),
            hair: 1.0 / scale,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Primary,
    Danger,
    Plain,
}

pub fn button(
    p: &Paint,
    key: &str,
    label: &str,
    look: Look,
    enabled: bool,
    on_press: impl Fn(&mut Ngine) + 'static,
) -> El {
    let (fill, ink) = match look {
        Look::Primary => (p.accent, Color::WHITE),
        Look::Danger => (p.danger, Color::WHITE),
        Look::Plain => (p.t.control_off, p.t.text),
    };
    let base = row()
        .key(key)
        .role("button")
        .label(label)
        .h(CONTROL_HEIGHT)
        .px(space::S3)
        .center()
        .radius(radius::MD)
        .fill(fill)
        .child(text(label).font(type_::LABEL).color(ink).a11y_hidden());
    if enabled {
        base.hover(Color::WHITE.alpha(0.08))
            .press(Color::BLACK.alpha(0.14))
            .focusable(true)
            .on_press(on_press)
    } else {
        base.disabled(true).opacity(0.4)
    }
}

pub fn caption(p: &Paint, key: &str, words: impl Into<String>) -> El {
    text(words)
        .key(key)
        .font(CAPTION)
        .color(p.t.text_secondary)
        .wrap()
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

pub struct Choice {
    pub id: &'static str,
    pub label: String,
    pub available: bool,
}

impl Choice {
    pub fn new(id: &'static str, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            available: true,
        }
    }

    pub fn available(mut self, available: bool) -> Self {
        self.available = available;
        self
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
    let inner = CONTROL_HEIGHT - 2.0 * SEGMENT_PAD;
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
        let mut segment = el()
            .key(format!("{key}.{}", choice.id))
            .role("segment")
            .label(choice.label.clone())
            .checked(chosen)
            .size(segment_width, inner)
            .center()
            .radius(radius::SM)
            .child(
                text(choice.label)
                    .font(type_::LABEL)
                    .color(ink)
                    .a11y_hidden(),
            );
        if choice.available {
            let pick = pick.clone();
            segment = segment
                .focusable(true)
                .hover(if chosen {
                    Color::TRANSPARENT
                } else {
                    p.t.hover
                })
                .on_press(move |app: &mut Ngine| pick(app, index));
        } else {
            segment = segment.disabled(true).opacity(0.35);
        }
        strip = strip.child(segment);
    }
    let mut control = el()
        .key(key)
        .role("segmented")
        .label(label)
        .size(left + SEGMENT_PAD, CONTROL_HEIGHT)
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
}

pub fn slider(p: &Paint, stops: Stops<'_>, set: impl Fn(&mut Ngine, u16) + Clone + 'static) -> El {
    let Stops {
        key,
        label,
        index,
        count,
        value,
    } = stops;
    let last = f32::from(count.saturating_sub(1).max(1));
    let travel = SLIDER_WIDTH - KNOB;
    let fraction = f32::from(index) / last;
    let stop_at = move |x: f32| nearest_stop((x - KNOB / 2.0) / travel, count);
    let (drag_set, step_set) = (set.clone(), set);
    let track = el()
        .key(key)
        .role("slider")
        .label(label)
        .range_value(f64::from(index), 0.0, f64::from(last))
        .range_step(1.0)
        .a11y_value(value.clone())
        .focusable(true)
        .size(SLIDER_WIDTH, SWITCH.1)
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
                .size(SLIDER_WIDTH, SLIDER_BAR)
                .absolute(0.0, (SWITCH.1 - SLIDER_BAR) / 2.0)
                .radius(SLIDER_BAR / 2.0)
                .fill(p.t.control_off)
                .a11y_hidden(),
        )
        .child(
            el().key(format!("{key}-fill"))
                .size(KNOB / 2.0 + fraction * travel, SLIDER_BAR)
                .absolute(0.0, (SWITCH.1 - SLIDER_BAR) / 2.0)
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
        .gap(space::S3)
        .center_y()
        .child(
            text(value)
                .key(format!("{key}-value"))
                .font(type_::LABEL)
                .color(p.t.text_secondary)
                .a11y_hidden(),
        )
        .child(track)
}

pub struct Input<'a> {
    pub key: &'a str,
    pub label: &'a str,
    pub edit: &'a TextEdit,
    pub field: Field,
    pub width: f32,
    pub invalid: bool,
    pub applies_on_enter: bool,
}

pub fn input(p: &Paint, input: &Input<'_>) -> El {
    let field = input.field;
    let mut editor = text_input(input.edit, type_::BODY)
        .key(input.key)
        .label(input.label)
        .single_line()
        .color(p.t.text)
        .on_edit(move |app: &mut Ngine| app.fields.get_mut(field));
    editor = if input.applies_on_enter {
        editor.on_submit(move |app: &mut Ngine, _| app.field_edited(field))
    } else {
        editor.on_input_change(move |app: &mut Ngine, _| app.field_edited(field))
    };
    el().key(format!("{}-box", input.key))
        .w(input.width)
        .h(CONTROL_HEIGHT)
        .px(space::S2)
        .radius(radius::MD)
        .fill(p.t.control_track)
        .border(
            1.0,
            if input.invalid {
                p.danger
            } else {
                p.t.card_border
            },
        )
        .center_y()
        .child(editor)
}

pub fn setting(p: &Paint, key: &str, label: &str, hint: Option<String>, control: El) -> El {
    let hint = hint.map(|hint| caption(p, &format!("{key}-hint"), hint));
    labelled(p, key, label, hint, control)
}

pub fn troubled_setting(p: &Paint, key: &str, label: &str, trouble: String, control: El) -> El {
    let hint = caption(p, &format!("{key}-hint"), trouble).color(p.danger);
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
                .color(p.t.text),
        );
    if let Some(hint) = hint {
        words = words.child(hint);
    }
    row()
        .key(key)
        .min_h(ROW_HEIGHT)
        .px(space::S3)
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
                    .ml(space::S3)
                    .mr(space::S3)
                    .fill(p.t.separator)
                    .a11y_hidden(),
            );
        }
    }
    card
}

pub fn section(p: &Paint, key: &str, title: &str, rows: Vec<El>) -> El {
    col()
        .key(key)
        .gap(space::S2)
        .child(
            text(title)
                .key(format!("{key}-title"))
                .font(type_::SECTION)
                .color(p.t.text_secondary)
                .ml(space::S3),
        )
        .child(card(p, &format!("{key}-card"), rows))
}
