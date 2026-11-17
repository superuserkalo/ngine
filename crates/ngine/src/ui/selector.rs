use rune::tok::{radius, space, type_};
use rune::{
    Color, ConfiguredOverlay, Cursor, Cx, Drag, El, Key, KeyEvent, KeyPolicy, OverlayInput,
    OverlayLevel, OverlayOptions, OverlayPlacement, OverlaySpec, Phase, Tx, el, row, text,
};

use super::widgets::{CONTROL, NUMBERS, Paint, STRONG};
use crate::engine::{Display, Region, Target, TargetKind};
use crate::format;
use crate::selection::{Area, Bounds, Grip, Point};

pub const NAME: &str = "ngine-selection";
pub const SAMPLE_SIZE: (f32, f32) = (1440.0, 900.0);
const GRAB_REACH: f32 = 8.0;
const HANDLE: f32 = 10.0;
const FRAME: f32 = 2.0;
const SHADE: f32 = 0.5;
const LABEL_GAP: f32 = 8.0;
const LABEL_HEIGHT: f32 = 24.0;
const BAR_MARGIN: f32 = 32.0;
const BAR_PADDING: f32 = 6.0;
pub const DRAG_HINT: &str = "Drag to choose what to record";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Record(Region),
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Grab {
    grip: Option<Grip>,
    before: Option<Area>,
}

/// The full-screen overlay where a part of a screen is chosen, and the frame
/// around that part while it records.
#[derive(Default)]
pub struct Selector {
    pub screen: Option<Target>,
    pub area: Option<Area>,
    pub framing: bool,
    pub hovered: Option<Grip>,
    pub answered: Option<Answer>,
    grab: Option<Grab>,
    reply: Option<Box<dyn Fn(Answer)>>,
}

impl Selector {
    pub fn answering(reply: impl Fn(Answer) + 'static) -> Self {
        Self {
            reply: Some(Box::new(reply)),
            ..Self::default()
        }
    }

    /// Starts choosing on `screen`, offering `area` to begin with.
    pub fn choose_on(&mut self, screen: Target, area: Option<Area>) {
        self.screen = Some(screen);
        self.area = area;
        self.framing = false;
        self.hovered = None;
        self.answered = None;
        self.grab = None;
    }

    /// Draws a frame around `area` of `screen`, outside it, and takes no
    /// input.
    pub fn frame(&mut self, screen: Target, area: Area) {
        self.choose_on(screen, Some(area));
        self.framing = true;
    }

    fn bounds(&self) -> Bounds {
        let display = self.screen.as_ref().and_then(|screen| screen.display);
        display.map_or(Bounds::of(0, 0), |display| {
            Bounds::of(display.width, display.height)
        })
    }

    fn answer(&mut self, answer: Answer) {
        if let Some(reply) = &self.reply {
            reply(answer.clone());
        }
        self.answered = Some(answer);
    }

    pub fn confirm(&mut self) {
        let chosen = self.screen.as_ref().zip(self.area);
        if let Some((screen, area)) = chosen.filter(|(_, area)| area.is_usable())
            && !self.framing
        {
            let region = Region {
                screen: screen.id.clone(),
                area,
            };
            self.answer(Answer::Record(region));
        }
    }

    pub fn cancel(&mut self) {
        if !self.framing {
            self.answer(Answer::Cancel);
        }
    }

    pub fn key(&mut self, event: &KeyEvent) -> bool {
        match event.key {
            Key::Escape => self.cancel(),
            Key::Enter => self.confirm(),
            _ => return false,
        }
        true
    }

    pub fn hover(&mut self, x: f32, y: f32, pixels_per_point: f32) {
        let at = Point::from_points(x, y, pixels_per_point);
        let reach = f64::from(GRAB_REACH * pixels_per_point);
        self.hovered = self.area.and_then(|area| area.grip_at(at, reach));
    }

