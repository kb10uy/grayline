//! The application menu, described once and rendered per platform.
//!
//! Only Windows takes the native menu bar. muda cannot attach one on Linux —
//! it needs a gtk window there and winit does not create one — and on macOS
//! it could, but the project has no Mac to verify the native bar's behavior
//! on, so macOS draws the same in-window bar. Rather than maintain two menu
//! definitions, the menu is built as a platform-independent [model](model)
//! that the native and in-window renderers both consume, so the two paths
//! cannot drift apart.

use grayline_shell::{
    common::DEFAULT_UI_SCALE,
    i18n::{Locale, number},
    menu as shared_menu,
};

use crate::{
    app::App,
    storage::{history::HistoryFormat, paths::Folder},
};

pub type MenuHost = shared_menu::MenuHost<Action>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    SelectDevice(String),
    SelectOutputDevice(String),
    SelectLocale(Locale),
    ShowStation,
    ShowCustomVariables,
    ToggleSendFskid,
    ToggleContestMode,
    ToggleVisRestart,
    ToggleVisStrict,
    ToggleContactLookup,
    WriteContactCredentials,
    WriteRigScript,
    WriteBandPlan,
    ToggleAutoHistory,
    SelectHistoryFormat(HistoryFormat),
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
            items: folder_items(app),
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
                Item::Command {
                    label: text("menu-station"),
                    action: Action::ShowStation,
                },
                Item::Command {
                    label: text("menu-custom-variables"),
                    action: Action::ShowCustomVariables,
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
                Item::Submenu {
                    label: text("menu-transmit"),
                    items: vec![
                        Item::Check {
                            label: text("action-send-fskid"),
                            checked: app.send_fskid,
                            action: Action::ToggleSendFskid,
                        },
                        Item::Check {
                            label: text("action-contest-mode"),
                            checked: app.contest_mode,
                            action: Action::ToggleContestMode,
                        },
                    ],
                },
                Item::Submenu {
                    label: text("menu-receive"),
                    items: vec![
                        Item::Check {
                            label: text("action-vis-restart"),
                            checked: app.vis_restart,
                            action: Action::ToggleVisRestart,
                        },
                        Item::Check {
                            label: text("action-vis-strict"),
                            checked: app.vis_strict,
                            action: Action::ToggleVisStrict,
                        },
                    ],
                },
                Item::Submenu {
                    label: text("menu-history"),
                    items: history_items(app),
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
                Item::Submenu {
                    label: text("menu-rig"),
                    items: vec![
                        Item::Command {
                            label: text("action-rig-write-script"),
                            action: Action::WriteRigScript,
                        },
                        Item::Command {
                            label: text("action-rig-write-bands"),
                            action: Action::WriteBandPlan,
                        },
                    ],
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

/// The File menu: every directory the application keeps, and then Quit.
///
/// The application stores nothing of its own, so opening a folder is the whole
/// of what File has to offer; the entries are built from [`Folder::ALL`] so a
/// new directory cannot be added without also being reachable.
fn folder_items(app: &App) -> Vec<Item> {
    Folder::ALL
        .into_iter()
        .map(|folder| Item::Command {
            label: app.i18n.text(folder.label_key()),
            action: Action::Reveal(folder),
        })
        .chain([
            Item::Separator,
            Item::Command {
                label: app.i18n.text("menu-quit"),
                action: Action::Quit,
            },
        ])
        .collect()
}

fn history_items(app: &App) -> Vec<Item> {
    let mut items = vec![
        Item::Check {
            label: app.i18n.text("action-auto-history"),
            checked: app.auto_history,
            action: Action::ToggleAutoHistory,
        },
        Item::Separator,
    ];
    items.extend(HistoryFormat::ALL.into_iter().map(|format| Item::Check {
        label: app.i18n.text(format.label_key()),
        checked: app.history_format == format,
        action: Action::SelectHistoryFormat(format),
    }));
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

/// Applies `action` to the application.
///
/// Returns whether the application was asked to close.
pub fn apply(app: &mut App, action: Action) -> bool {
    match action {
        Action::SelectDevice(name) => app.select_device_named(&name),
        Action::SelectOutputDevice(name) => app.select_output_device_named(&name),
        Action::SelectLocale(locale) => app.select_locale(locale),
        Action::ShowStation => app.station.open = true,
        Action::ShowCustomVariables => app.open_custom_variables(),
        Action::ToggleSendFskid => app.send_fskid = !app.send_fskid,
        Action::ToggleContestMode => app.contest_mode = !app.contest_mode,
        Action::ToggleVisRestart => app.set_vis_restart(!app.vis_restart),
        Action::ToggleVisStrict => app.set_vis_strict(!app.vis_strict),
        Action::ToggleContactLookup => app.set_contact_lookup(!app.contact_settings.lookup),
        Action::WriteContactCredentials => app.write_contact_credentials(),
        Action::WriteRigScript => app.write_rig_script(),
        Action::WriteBandPlan => app.write_band_plan(),
        Action::ToggleAutoHistory => app.auto_history = !app.auto_history,
        Action::SelectHistoryFormat(format) => app.history_format = format,
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
    fn the_help_menu_opens_the_manual() {
        let app = App::headless();
        let model = model(&app);
        let help = model.last().expect("the menu should end with Help");

        assert!(matches!(
            help.items.as_slice(),
            [Item::Command {
                action: Action::OpenManual,
                ..
            }]
        ));
        assert_ne!(app.i18n.text("menu-manual"), "menu-manual");
    }

    #[test]
    fn opening_the_manual_reports_nothing() {
        let mut app = App::headless();

        apply(&mut app, Action::OpenManual);

        assert_eq!(app.library.error, None);
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
}
