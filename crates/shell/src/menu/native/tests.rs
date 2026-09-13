use super::{Entry, MenuHost};
use crate::menu::{
    Item,
    tests::{Action, model},
};

#[test]
fn a_fresh_menu_keeps_nested_labels_in_model_order() {
    let native = MenuHost::detached(&model(false));
    assert_eq!(
        native.labels(),
        ["File", "Open", "Settings", "Enabled", "Disconnected", "-", "Quit"]
    );
    assert_eq!(native.checks(), [true]);
}

#[test]
fn activating_an_unchanged_selection_restores_its_check() {
    let menus = model(false);
    let mut native = MenuHost::detached(&menus);
    native.flip_checks();
    assert_eq!(native.checks(), [false]);
    native.sync(&menus);
    native.restore_checks();
    assert_eq!(native.checks(), [true]);
    let mut changed = menus;
    if let Item::Submenu { items, .. } = &mut changed[0].items[1]
        && let Item::Check { checked, .. } = &mut items[0]
    {
        *checked = false;
    }
    native.sync(&changed);
    assert_eq!(native.checks(), [false]);
}

#[test]
fn relabelling_updates_existing_entries_without_drift() {
    let mut native = MenuHost::detached(&model(false));
    let entry_id = match &native.items[0] {
        Entry::Command(entry) => entry.id().clone(),
        _ => unreachable!(),
    };
    for japanese in [true, false, true, false, true] {
        native.sync(&model(japanese));
    }
    assert_eq!(
        native.labels(),
        ["ファイル", "開く", "設定", "有効", "未接続", "-", "終了"]
    );
    let Entry::Command(entry) = &native.items[0] else {
        unreachable!();
    };
    assert_eq!(entry.id(), &entry_id);
    assert_eq!(native.actions[&entry_id], Action::Open);
}

#[test]
fn a_structure_change_rebuilds_entries_and_discards_old_actions() {
    let mut menus = model(false);
    let mut native = MenuHost::detached(&menus);
    let old_ids: Vec<_> = native.actions.keys().cloned().collect();
    menus[0].items = vec![Item::Command {
        label: "Close".to_owned(),
        action: Action::Quit,
    }];
    native.sync(&menus);
    assert_eq!(native.labels(), ["File", "Close"]);
    assert!(old_ids.iter().all(|id| !native.actions.contains_key(id)));
    assert_eq!(native.actions.values().collect::<Vec<_>>(), [&Action::Quit]);
}