    pub fn drag(&mut self, drag: &Drag, pixels_per_point: f32) {
        let from = Point::from_points(drag.start_x, drag.start_y, pixels_per_point);
        if drag.phase == Phase::Down {
            let reach = f64::from(GRAB_REACH * pixels_per_point);
            self.grab = Some(Grab {
                grip: self.area.and_then(|area| area.grip_at(from, reach)),
                before: self.area,
            });
            return;
        }
        let Some(grab) = self.grab else {
            return;
        };
        if drag.cancelled() {
            self.area = grab.before;
            self.grab = None;
            return;
        }
        let at = Point::from_points(drag.x, drag.y, pixels_per_point);
        let (dx, dy) = (at.x - from.x, at.y - from.y);
        let bounds = self.bounds();
        let area = match (grab.grip, grab.before) {
            (Some(grip), Some(before)) => before.resized(grip, dx, dy, bounds),
            _ => Area::drawn(from, at, bounds),
        };
        self.area = Some(area);
        if drag.phase == Phase::Up {
            if !area.is_usable() {
                self.area = grab.before;
            }
            self.grab = None;
            self.hover(drag.x, drag.y, pixels_per_point);
        }
    }

    fn held(&self) -> Option<Grip> {
        match self.grab {
            Some(Grab { grip, .. }) => grip,
            None => self.hovered,
        }
    }
}

pub fn cursor(grip: Option<Grip>) -> Cursor {
    match grip {
        Some(Grip::Inside) => Cursor::Move,
        Some(Grip::Left | Grip::Right) => Cursor::ResizeColumn,
        Some(Grip::Top | Grip::Bottom) => Cursor::ResizeRow,
        Some(Grip::TopLeft | Grip::TopRight | Grip::BottomLeft | Grip::BottomRight) | None => {
            Cursor::Crosshair
        }
    }
}

pub fn spec() -> ConfiguredOverlay<Selector> {
    let options = OverlayOptions {
        placement: Some(OverlayPlacement::FullScreen),
        level: OverlayLevel::AboveOverlays,
        exclude_from_capture: true,
        keys: KeyPolicy::OnDemand,
        input: OverlayInput::All,
        ..OverlayOptions::default()
    };
    OverlaySpec::new(NAME, SAMPLE_SIZE, view)
        .options(options)
        .on_key(|selector: &mut Selector, event, _| selector.key(event))
        .release_gpu_when_hidden()
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a screen's pixels are far below the 2^24 an f32 holds exactly"
)]
fn pixels_per_point(bounds: Bounds, points_wide: f32, scale: f32) -> f32 {
    if bounds.width > 0 && points_wide > 0.0 {
        bounds.width as f32 / points_wide
    } else {
        scale
    }
}

pub fn view(selector: &Selector, cx: &mut Cx<Selector>) -> El {
    let scale = cx.env().scale;
    let paint = Paint::dark(scale);
    let size = cx
        .layout_size("selector")
        .map_or(SAMPLE_SIZE, |size| (size[0], size[1]));
    let ratio = pixels_per_point(selector.bounds(), size.0, scale);
    if selector.framing {
        return frame(&paint, selector, ratio);
    }
    let mut root = el()
        .key("selector")
        .role("dialog")
        .label("Choose the part of the screen to record")
        .w_full()
        .h_full()
        .hit()
        .cursor(cursor(selector.held()))
        .on_pointer_move_local(move |selector: &mut Selector, pointer, _| {
            selector.hover(pointer.x, pointer.y, ratio);
        })
        .on_drag(
            move |selector: &mut Selector, drag: &mut Drag, tx: &mut Tx| {
                selector.drag(drag, ratio);
                tx.instant();
            },
        );
    let shade = Color::BLACK.alpha(SHADE);
    match selector.area {
        None => {
            root = root.child(
                el().key("selector-shade")
                    .w_full()
                    .h_full()
                    .absolute(0.0, 0.0)
                    .fill(shade)
                    .a11y_hidden(),
            );
        }
        Some(area) => {
            let [x, y, w, h] = area.in_points(ratio);
            let panes = [
                ("top", [0.0, 0.0, size.0, y]),
                ("bottom", [0.0, y + h, size.0, size.1 - y - h]),
                ("left", [0.0, y, x, h]),
                ("right", [x + w, y, size.0 - x - w, h]),
            ];
            for (side, [left, top, width, height]) in panes {
                root = root.child(
                    el().key(format!("selector-shade-{side}"))
                        .absolute(left, top)
                        .size(width.max(0.0), height.max(0.0))
                        .fill(shade)
                        .a11y_hidden(),
                );
            }
            root = root
                .child(
                    el().key("selector-area")
                        .role("img")
                        .label(format!(
                            "Selection, {}",
                            format::dimensions(area.width, area.height)
                        ))
                        .absolute(x, y)
                        .size(w, h)
                        .border(paint.hair.max(1.0), Color::WHITE.alpha(0.9))
                        .outline(1.0, 0.0, Color::BLACK.alpha(0.5)),
                )
                .children(handles([x, y, w, h]))
                .child(size_label(&paint, area, [x, y, w, h], size));
        }
    }
    root.child(bar(&paint, selector, size, ratio))
}

