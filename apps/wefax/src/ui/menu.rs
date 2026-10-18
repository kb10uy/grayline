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

use grayline_shell::{common::DEFAULT_UI_SCALE, i18n::Locale, menu as shared_menu};

use crate::{app::App, storage::paths::Folder};

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

pub type Item = shared_menu::Item<Action>;
pub type Menu = shared_menu::Menu<Action>;

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
            items: shared_menu::zoom_items(
                &app.i18n,
                app.ui_scale,
                Action::ZoomIn,
                Action::ZoomOut,
                Action::ZoomReset,
            ),
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
                shared_menu::language_submenu(&app.i18n, Action::SelectLocale),
            ],
        },
        shared_menu::help_menu(&app.i18n, Action::OpenManual),
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

fn device_items(app: &App) -> Vec<Item> {
    shared_menu::device_items(
        app.audio.devices.iter().map(|device| device.name()),
        app.audio.device.as_ref().map(|device| device.name()),
        app.i18n.text("status-no-audio"),
        Action::SelectDevice,
    )
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
        Action::ZoomIn => app.zoom_by(shared_menu::ZOOM_STEP),
        Action::ZoomOut => app.zoom_by(-shared_menu::ZOOM_STEP),
        Action::ZoomReset => app.set_ui_scale(DEFAULT_UI_SCALE),
        Action::Reveal(folder) => app.reveal(folder),
        Action::Quit => return true,
    }
    false
}

pub fn bar(ui: &mut egui::Ui, model: &[Menu]) -> Option<Action> {
    shared_menu::bar(ui, model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

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
        for item in shared_menu::flatten(&model(&App::headless())) {
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
