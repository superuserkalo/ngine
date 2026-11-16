#![expect(
    unsafe_code,
    reason = "AppKit hands a menu item its target and action only through unsafe calls, and Rune has no macOS tray"
)]

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::NSString;

use super::{Entry, Indicator, Model};
use crate::app::Action;

struct Callback {
    on_action: Box<dyn Fn(Action)>,
}

define_class!(
    // SAFETY: NSObject places no requirements on its subclasses, and MenuTarget has no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Callback]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(pick:))]
        fn pick(&self, item: &NSMenuItem) {
            let picked = usize::try_from(item.tag()).ok().and_then(Action::numbered);
            if let Some(action) = picked {
                (self.ivars().on_action)(action);
            }
        }
    }
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker, on_action: Box<dyn Fn(Action)>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Callback { on_action });
        // SAFETY: NSObject's init takes no arguments and returns the receiver it was sent to.
        unsafe { msg_send![super(this), init] }
    }
}

pub struct Tray {
    item: Retained<NSStatusItem>,
    target: Retained<MenuTarget>,
    mtm: MainThreadMarker,
}

fn symbol(indicator: Indicator) -> &'static str {
    match indicator {
        Indicator::Idle => "circle",
        Indicator::ReplayOn => "arrow.counterclockwise.circle",
        Indicator::Recording => "record.circle.fill",
    }
}

fn tag(action: Action) -> isize {
    isize::try_from(action.number()).unwrap_or(-1)
}

impl Tray {
    /// Returns `None` when called off the main thread.
    pub fn new(model: &Model, on_action: impl Fn(Action) + Send + Sync + 'static) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let tray = Self {
            item,
            target: MenuTarget::new(mtm, Box::new(on_action)),
            mtm,
        };
        tray.show(model);
        Some(tray)
    }

    fn menu_item(&self, entry: &Entry) -> Retained<NSMenuItem> {
        let Entry::Item {
            action,
            label,
            enabled,
        } = entry
        else {
            return NSMenuItem::separatorItem(self.mtm);
        };
        let target: &AnyObject = &self.target;
        // SAFETY: `pick:` is the one method MenuTarget defines, and it takes the menu item
        // that AppKit passes as the sender.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm),
                &NSString::from_str(label),
                Some(sel!(pick:)),
                &NSString::from_str(""),
            )
        };
        // SAFETY: the target outlives the menu, because both belong to this Tray and the
        // status item is removed before either is released.
        unsafe { item.setTarget(Some(target)) };
        item.setTag(tag(*action));
        item.setEnabled(*enabled);
        item
    }

    pub fn show(&self, model: &Model) {
        let menu = NSMenu::new(self.mtm);
        menu.setAutoenablesItems(false);
        for entry in &model.entries {
            menu.addItem(&self.menu_item(entry));
        }
        self.item.setMenu(Some(&menu));
        let Some(button) = self.item.button(self.mtm) else {
            return;
        };
        let description = NSString::from_str(&model.tooltip);
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol(model.indicator)),
            Some(&description),
        );
        match &image {
            Some(image) => button.setImage(Some(image)),
            None => button.setTitle(&NSString::from_str("ngine")),
        }
        button.setToolTip(Some(&description));
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}
