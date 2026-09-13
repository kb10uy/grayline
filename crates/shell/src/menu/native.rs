use std::collections::HashMap;

use muda::{CheckMenuItem, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};

use super::{Item, Menu};

/// The platform menu bar, kept in step with the model.
///
/// The menu is built once and then updated in place. Rebuilding it every
/// frame would be visible to the window manager, and on Windows would
/// re-measure the client area on each pass.
pub struct MenuHost<Action> {
    menu: muda::Menu,
    /// The top-level menus, in bar order.
    bar: Vec<Submenu>,
    /// Every entry below the bar, in [`super::flatten`] order.
    ///
    /// Held apart from `bar` so each vector lines up with one sequence
    /// from the model. Interleaving the two is what once let the update
    /// walk drift and write labels onto the wrong entries.
    items: Vec<Entry>,
    actions: HashMap<MenuId, Action>,
    model: Vec<Menu<Action>>,
    hwnd: Option<isize>,
}

enum Entry {
    Check(CheckMenuItem),
    Command(MenuItem),
    Pending(MenuItem),
    Submenu(Submenu),
    Separator,
}

impl Entry {
    fn update<Action>(&self, item: &Item<Action>) {
        match (self, item) {
            (Self::Submenu(entry), Item::Submenu { label, .. }) => entry.set_text(label),
            (Self::Check(entry), Item::Check { label, checked, .. }) => {
                entry.set_text(label);
                entry.set_checked(*checked);
            }
            (Self::Command(entry), Item::Command { label, .. }) | (Self::Pending(entry), Item::Pending(label)) => {
                entry.set_text(label)
            }
            (Self::Separator, Item::Separator) => {}
            _ => debug_assert!(false, "menu entries drifted from the model"),
        }
    }
}

impl<Action: Clone + PartialEq> MenuHost<Action> {
    /// Builds the menu and attaches it to the platform.
    ///
    /// A failure here is reported rather than fatal: the application is
    /// usable without a menu bar, and the in-window controls still work.
    pub fn install(cc: &eframe::CreationContext<'_>, model: &[Menu<Action>]) -> Result<Self, muda::Error> {
        let menu = muda::Menu::new();
        let mut native = Self {
            menu,
            bar: Vec::new(),
            items: Vec::new(),
            actions: HashMap::new(),
            model: Vec::new(),
            hwnd: None,
        };
        native.build(model)?;
        native.attach(cc)?;
        Ok(native)
    }

    fn attach(&mut self, cc: &eframe::CreationContext<'_>) -> Result<(), muda::Error> {
        use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

        let Ok(handle) = cc.window_handle() else {
            return Ok(());
        };
        if let RawWindowHandle::Win32(window) = handle.as_raw() {
            // muda installs its own window subclass on this handle and
            // answers WM_COMMAND itself, so the winit event loop needs no
            // cooperation for menu clicks.
            let hwnd = window.hwnd.get();
            unsafe { self.menu.init_for_hwnd(hwnd) }?;
            self.hwnd = Some(hwnd);
        }
        Ok(())
    }

    /// Hides the native window before closing its menu.
    pub fn prepare_for_close(&self) {
        if let Some(hwnd) = self.hwnd {
            crate::platform::hide_window(hwnd);
        }
    }

    fn build(&mut self, model: &[Menu<Action>]) -> Result<(), muda::Error> {
        while self.menu.remove_at(0).is_some() {}
        self.bar.clear();
        self.items.clear();
        self.actions.clear();

        for menu in model {
            let submenu = Submenu::new(&menu.label, true);
            self.menu.append(&submenu)?;
            self.bar.push(submenu.clone());
            self.append_items(&submenu, &menu.items)?;
        }
        self.model = model.to_vec();
        Ok(())
    }

    fn append_items(&mut self, parent: &Submenu, items: &[Item<Action>]) -> Result<(), muda::Error> {
        for item in items {
            match item {
                Item::Submenu { label, items } => {
                    let submenu = Submenu::new(label, true);
                    parent.append(&submenu)?;
                    self.items.push(Entry::Submenu(submenu.clone()));
                    self.append_items(&submenu, items)?;
                }
                Item::Check { label, checked, action } => {
                    let entry = CheckMenuItem::new(label, true, *checked, None);
                    parent.append(&entry)?;
                    self.actions.insert(entry.id().clone(), action.clone());
                    self.items.push(Entry::Check(entry));
                }
                Item::Command { label, action } => {
                    let entry = MenuItem::new(label, true, None);
                    parent.append(&entry)?;
                    self.actions.insert(entry.id().clone(), action.clone());
                    self.items.push(Entry::Command(entry));
                }
                Item::Pending(label) => {
                    let entry = MenuItem::new(label, false, None);
                    parent.append(&entry)?;
                    self.items.push(Entry::Pending(entry));
                }
                Item::Separator => {
                    parent.append(&PredefinedMenuItem::separator())?;
                    self.items.push(Entry::Separator);
                }
            }
        }
        Ok(())
    }