fn handles([x, y, w, h]: [f32; 4]) -> Vec<El> {
    let spots = [
        ("top-left", x, y),
        ("top", x + w / 2.0, y),
        ("top-right", x + w, y),
        ("right", x + w, y + h / 2.0),
        ("bottom-right", x + w, y + h),
        ("bottom", x + w / 2.0, y + h),
        ("bottom-left", x, y + h),
        ("left", x, y + h / 2.0),
    ];
    spots
        .into_iter()
        .map(|(spot, left, top)| {
            el().key(format!("selector-handle-{spot}"))
                .absolute(left - HANDLE / 2.0, top - HANDLE / 2.0)
                .size(HANDLE, HANDLE)
                .radius(HANDLE / 2.0)
                .fill(Color::WHITE)
                .border(1.0, Color::BLACK.alpha(0.5))
                .a11y_hidden()
        })
        .collect()
}

const BAR_HEIGHT: f32 = CONTROL + 2.0 * BAR_PADDING;

fn bar_top(screen: (f32, f32), part: Option<[f32; 4]>) -> f32 {
    let bottom = screen.1 - BAR_MARGIN - BAR_HEIGHT;
    let Some([_, y, _, h]) = part else {
        return bottom;
    };
    let reaches_bottom = y + h > bottom - LABEL_GAP;
    let room_above = y >= BAR_MARGIN + BAR_HEIGHT + LABEL_GAP;
    if reaches_bottom && room_above {
        BAR_MARGIN
    } else {
        bottom
    }
}

fn size_label(paint: &Paint, area: Area, [x, y, w, h]: [f32; 4], screen: (f32, f32)) -> El {
    let below = y + h + LABEL_GAP;
    let above = y - LABEL_GAP - LABEL_HEIGHT;
    let bar = bar_top(screen, Some([x, y, w, h]));
    let clear_of_bar =
        |top: f32| top + LABEL_HEIGHT + LABEL_GAP <= bar || top >= bar + BAR_HEIGHT + LABEL_GAP;
    let top = if below + LABEL_HEIGHT <= screen.1 && clear_of_bar(below) {
        below
    } else if above >= 0.0 && clear_of_bar(above) {
        above
    } else {
        y + LABEL_GAP
    };
    row()
        .key("selector-size-box")
        .absolute(x, top)
        .w(w)
        .h(LABEL_HEIGHT)
        .center()
        .child(
            row()
                .key("selector-size")
                .h(LABEL_HEIGHT)
                .px(space::S2)
                .center()
                .radius(radius::MD)
                .fill(paint.raised.alpha(0.92))
                .child(
                    text(format::dimensions(area.width, area.height))
                        .key("selector-size-text")
                        .font(NUMBERS)
                        .color(paint.t.text),
                ),
        )
}

fn button(paint: &Paint, key: &str, label: &str, primary: bool, press: fn(&mut Selector)) -> El {
    let (fill, ink) = if primary {
        (paint.accent, paint.on_accent)
    } else {
        (Color::TRANSPARENT, paint.t.text)
    };
    row()
        .key(key)
        .role("button")
        .label(label)
        .h(CONTROL)
        .px(space::S3)
        .center()
        .radius(radius::MD)
        .fill(fill)
        .hover(paint.t.text.alpha(0.07))
        .press(Color::BLACK.alpha(0.16))
        .cursor(Cursor::Pointer)
        .on_press(press)
        .child(
            text(label)
                .font(if primary { STRONG } else { type_::LABEL })
                .color(ink)
                .a11y_hidden(),
        )
}

fn bar(paint: &Paint, selector: &Selector, screen: (f32, f32), ratio: f32) -> El {
    let mut bar = row()
        .key("selector-bar")
        .role("toolbar")
        .label("Selection")
        .padding(BAR_PADDING)
        .gap(space::S1)
        .center_y()
        .radius(radius::LG + BAR_PADDING / 2.0)
        .fill(paint.raised)
        .border(paint.hair.max(1.0), paint.outline);
    if selector.area.is_none() {
        bar = bar.child(
            text(DRAG_HINT)
                .key("selector-hint")
                .font(type_::LABEL)
                .color(paint.t.text_secondary)
                .px(space::S2),
        );
    }
    bar = bar.child(button(
        paint,
        "selector-cancel",
        "Cancel",
        false,
        Selector::cancel,
    ));
    if selector.area.is_some() {
        bar = bar.child(button(
            paint,
            "selector-record",
            "Record",
            true,
            Selector::confirm,
        ));
    }
    let part = selector.area.map(|area| area.in_points(ratio));
    row()
        .key("selector-bar-box")
        .absolute(0.0, bar_top(screen, part))
        .w(screen.0)
        .center()
        .child(bar)
}

