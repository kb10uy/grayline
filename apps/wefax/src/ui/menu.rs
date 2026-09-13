//! The application menu, described once and rendered per platform.
//!
//! Only Windows takes the native menu bar, for the reasons recorded beside the
//! SSTV application's own copy of this. The menu is built as a
//! platform-independent [model](model) that both renderers consume, so the two
//! paths cannot drift apart.
//!
//! What is on it is deliberately little: everything the operator works during
//! a reception is on the control bar under the picture, so the menu carries
//! only what is set once and then left alone.

use grayline_shell::{
    common::DEFAULT_UI_SCALE,
    i18n::{Locale, number},
};

use crate::{app::App, storage::paths::Folder};

#[cfg(target_os = "windows")]
mod native;

#[cfg(not(target_os = "windows"))]
mod in_window;

#[cfg(target_os = "windows")]
pub use native::MenuHost;

#[cfg(not(target_os = "windows"))]
pub use in_window::MenuHost;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    SelectDevice(String),
    SelectLocale(Locale),
    ToggleInferLinesPerMinute,
    ToggleNarrowShift,
    ToggleAutoSave,
    SaveChart,
    OpenManual,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Reveal(Folder),
    Quit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Item {
    Submenu {
        label: String,
        items: Vec<Item>,
    },
    Check {
        label: String,
        checked: bool,
        action: Action,
    },
    Command {
        label: String,
        action: Action,
    },
    /// An entry there is nothing to activate on.
    Pending(String),
    Separator,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Menu {
    pub label: String,
    pub items: Vec<Item>,
}

/// Describes the menu as it should currently appear.
///
/// Rebuilt every frame from application state, so check marks and labels
/// follow the interface without anything having to invalidate them.
pub fn model(app: &App) -> Vec<Menu> {
    let text = |key: &str| app.i18n.text(key);
    vec![
        Menu {
            label: text("menu-file"),
            items: file_items(app),
        },
        Menu {
            label: text("menu-view"),
            items: vec![
                Item::Command {
                    label: text("menu-zoom-in"),
                    action: Action::ZoomIn,
                },
                Item::Command {
                    label: text("menu-zoom-out"),
                    action: Action::ZoomOut,
                },
                Item::Command {
                    label: app
                        .i18n
                        .text_with("menu-zoom-reset", &[("percent", number(ui_scale_percent(app)))]),
                    action: Action::ZoomReset,
                },
            ],
        },
        Menu {
            label: text("menu-settings"),
            items: vec![
                Item::Submenu {
                    label: text("input-device"),
                    items: device_items(app),
                },
                Item::Separator,
                Item::Check {
                    label: text("action-infer-lpm"),
                    checked: app.infer_lines_per_minute,
                    action: Action::ToggleInferLinesPerMinute,
                },
                Item::Check {
                    label: text("action-narrow-shift"),
                    checked: app.narrow_shift,
                    action: Action::ToggleNarrowShift,
                },
                Item::Check {
                    label: text("action-auto-save"),
                    checked: app.auto_save,
                    action: Action::ToggleAutoSave,
                },
                Item::Separator,
                Item::Submenu {
                    label: text("menu-language"),
                    items: locale_items(app),
                },
            ],
        },
        Menu {
            label: text("menu-help"),
            items: vec![Item::Command {
                label: text("menu-manual"),
                action: Action::OpenManual,
            }],
        },
    ]
}

fn file_items(app: &App) -> Vec<Item> {
    let mut items = vec![
        Item::Command {
            label: app.i18n.text("action-save-chart"),
            action: Action::SaveChart,
        },
        Item::Separator,
    ];
    items.extend(Folder::ALL.into_iter().map(|folder| Item::Command {
        label: app.i18n.text(folder.label_key()),
        action: Action::Reveal(folder),
    }));
    items.push(Item::Separator);
    items.push(Item::Command {
        label: app.i18n.text("menu-quit"),
        action: Action::Quit,
    });
    items
}

fn ui_scale_percent(app: &App) -> f32 {
    (app.ui_scale * 100.0).round()
}

fn device_items(app: &App) -> Vec<Item> {
    if app.audio.devices.is_empty() {
        return vec![Item::Pending(app.i18n.text("status-no-audio"))];
    }
    let selected = app.audio.device.as_ref().map(|device| device.name());
    app.audio
        .devices
        .iter()
        .map(|device| Item::Check {
            label: device.name().to_owned(),
            checked: selected == Some(device.name()),
            action: Action::SelectDevice(device.name().to_owned()),
        })
        .collect()
}

fn locale_items(app: &App) -> Vec<Item> {
    Locale::ALL
        .into_iter()
        .map(|locale| Item::Check {
            label: locale.to_string(),
            checked: locale == app.i18n.locale(),
            action: Action::SelectLocale(locale),
        })
        .collect()
}

/// Applies `action` to the application.
///
/// Returns whether the application was asked to close.
pub fn apply(app: &mut App, action: Action) -> bool {
    match action {
        Action::SelectDevice(name) => app.select_device_named(&name),
        Action::SelectLocale(locale) => app.select_locale(locale),
        Action::ToggleInferLinesPerMinute => {
            app.infer_lines_per_minute = !app.infer_lines_per_minute;
            app.push_settings();
        }
        Action::ToggleNarrowShift => {
            app.narrow_shift = !app.narrow_shift;
            app.push_settings();
        }
        Action::ToggleAutoSave => app.auto_save = !app.auto_save,
        Action::SaveChart => app.save_chart(),
        Action::OpenManual => app.open_manual(),
        Action::ZoomIn => app.zoom_by(ZOOM_STEP),
        Action::ZoomOut => app.zoom_by(-ZOOM_STEP),
        Action::ZoomReset => app.set_ui_scale(DEFAULT_UI_SCALE),
        Action::Reveal(folder) => app.reveal(folder),
        Action::Quit => return true,
    }
    false
}

const ZOOM_STEP: f32 = 0.1;

/// Every item of every menu, in the order a renderer creates them.
///
/// Building the platform menu and updating it later both walk this, so the two
/// cannot disagree about which entry corresponds to which item.
#[cfg(any(target_os = "windows", test))]
pub fn flatten(menus: &[Menu]) -> Vec<&Item> {
    fn walk<'a>(items: &'a [Item], out: &mut Vec<&'a Item>) {
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

pub const fn is_in_window() -> bool {
    !cfg!(target_os = "windows")
}

/// Draws the menu bar as egui widgets.
///
/// Used where the platform has no menu bar to attach to.
pub fn bar(ui: &mut egui::Ui, model: &[Menu]) -> Option<Action> {
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

fn items(ui: &mut egui::Ui, items: &[Item]) -> Option<Action> {
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
mod tests {
    use super::*;
    use crate::app::App;

    fn count(items: &[Item]) -> usize {
        items
            .iter()
            .map(|item| match item {
                Item::Submenu { items, .. } => 1 + count(items),
                _ => 1,
            })
            .sum()
    }

    #[test]
    fn flattening_counts_every_item_including_separators() {
        let model = model(&App::headless());
        let expected: usize = model.iter().map(|menu| count(&menu.items)).sum();
        assert_eq!(flatten(&model).len(), expected);
        assert!(
            model.iter().any(|menu| menu.items.contains(&Item::Separator)),
            "the model needs a separator for this to be worth asserting"
        );
    }

    #[test]
    fn flattening_visits_a_submenu_before_its_contents() {
        let model = vec![Menu {
            label: "settings".to_owned(),
            items: vec![
                Item::Separator,
                Item::Submenu {
                    label: "outer".to_owned(),
                    items: vec![Item::Pending("inner".to_owned())],
                },
                Item::Pending("after".to_owned()),
            ],
        }];
        let flat = flatten(&model);
        assert!(matches!(flat[0], Item::Separator));
        assert!(matches!(flat[1], Item::Submenu { .. }));
        assert!(matches!(flat[2], Item::Pending(label) if label == "inner"));
        assert!(matches!(flat[3], Item::Pending(label) if label == "after"));
    }

    #[test]
    fn the_file_menu_offers_every_folder() {
        let app = App::headless();
        let model = model(&app);
        let offered: Vec<Folder> = model[0]
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Command {
                    action: Action::Reveal(folder),
                    ..
                } => Some(*folder),
                _ => None,
            })
            .collect();

        assert_eq!(offered, Folder::ALL);
        for folder in Folder::ALL {
            let key = folder.label_key();
            assert_ne!(app.i18n.text(key), key, "{key} is not translated");
        }
    }

    #[test]
    fn every_action_in_the_model_is_applicable() {
        let mut app = App::headless();
        for item in flatten(&model(&App::headless())) {
            let action = match item {
                Item::Check { action, .. } | Item::Command { action, .. } => action.clone(),
                _ => continue,
            };
            let quits = matches!(action, Action::Quit);
            assert_eq!(apply(&mut app, action), quits);
        }
    }

    #[test]
    fn opening_the_manual_reports_nothing() {
        let mut app = App::headless();
        apply(&mut app, Action::OpenManual);
        assert_eq!(app.notice, None);
    }
}
