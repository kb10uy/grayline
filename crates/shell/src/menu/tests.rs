use egui_kittest::{Harness, kittest::Queryable};

use super::{Item, Menu, bar, flatten};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Action {
    Toggle,
    Open,
    Quit,
}

pub(super) fn model(japanese: bool) -> Vec<Menu<Action>> {
    let words = if japanese {
        ["ファイル", "開く", "設定", "有効", "未接続", "終了"]
    } else {
        ["File", "Open", "Settings", "Enabled", "Disconnected", "Quit"]
    };
    vec![Menu {
        label: words[0].to_owned(),
        items: vec![
            Item::Command {
                label: words[1].to_owned(),
                action: Action::Open,
            },
            Item::Submenu {
                label: words[2].to_owned(),
                items: vec![Item::Check {
                    label: words[3].to_owned(),
                    checked: true,
                    action: Action::Toggle,
                }],
            },
            Item::Pending(words[4].to_owned()),
            Item::Separator,
            Item::Command {
                label: words[5].to_owned(),
                action: Action::Quit,
            },
        ],
    }]
}

#[test]
fn flattening_keeps_separators_and_visits_parents_before_children() {
    let menus = model(false);
    let flat = flatten(&menus);
    assert_eq!(flat.len(), 6);
    assert!(matches!(flat[1], Item::Submenu { .. }));
    assert!(matches!(
        flat[2],
        Item::Check {
            action: Action::Toggle,
            ..
        }
    ));
    assert!(matches!(flat[4], Item::Separator));
    assert!(matches!(
        flat[5],
        Item::Command {
            action: Action::Quit,
            ..
        }
    ));
}

#[test]
fn an_in_window_command_returns_the_applications_action() {
    let menus = model(false);
    let mut selected = None;
    let mut harness = Harness::new_ui(|ui| {
        if let Some(action) = bar(ui, &menus) {
            selected = Some(action);
        }
    });
    harness.run();
    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label("Open").click();
    harness.run();
    drop(harness);
    assert_eq!(selected, Some(Action::Open));
}

#[test]
fn an_in_window_check_returns_an_action_without_changing_the_model() {
    let menus = vec![Menu {
        label: "Settings".to_owned(),
        items: vec![Item::Check {
            label: "Enabled".to_owned(),
            checked: true,
            action: Action::Toggle,
        }],
    }];
    let mut selected = None;
    let mut harness = Harness::new_ui(|ui| {
        if let Some(action) = bar(ui, &menus) {
            selected = Some(action);
        }
    });
    harness.run();
    harness.get_by_label("Settings").click();
    harness.run();
    harness.get_by_label("Enabled").click();
    harness.run();
    drop(harness);
    assert_eq!(selected, Some(Action::Toggle));
    assert!(matches!(menus[0].items[0], Item::Check { checked: true, .. }));
}
