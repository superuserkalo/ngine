use rune::tok::{self, type_};
use rune::{AppSpec, Cx, El, col, text};

const NAME: &str = env!("CARGO_PKG_NAME");
const VERSION: &str = env!("CARGO_PKG_VERSION");
const WINDOW_SIZE: (f32, f32) = (720.0, 480.0);

#[derive(Default)]
struct State;

fn view(_: &State, _: &mut Cx<State>) -> El {
    let theme = tok::theme(true);
    col()
        .w_full()
        .h_full()
        .center()
        .gap(8.0)
        .fill(theme.bg)
        .child(text(NAME).key("name").font(type_::TITLE).color(theme.text))
        .child(
            text(VERSION)
                .key("version")
                .font(type_::BODY)
                .color(theme.text),
        )
}

fn main() {
    rune::run(AppSpec::<State>::new(
        NAME,
        WINDOW_SIZE,
        State::default,
        view,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn the_window_shows_the_name_and_version() {
        let mut runtime = Runtime::new(State, view, WINDOW_SIZE, Env::new(true, 2.0));
        runtime.frame(0.0);
        assert!(runtime.find("name").is_some());
        assert!(runtime.find("version").is_some());
    }
}
