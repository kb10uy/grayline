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

use grayline_shell::{
    common::DEFAULT_UI_SCALE,
    i18n::{Locale, number},
    menu as shared_menu,
};

use crate::{app::App, storage::paths::Folder};

pub type MenuHost = shared_menu::MenuHost<Action>;

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

fn output_device_items(app: &App) -> Vec<Item> {
    if app.audio.output_devices.is_empty() {
        return vec![Item::Pending(app.i18n.text("status-no-output"))];
    }
    let selected = app.audio.output_device.as_ref().map(|device| device.name());
    app.audio
        .output_devices
        .iter()
        .map(|device| Item::Check {
            label: device.name().to_owned(),
            checked: selected == Some(device.name()),
            action: Action::SelectOutputDevice(device.name().to_owned()),
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
        Action::ZoomIn => app.zoom_by(ZOOM_STEP),
        Action::ZoomOut => app.zoom_by(-ZOOM_STEP),
        Action::ZoomReset => app.set_ui_scale(DEFAULT_UI_SCALE),
        Action::Reveal(folder) => app.reveal(folder),
        Action::Quit => return true,
    }
    false
}

const ZOOM_STEP: f32 = 0.1;

#[cfg(test)]
pub fn flatten(menus: &[Menu]) -> Vec<&Item> {
    shared_menu::flatten(menus)
}

pub const fn is_in_window() -> bool {
    shared_menu::is_in_window()
}

pub fn bar(ui: &mut egui::Ui, model: &[Menu]) -> Option<Action> {
    shared_menu::bar(ui, model)
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
