//! Menu models and renderers shared by the desktop applications.

#[cfg(not(target_os = "windows"))]
mod in_window;
#[cfg(target_os = "windows")]
mod native;

#[cfg(not(target_os = "windows"))]
pub use in_window::MenuHost;
#[cfg(target_os = "windows")]
pub use native::MenuHost;

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
