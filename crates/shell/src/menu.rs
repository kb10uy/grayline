//! Menu models and renderers shared by the desktop applications.

#[cfg(not(target_os = "windows"))]
mod in_window;
#[cfg(target_os = "windows")]
mod native;

#[cfg(not(target_os = "windows"))]
pub use in_window::MenuHost;
#[cfg(target_os = "windows")]
pub use native::MenuHost;

use crate::i18n::{I18n, Locale, number};

/// The message keys the entries built here are labelled with.
///
/// Every application's catalogue has to define them, and
/// [`crate::check::assert_requested_keys_defined`] holds it to that, because
/// the call sites that ask for them are here rather than in the application.
pub const KEYS: [&str; 6] = [
    "menu-zoom-in",
    "menu-zoom-out",
    "menu-zoom-reset",
    "menu-language",
    "menu-help",
    "menu-manual",
];

/// How much one zoom step changes the interface scale.
pub const ZOOM_STEP: f32 = 0.1;

/// The zoom entries of a View menu, the reset one naming the scale in use.
pub fn zoom_items<Action>(
    i18n: &I18n,
    ui_scale: f32,
    zoom_in: Action,
    zoom_out: Action,
    zoom_reset: Action,
) -> Vec<Item<Action>> {
    let percent = (ui_scale * 100.0).round();
    vec![
        Item::Command {
            label: i18n.text("menu-zoom-in"),
            action: zoom_in,
        },
        Item::Command {
            label: i18n.text("menu-zoom-out"),
            action: zoom_out,
        },
        Item::Command {
            label: i18n.text_with("menu-zoom-reset", &[("percent", number(percent))]),
            action: zoom_reset,
        },
    ]
}

/// The submenu that offers every language, the one in use checked.
pub fn language_submenu<Action>(i18n: &I18n, select: impl Fn(Locale) -> Action) -> Item<Action> {
    Item::Submenu {
        label: i18n.text("menu-language"),
        items: Locale::ALL
            .into_iter()
            .map(|locale| Item::Check {
                label: locale.to_string(),
                checked: locale == i18n.locale(),
                action: select(locale),
            })
            .collect(),
    }
}

/// The Help menu, which opens the operator's manual.
pub fn help_menu<Action>(i18n: &I18n, open_manual: Action) -> Menu<Action> {
    Menu {
        label: i18n.text("menu-help"),
        items: vec![Item::Command {
            label: i18n.text("menu-manual"),
            action: open_manual,
        }],
    }
}

/// One entry per device named, the selected one checked.
///
/// With no devices at all there is one inert entry reading `none`, so the
/// submenu says why it is empty rather than looking broken.
pub fn device_items<'a, Action>(
    names: impl IntoIterator<Item = &'a str>,
    selected: Option<&str>,
    none: String,
    select: impl Fn(String) -> Action,
) -> Vec<Item<Action>> {
    let items: Vec<_> = names
        .into_iter()
        .map(|name| Item::Check {
            label: name.to_owned(),
            checked: selected == Some(name),
            action: select(name.to_owned()),
        })
        .collect();
    if items.is_empty() {
        return vec![Item::Pending(none)];
    }
    items
}

/// One entry in an application-defined menu.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Item<Action> {
    /// A nested menu.
    Submenu {
        /// The displayed title.
        label: String,
        /// The entries in display order.
        items: Vec<Item<Action>>,
    },
    /// A command with a model-owned check mark.
    Check {
        /// The displayed label.
        label: String,
        /// Whether the check mark is set.
        checked: bool,
        /// The action returned on activation.
        action: Action,
    },
    /// An actionable menu entry.
    Command {
        /// The displayed label.
        label: String,
        /// The action returned on activation.
        action: Action,
    },
    /// An entry there is nothing to activate on.
    Pending(String),
    /// A separator between groups of entries.
    Separator,
}

/// A top-level menu and its entries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Menu<Action> {
    /// The displayed title.
    pub label: String,
    /// The entries in display order.
    pub items: Vec<Item<Action>>,
}

/// Every item of every menu, in the order a renderer creates them.
///
/// Building the platform menu and updating it later both walk this, so the
/// two cannot disagree about which entry corresponds to which item.
pub fn flatten<Action>(menus: &[Menu<Action>]) -> Vec<&Item<Action>> {
    fn walk<'a, Action>(items: &'a [Item<Action>], out: &mut Vec<&'a Item<Action>>) {
        for item in items {
            out.push(item);
            if let Item::Submenu { items, .. } = item {
                walk(items, out);
            }
        }
    }

    let mut out = Vec::new();
    for menu in menus {
        walk(&menu.items, &mut out);
    }
    out
}

/// Whether this platform uses an in-window menu by default.
pub const fn is_in_window() -> bool {
    !cfg!(target_os = "windows")
}

/// Draws the menu bar as egui widgets.
///
/// Used where the platform has no menu bar to attach to.
pub fn bar<Action: Clone>(ui: &mut egui::Ui, model: &[Menu<Action>]) -> Option<Action> {
    let mut activated = None;
    egui::MenuBar::new().ui(ui, |ui| {
        for menu in model {
            ui.menu_button(&menu.label, |ui| {
                if let Some(action) = items(ui, &menu.items) {
                    activated = Some(action);
                }
            });
        }
    });
    activated
}

fn items<Action: Clone>(ui: &mut egui::Ui, items: &[Item<Action>]) -> Option<Action> {
    let mut activated = None;
    for item in items {
        match item {
            Item::Submenu { label, items: nested } => {
                ui.menu_button(label, |ui| {
                    if let Some(action) = self::items(ui, nested) {
                        activated = Some(action);
                    }
                });
            }
            Item::Check { label, checked, action } => {
                let mut checked = *checked;
                if ui.checkbox(&mut checked, label).clicked() {
                    activated = Some(action.clone());
                    ui.close();
                }
            }
            Item::Command { label, action } => {
                if ui.button(label).clicked() {
                    activated = Some(action.clone());
                    ui.close();
                }
            }
            Item::Pending(label) => {
                ui.add_enabled(false, egui::Button::new(label));
            }
            Item::Separator => {
                ui.separator();
            }
        }
    }
    activated
}

#[cfg(test)]
mod tests;
