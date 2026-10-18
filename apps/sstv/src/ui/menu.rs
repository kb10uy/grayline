//! The application menu, described once and rendered per platform.
//!
//! Only Windows takes the native menu bar. muda cannot attach one on Linux —
//! it needs a gtk window there and winit does not create one — and on macOS
//! it could, but the project has no Mac to verify the native bar's behavior
//! on, so macOS draws the same in-window bar. Rather than maintain two menu
//! definitions, the menu is built as a platform-independent [model](model)
//! that the native and in-window renderers both consume, so the two paths
//! cannot drift apart.

use grayline_shell::{common::DEFAULT_UI_SCALE, i18n::Locale, menu as shared_menu};

use crate::{
    app::App,
    storage::{history::HistoryFormat, paths::Folder},
};

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
                            checked: app.settings.send_fskid,
                            action: Action::ToggleSendFskid,
                        },
                        Item::Check {
                            label: text("action-contest-mode"),
                            checked: app.settings.contest_mode,
                            action: Action::ToggleContestMode,
                        },
                    ],
                },
                Item::Submenu {
                    label: text("menu-receive"),
                    items: vec![
                        Item::Check {
                            label: text("action-vis-restart"),
                            checked: app.settings.vis_restart,
                            action: Action::ToggleVisRestart,
                        },
                        Item::Check {
                            label: text("action-vis-strict"),
                            checked: app.settings.vis_strict,
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
                            checked: app.settings.contact.lookup,
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
                shared_menu::language_submenu(&app.i18n, Action::SelectLocale),
            ],
        },
        shared_menu::help_menu(&app.i18n, Action::OpenManual),
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
            checked: app.settings.auto_history,
            action: Action::ToggleAutoHistory,
        },
        Item::Separator,
    ];
    items.extend(HistoryFormat::ALL.into_iter().map(|format| Item::Check {
        label: app.i18n.text(format.label_key()),
        checked: app.settings.history_format == format,
        action: Action::SelectHistoryFormat(format),
    }));
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
        Action::SelectDevice(name) => app.select_device_named(&name),
        Action::SelectOutputDevice(name) => app.select_output_device_named(&name),
        Action::SelectLocale(locale) => app.select_locale(locale),
        Action::ShowStation => app.station_dialog_open = true,
        Action::ShowCustomVariables => app.open_custom_variables(),
        Action::ToggleSendFskid => app.settings.send_fskid = !app.settings.send_fskid,
        Action::ToggleContestMode => app.settings.contest_mode = !app.settings.contest_mode,
        Action::ToggleVisRestart => app.set_vis_restart(!app.settings.vis_restart),
        Action::ToggleVisStrict => app.set_vis_strict(!app.settings.vis_strict),
        Action::ToggleContactLookup => app.set_contact_lookup(!app.settings.contact.lookup),
        Action::WriteContactCredentials => app.write_contact_credentials(),
        Action::WriteRigScript => app.write_rig_script(),
        Action::WriteBandPlan => app.write_band_plan(),
        Action::ToggleAutoHistory => app.settings.auto_history = !app.settings.auto_history,
        Action::SelectHistoryFormat(format) => app.settings.history_format = format,
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

        assert_eq!(app.error, None);
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
}