    /// Brings the platform menu in line with `model`.
    ///
    /// Labels and check marks are written in place while the structure
    /// matches; a structural change, such as a device appearing, falls
    /// back to a rebuild.
    pub fn sync(&mut self, model: &[Menu<Action>]) {
        if self.model == model {
            return;
        }
        if structure_of(&self.model) != structure_of(model) {
            let _ = self.build(model);
            return;
        }
        for (submenu, menu) in self.bar.iter().zip(model) {
            submenu.set_text(&menu.label);
        }
        for (entry, item) in self.items.iter().zip(super::flatten(model)) {
            entry.update(item);
        }
        self.model = model.to_vec();
    }

    #[cfg(test)]
    fn detached(model: &[Menu<Action>]) -> Self {
        let mut native = Self {
            menu: muda::Menu::new(),
            bar: Vec::new(),
            items: Vec::new(),
            actions: HashMap::new(),
            model: Vec::new(),
            hwnd: None,
        };
        native.build(model).expect("a detached menu can be built");
        native
    }

    /// Returns the label each entry is currently showing, in model order.
    #[cfg(test)]
    fn labels(&self) -> Vec<String> {
        let bar = self.bar.iter().map(Submenu::text);
        let items = self.items.iter().map(|entry| match entry {
            Entry::Check(entry) => entry.text(),
            Entry::Command(entry) | Entry::Pending(entry) => entry.text(),
            Entry::Submenu(entry) => entry.text(),
            Entry::Separator => "-".to_owned(),
        });
        bar.chain(items).collect()
    }

    /// Returns whatever the operator activated since the last frame.
    pub fn poll(&self) -> Vec<Action> {
        let mut actions = Vec::new();
        let mut activated = false;
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            activated = true;
            if let Some(action) = self.actions.get(&event.id) {
                actions.push(action.clone());
            }
        }
        if activated {
            self.restore_checks();
        }
        actions
    }

    /// Rewrites the check marks the platform flipped on its own.
    ///
    /// A check entry toggles itself when it is activated, so choosing the
    /// device or the language that is already selected clears its mark
    /// even though the selection did not change. The next [`Self::sync`]
    /// cannot put it back: an unchanged selection produces the model that
    /// is already applied, which is exactly the case sync skips. The model
    /// is the authority, so the marks are written back from it here.
    pub(super) fn restore_checks(&self) {
        for (entry, item) in self.items.iter().zip(super::flatten(&self.model)) {
            if let (Entry::Check(entry), Item::Check { checked, .. }) = (entry, item) {
                entry.set_checked(*checked);
            }
        }
    }

    /// Flips every check mark, as the platform does when one is activated.
    #[cfg(test)]
    fn flip_checks(&self) {
        for entry in &self.items {
            if let Entry::Check(entry) = entry {
                entry.set_checked(!entry.is_checked());
            }
        }
    }

    /// Returns the state of each check entry, in model order.
    #[cfg(test)]
    fn checks(&self) -> Vec<bool> {
        self.items
            .iter()
            .filter_map(|entry| match entry {
                Entry::Check(entry) => Some(entry.is_checked()),
                _ => None,
            })
            .collect()
    }
}

fn structure_of<Action>(model: &[Menu<Action>]) -> Vec<Vec<Shape>> {
    model.iter().map(|menu| shapes(&menu.items)).collect()
}

#[derive(Eq, PartialEq)]
enum Shape {
    Submenu(Vec<Shape>),
    Check,
    Command,
    Pending,
    Separator,
}

fn shapes<Action>(items: &[Item<Action>]) -> Vec<Shape> {
    items
        .iter()
        .map(|item| match item {
            Item::Submenu { items, .. } => Shape::Submenu(shapes(items)),
            Item::Check { .. } => Shape::Check,
            Item::Command { .. } => Shape::Command,
            Item::Pending(_) => Shape::Pending,
            Item::Separator => Shape::Separator,
        })
        .collect()
}

#[cfg(test)]
mod tests;
