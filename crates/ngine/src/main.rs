#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::fmt;

mod app;
mod bar;
mod cli;
mod doctor;
mod editor;
mod engine;
mod format;
mod headless;
mod hotkeys;
mod ipc;
mod library;
#[cfg_attr(
    not(target_os = "linux"),
    expect(
        dead_code,
        reason = "only on Linux does ngine hand its notifications to a desktop service"
    )
)]
mod notification;
mod platform;
mod selection;
mod settings;
mod shell;
mod speaker;
mod toast;
mod tray;
mod ui;
mod work;

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
    platform::print_to_the_terminal_that_started_ngine();
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let outcome = match arguments.first().map(String::as_str) {
        None => shell::run(false),
        Some(platform::BACKGROUND_FLAG) => shell::run(true),
        Some("--help" | "-h" | "help") => {
            println!(
                "{}\n\n{}\n\n{}\n\n{}",
                cli::USAGE,
                bar::USAGE,
                headless::USAGE,
                doctor::USAGE
            );
            Ok(())
        }
        Some(command @ ("record" | "replay"))
            if arguments
                .get(1)
                .is_some_and(|option| option.starts_with("--")) =>
        {
            headless::run(command, &arguments[1..])
        }
        Some("status") if arguments.len() > 1 => shell::show_on_the_bar(&arguments[1..]),
        Some("doctor") => doctor::run(&arguments[1..]),
        Some("--version") => {
            println!("{} {}", app::NAME, app::VERSION);
            Ok(())
        }
        Some(flag) if flag.starts_with("--") => shell::run_tooling(arguments),
        Some(_) => control(&arguments),
    };
    outcome.map_err(Failure)
}
