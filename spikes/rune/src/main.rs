use std::thread;
use std::time::{Duration, Instant};

use rune::tok::{self, type_};
use rune::{
    Align, App, AppSpec, Color, Cx, El, OverlaySpec, Pixels, ScreenEdge, col, el, row, text,
};

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const BARS: [[u8; 4]; 8] = [
    [235, 235, 235, 255],
    [235, 235, 16, 255],
    [16, 235, 235, 255],
    [16, 235, 16, 255],
    [235, 16, 235, 255],
    [235, 16, 16, 255],
    [16, 16, 235, 255],
    [16, 16, 16, 255],
];

struct State {
    frame: Pixels,
}

struct Hud;

fn colour_bars(shift: u32) -> Vec<u8> {
    let bar_width = WIDTH / 8;
    let mut line = Vec::with_capacity(WIDTH as usize * 4);
    for x in 0..WIDTH {
        let bar = ((x + shift) / bar_width) % 8;
        line.extend_from_slice(&BARS[bar as usize]);
    }
    line.repeat(HEIGHT as usize)
}

fn view(state: &State, _: &mut Cx<State>) -> El {
    let theme = tok::theme(true);
    col()
        .w_full()
        .h_full()
        .center()
        .gap(12.0)
        .fill(theme.bg)
        .child(
            text("ngine spike: a 1920x1080 frame through Pixels")
                .font(type_::TITLE)
                .color(theme.text),
        )
        .child(
            el().key("frame")
                .size(640.0, 360.0)
                .radius(8.0)
                .pixels(&state.frame),
        )
}

fn hud(_: &Hud, _: &mut Cx<Hud>) -> El {
    let red = Color {
        r: 0.9,
        g: 0.1,
        b: 0.1,
        a: 1.0,
    };
    let theme = tok::theme(true);
    row()
        .w_full()
        .h_full()
        .center()
        .gap(8.0)
        .radius(18.0)
        .fill(theme.bg)
        .child(el().key("dot").size(10.0, 10.0).radius(5.0).fill(red))
        .child(text("REC").font(type_::BODY).color(theme.text))
}

fn main() {
    let frame = Pixels::new();
    frame.set(WIDTH, HEIGHT, colour_bars(0));
    let shown = frame.clone();
    let spec = AppSpec::<State>::new(
        "ngine Rune spike",
        (720.0, 480.0),
        || State {
            frame: Pixels::new(),
        },
        view,
    );
    let mut app = App::new(spec).init(move || State { frame: shown });

    let seconds = std::env::var("SPIKE_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok());
    if let Some(seconds) = seconds {
        let overlay = app.overlay(
            OverlaySpec {
                name: "spike-hud",
                size: (120.0, 36.0),
                edge: ScreenEdge::Top,
                margin: 16.0,
                align: Align::Center,
                inset: 0.0,
                interactive: false,
                view: hud,
            },
            Hud,
        );
        let handle = app.handle();
        thread::spawn(move || {
            overlay.show();
            let started = Instant::now();
            let mut pushed = 0_u32;
            let mut in_set = Duration::ZERO;
            while started.elapsed() < Duration::from_secs(seconds) {
                let bars = colour_bars(pushed * 8);
                let before = Instant::now();
                frame.set(WIDTH, HEIGHT, bars);
                in_set += before.elapsed();
                pushed += 1;
                thread::sleep(Duration::from_millis(16));
            }
            println!("spike: overlay {:?}", overlay.availability());
            println!(
                "spike: pushed {pushed} frames of {WIDTH}x{HEIGHT} in {:.1} s, mean set {} us",
                started.elapsed().as_secs_f32(),
                in_set.as_micros() / u128::from(pushed.max(1)),
            );
            handle.quit();
        });
    }
    app.run();
}
