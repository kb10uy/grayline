use std::marker::PhantomData;

use super::Menu;

/// A placeholder for platforms whose menu is drawn by [`super::bar`].
pub struct MenuHost<Action>(PhantomData<fn() -> Action>);

impl<Action> MenuHost<Action> {
    /// Leaves installation to the in-window renderer.
    pub fn install(
        _cc: &eframe::CreationContext<'_>,
        _model: &[Menu<Action>],
    ) -> Result<Self, std::convert::Infallible> {
        Ok(Self(PhantomData))
    }

    /// Leaves synchronization to the in-window renderer.
    pub fn sync(&mut self, _model: &[Menu<Action>]) {}

    /// Returns no native actions; the in-window renderer returns them directly.
    pub fn poll(&self) -> Vec<Action> {
        Vec::new()
    }

    /// Leaves window closing to eframe.
    pub fn prepare_for_close(&self) {}
}
