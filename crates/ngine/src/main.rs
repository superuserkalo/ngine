use std::fmt;

mod app;
mod cli;
mod engine;
mod format;
mod headless;
mod hotkeys;
mod ipc;
mod platform;
mod settings;
mod shell;
mod toast;
mod tray;
mod ui;

struct Failure(String);

impl fmt::Debug for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn control(arguments: &[String]) -> Result<(), String> {
    let command = cli::parse(arguments).map_err(|error| format!("{error}\n\n{}", cli::USAGE))?;
    let message = shell::send(command)??;
    println!("{message}");
    Ok(())
}

fn main() -> Result<(), Failure> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let outcome = match arguments.first().map(String::as_str) {
        None => shell::run(false),
        Some(platform::BACKGROUND_FLAG) => shell::run(true),
        Some("--help" | "-h" | "help") => {
            println!("{}\n\n{}", cli::USAGE, headless::USAGE);
            Ok(())
        }
        Some(command @ ("record" | "replay"))
            if arguments
                .get(1)
                .is_some_and(|option| option.starts_with("--")) =>
        {
            headless::run(command, &arguments[1..])
        }
        Some("--version") => {
            println!("{} {}", app::NAME, app::VERSION);
            Ok(())
        }
        Some(flag) if flag.starts_with("--") => shell::run_tooling(arguments),
        Some(_) => control(&arguments),
    };
    outcome.map_err(Failure)
}
