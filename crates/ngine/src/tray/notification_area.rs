use std::sync::Arc;

use rune::tray::{Tray as RuneTray, TrayEvent, TrayIcon, TrayItem, TraySpec};

use super::{Entry, Indicator, Model, icon};
use crate::app::Action;

type OnAction = Arc<dyn Fn(Action) + Send + Sync>;

pub struct Tray {
    shown: Option<(RuneTray, Indicator)>,
    on_action: OnAction,
}

fn items(model: &Model) -> Vec<TrayItem> {
    model
        .entries
        .iter()
        .map(|entry| match entry {
            Entry::Item {
                action,
                label,
                enabled,
            } => TrayItem::action(action.number().to_string(), label.clone()).enabled(*enabled),
            Entry::Separator => TrayItem::Separator,
        })
        .collect()
}

fn create(model: &Model, on_action: &OnAction) -> Option<RuneTray> {
    let pixels = TrayIcon::rgba(icon::SIDE, icon::SIDE, icon::pixels(model.indicator));
    let spec = TraySpec::new("ngine", "ngine", pixels)
        .tooltip(model.tooltip.clone())
        .menu(items(model));
    let on_action = Arc::clone(on_action);
    let created = RuneTray::new(spec, move |event| match event {
        TrayEvent::Activate { .. } => on_action(Action::ShowWindow),
        TrayEvent::Menu(id) => {
            if let Some(action) = id.parse().ok().and_then(Action::numbered) {
                on_action(action);
            }
        }
        _ => {}
    });
    match created {
        Ok(tray) => Some(tray),
        Err(error) => {
            eprintln!("ngine: no tray icon: {error}");
            None
        }
    }
}

impl Tray {
    pub fn new(model: &Model, on_action: impl Fn(Action) + Send + Sync + 'static) -> Option<Self> {
        let on_action: OnAction = Arc::new(on_action);
        let shown = create(model, &on_action)?;
        Some(Self {
            shown: Some((shown, model.indicator)),
            on_action,
        })
    }

    pub fn show(&mut self, model: &Model) {
        match &self.shown {
            Some((tray, indicator)) if *indicator == model.indicator => {
                tray.set_menu(items(model));
                tray.set_tooltip(model.tooltip.clone());
            }
            _ => {
                self.shown = None;
                self.shown = create(model, &self.on_action).map(|tray| (tray, model.indicator));
            }
        }
    }
}
