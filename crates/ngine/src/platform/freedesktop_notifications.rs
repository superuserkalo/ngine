use std::collections::HashMap;
use std::future::poll_fn;
use std::pin::Pin;
use std::process::Command;
use std::thread;

use futures_core::Stream;
use zbus::message::Type;
use zbus::{Connection, MatchRule, Message, MessageStream, Variant};

use crate::notification::{APP_ICON, APP_NAME, Hint, Notification};

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const ACTION_INVOKED: &str = "ActionInvoked";
const CLOSED: &str = "NotificationClosed";

fn value(hint: &Hint) -> Variant<'_> {
    match hint {
        Hint::Byte(byte) => Variant::from(*byte),
        Hint::Text(text) => Variant::from(text.as_str()),
    }
}

fn run(argv: &[String]) {
    let Some((program, arguments)) = argv.split_first() else {
        return;
    };
    match Command::new(program).args(arguments).spawn() {
        Ok(mut opened) => {
            let _opener_finished = opened.wait();
        }
        Err(error) => eprintln!("ngine: {program} could not be started: {error}"),
    }
}

async fn next(signals: &mut MessageStream) -> Option<zbus::Result<Message>> {
    poll_fn(|context| Pin::new(&mut *signals).poll_next(context)).await
}

async fn show_and_follow(notification: &Notification) -> zbus::Result<()> {
    let connection = Connection::session().await?;
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface(SERVICE)
        .path(PATH)
        .build()?;
    let mut signals = MessageStream::for_match_rule(rule, &connection, None).await?;
    let hints = notification.hints();
    let hints: HashMap<&str, Variant<'_>> = hints
        .iter()
        .map(|(name, hint)| (*name, value(hint)))
        .collect();
    let actions = notification.actions();
    let reply = connection
        .call_method(
            Some(SERVICE),
            PATH,
            Some(SERVICE),
            "Notify",
            &(
                APP_NAME,
                0_u32,
                APP_ICON,
                notification.summary.as_str(),
                notification.body.as_str(),
                actions,
                hints,
                notification.expire_ms,
            ),
        )
        .await?;
    let shown: u32 = reply.body().deserialize()?;
    while let Some(message) = next(&mut signals).await {
        let message = message?;
        let header = message.header();
        match header.member().map(zbus::names::MemberName::as_str) {
            Some(ACTION_INVOKED) => {
                let (id, action): (u32, String) = message.body().deserialize()?;
                if id == shown {
                    if let Some(argv) = notification.command_for(&action) {
                        run(&argv);
                    }
                    return Ok(());
                }
            }
            Some(CLOSED) => {
                let (id, _reason): (u32, u32) = message.body().deserialize()?;
                if id == shown {
                    return Ok(());
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Shows the notification through the desktop's notification service and,
/// on a thread of its own, carries out the action the person clicks until
/// the notification closes.
pub fn show(notification: Notification) {
    let spawned = thread::Builder::new()
        .name("ngine-notification".into())
        .spawn(move || {
            if let Err(error) = zbus::block_on(show_and_follow(&notification)) {
                eprintln!("ngine: the desktop notification failed: {error}");
            }
        });
    if let Err(error) = spawned {
        eprintln!("ngine: the desktop notification could not start: {error}");
    }
}
