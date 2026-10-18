//! The application menu, described once and rendered per platform.
//!
//! Only Windows takes the native menu bar, for the reasons recorded beside the
//! SSTV application's own copy of this. The menu is built as a
//! platform-independent [model](model) that both renderers consume, so the two
//! paths cannot drift apart.
//!
//! What is on it is deliberately little: everything the operator works while
//! listening is on the panel beside the text, so the menu carries only what is
//! set once and then left alone.

use grayline_shell::{common::DEFAULT_UI_SCALE, i18n::Locale, menu as shared_menu};

use crate::{app::App, storage::paths::Folder};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    ShowStation,
    ToggleContactLookup,
    WriteContactCredentials,
    SelectDevice(String),
    SelectOutputDevice(String),
    SelectLocale(Locale),
    ToggleScope,
    ToggleUnshiftOnSpace,
    ToggleAtc,
    ClearText,
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
            items: vec![
                Item::Check {
                    label: text("action-scope"),
                    checked: app.scope.is_open(),
                    action: Action::ToggleScope,
                },
                Item::Separator,
            ]
            .into_iter()
            .chain(shared_menu::zoom_items(
                &app.i18n,
                app.ui_scale,
                Action::ZoomIn,
                Action::ZoomOut,
                Action::ZoomReset,
            ))
            .collect(),
        },
        Menu {
            label: text("menu-settings"),
            items: vec![
                Item::Command {
                    label: text("menu-station"),
                    action: Action::ShowStation,
                },
                Item::Submenu {
                    label: text("menu-contact"),
                    items: vec![
                        Item::Check {
                            label: text("action-contact-lookup"),
                            checked: app.contact_settings.lookup,
                            action: Action::ToggleContactLookup,
                        },
                        Item::Command {
                            label: text("action-contact-write-credentials"),
                            action: Action::WriteContactCredentials,
                        },
                    ],
                },
                Item::Separator,
                Item::Submenu {
                    label: text("input-device"),
                    items: device_items(app),
                },
                Item::Submenu {
                    label: text("output-device"),
                    items: output_device_items(app),
                },
                Item::Separator,
                Item::Check {
                    label: text("action-unshift-on-space"),
                    checked: app.unshift_on_space,
                    action: Action::ToggleUnshiftOnSpace,
                },
                Item::Check {
                    label: text("action-atc"),
                    checked: app.atc,
                    action: Action::ToggleAtc,
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
            label: app.i18n.text("action-clear"),
            action: Action::ClearText,
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

fn output_device_items(app: &App) -> Vec<Item> {
    shared_menu::device_items(
        app.audio.output_devices.iter().map(|device| device.name()),
        app.audio.output_device.as_ref().map(|device| device.name()),
        app.i18n.text("status-no-output"),
        Action::SelectOutputDevice,
    )
}

/// Applies `action` to the application.
///
/// Returns whether the application was asked to close.
pub fn apply(app: &mut App, action: Action) -> bool {
    match action {
        Action::ShowStation => app.open_station(),
        Action::ToggleContactLookup => app.set_contact_lookup(!app.contact_settings.lookup),
        Action::WriteContactCredentials => app.write_contact_credentials(),
        Action::SelectDevice(name) => app.select_device_named(&name),
        Action::SelectOutputDevice(name) => app.select_output_device_named(&name),
        Action::SelectLocale(locale) => app.select_locale(locale),
        Action::ToggleScope => {
            let open = app.scope.is_open();
            app.set_scope_open(!open);
        }
        Action::ToggleUnshiftOnSpace => {
            app.unshift_on_space = !app.unshift_on_space;
            app.push_settings();
        }
        Action::ToggleAtc => {
            app.atc = !app.atc;
            app.push_settings();
        }
        Action::ClearText => app.clear(),
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
    fn the_settings_on_the_menu_reach_the_receiver() {
        let mut app = App::headless();
        let before = app.audio.settings();
        apply(&mut app, Action::ToggleUnshiftOnSpace);
        apply(&mut app, Action::ToggleAtc);

        let after = app.audio.settings();
        assert_ne!(after.unshift_on_space, before.unshift_on_space);
        assert_ne!(after.atc, before.atc);
    }

    #[test]
    fn opening_the_manual_reports_nothing() {
        let mut app = App::headless();
        apply(&mut app, Action::OpenManual);
        assert_eq!(app.notice, None);
    }
}