fn frame(paint: &Paint, selector: &Selector, ratio: f32) -> El {
    let mut root = el().key("selector").w_full().h_full();
    if let Some(area) = selector.area {
        let [x, y, w, h] = area.in_points(ratio);
        root = root.child(
            el().key("selector-frame")
                .role("img")
                .label("Recording this part of the screen")
                .absolute(x - FRAME, y - FRAME)
                .size(w + 2.0 * FRAME, h + 2.0 * FRAME)
                .border(FRAME, paint.danger),
        );
    }
    root
}

/// A screen to choose on when there is no real one, for tooling and tests.
pub fn sample_screen() -> Target {
    Target::new(
        "screen-1",
        TargetKind::Screen,
        "Built-in display",
        format::dimensions(2880, 1800),
    )
    .on_display(Display {
        id: 1,
        width: 2880,
        height: 1800,
    })
}

pub fn sample() -> Selector {
    let mut selector = Selector::default();
    selector.choose_on(sample_screen(), None);
    selector
}

type Scene = (&'static str, fn(&mut Selector));

pub const SCENES: &[Scene] = &[
    ("empty", |_| {}),
    ("chosen", |selector| {
        selector.area = Some(Area::new(560, 400, 1280, 720));
    }),
    ("moving", |selector| {
        selector.area = Some(Area::new(560, 400, 1280, 720));
        selector.hovered = Some(Grip::Inside);
    }),
    ("near-the-bottom", |selector| {
        selector.area = Some(Area::new(200, 1100, 2400, 680));
    }),
    ("recording", |selector| {
        selector.frame(sample_screen(), Area::new(560, 400, 1280, 720));
    }),
];

#[cfg(test)]
mod tests {
    use rune::{Env, Mods, Runtime};

    use super::*;

    const FRAME_TIME: f64 = 1.0 / 60.0;

    fn runtime(selector: Selector) -> Runtime<Selector> {
        let mut runtime = Runtime::new(selector, view, SAMPLE_SIZE, Env::new(true, 2.0));
        runtime.frame(0.0);
        runtime.frame(FRAME_TIME);
        runtime
    }

    fn scene(name: &str) -> Runtime<Selector> {
        let mut selector = sample();
        let (_, apply) = SCENES
            .iter()
            .find(|(scene, _)| *scene == name)
            .unwrap_or_else(|| panic!("no scene called {name}"));
        apply(&mut selector);
        runtime(selector)
    }

    fn words(runtime: &Runtime<Selector>, key: &str) -> String {
        let element = runtime
            .elements()
            .into_iter()
            .find(|element| element.key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("no element with the key {key}"));
        element.text.or(element.label).unwrap_or_default()
    }

    fn rect(runtime: &Runtime<Selector>, key: &str) -> [f32; 4] {
        runtime
            .elements()
            .into_iter()
            .find(|element| element.key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("no element with the key {key}"))
            .rect
    }

    fn drag(runtime: &mut Runtime<Selector>, from: (f32, f32), to: (f32, f32)) {
        runtime.pointer_move(from.0, from.1);
        runtime.pointer_down(from.0, from.1);
        runtime.pointer_move(f32::midpoint(from.0, to.0), f32::midpoint(from.1, to.1));
        runtime.pointer_move(to.0, to.1);
        runtime.pointer_up(to.0, to.1);
        runtime.frame(FRAME_TIME);
        runtime.settle_all();
    }

    fn key(key: Key) -> KeyEvent {
        KeyEvent {
            key,
            mods: Mods::default(),
        }
    }

    #[test]
    fn the_overlay_asks_for_a_drag_until_a_part_is_chosen() {
        let runtime = scene("empty");

        assert_eq!(words(&runtime, "selector-hint"), DRAG_HINT);
        assert!(runtime.find("selector-cancel").is_some());
        assert!(runtime.find("selector-record").is_none());
        assert!(runtime.find("selector-area").is_none());
    }

    #[test]
    fn a_drag_draws_a_part_in_the_screens_pixels_and_says_its_size() {
        let mut runtime = scene("empty");

        drag(&mut runtime, (100.0, 100.0), (740.0, 460.0));

        assert_eq!(runtime.state.area, Some(Area::new(200, 200, 1280, 720)));
        assert_eq!(words(&runtime, "selector-size-text"), "1280 × 720");
        assert_eq!(
            rect(&runtime, "selector-area"),
            [100.0, 100.0, 640.0, 360.0]
        );
        assert!(runtime.find("selector-record").is_some());
        assert!(runtime.find("selector-hint").is_none());
    }

    #[test]
    fn the_inside_moves_the_part_and_an_edge_resizes_it_with_matching_cursors() {
        let mut runtime = scene("chosen");

        runtime.pointer_move(500.0, 400.0);
        runtime.frame(FRAME_TIME);
        assert_eq!(runtime.cursor(), Cursor::Move);
        drag(&mut runtime, (500.0, 400.0), (550.0, 420.0));
        assert_eq!(runtime.state.area, Some(Area::new(660, 440, 1280, 720)));

        runtime.pointer_move(970.0, 400.0);
        runtime.frame(FRAME_TIME);
        assert_eq!(runtime.cursor(), Cursor::ResizeColumn);
        drag(&mut runtime, (970.0, 400.0), (1000.0, 400.0));
        assert_eq!(runtime.state.area, Some(Area::new(660, 440, 1340, 720)));

        runtime.pointer_move(650.0, 580.0);
        runtime.frame(FRAME_TIME);
        assert_eq!(runtime.cursor(), Cursor::ResizeRow);
    }

    #[test]
    fn a_click_outside_keeps_the_part_already_chosen() {
        let mut runtime = scene("chosen");

        drag(&mut runtime, (40.0, 40.0), (40.0, 40.0));

        assert_eq!(runtime.state.area, Some(Area::new(560, 400, 1280, 720)));
    }

    #[test]
    fn enter_and_the_record_button_answer_with_the_part_and_escape_cancels() {
        let mut selector = sample();
        assert!(!selector.key(&key(Key::Tab)));
        selector.key(&key(Key::Enter));
        assert_eq!(selector.answered, None);

        selector.area = Some(Area::new(560, 400, 1280, 720));
        selector.key(&key(Key::Enter));
        let chosen = Answer::Record(Region {
            screen: "screen-1".into(),
            area: Area::new(560, 400, 1280, 720),
        });
        assert_eq!(selector.answered, Some(chosen.clone()));

        selector.key(&key(Key::Escape));
        assert_eq!(selector.answered, Some(Answer::Cancel));

        let mut runtime = scene("chosen");
        let (x, y) = runtime.find("selector-record").expect("a record button");
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(FRAME_TIME);
        assert_eq!(runtime.state.answered, Some(chosen));
    }

    #[test]
    fn the_size_label_moves_above_a_part_that_reaches_the_bottom() {
        let runtime = scene("near-the-bottom");

        let [_, top, _, height] = rect(&runtime, "selector-size-box");
        assert!(top + height <= 550.0, "{top}");
    }

    #[test]
    fn the_bar_moves_to_the_top_rather_than_cover_a_part_that_reaches_the_bottom() {
        let low = scene("near-the-bottom");
        let [_, top, _, height] = rect(&low, "selector-bar");
        assert!(top + height <= 550.0, "{top}");
        let middle = scene("chosen");
        let [_, top, _, _] = rect(&middle, "selector-bar");
        assert!(top > 560.0, "{top}");
    }

    #[test]
    fn while_recording_a_frame_goes_round_the_part_outside_it() {
        let runtime = scene("recording");

        assert_eq!(
            rect(&runtime, "selector-frame"),
            [
                280.0 - FRAME,
                200.0 - FRAME,
                640.0 + 2.0 * FRAME,
                360.0 + 2.0 * FRAME
            ]
        );
        assert!(runtime.find("selector-bar").is_none());
        assert!(runtime.find("selector-shade-top").is_none());
        let mut selector = sample();
        selector.frame(sample_screen(), Area::new(560, 400, 1280, 720));
        selector.key(&key(Key::Enter));
        selector.key(&key(Key::Escape));
        assert_eq!(selector.answered, None);
    }
}
