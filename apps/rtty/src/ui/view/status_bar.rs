//! The line along the bottom, which reports rather than offers anything.

use super::*;

use grayline_shell::i18n::number;

/// What is true of the session rather than of any one decode path.
///
/// Faults run from the left and audio facts from the right, as the other two
/// applications arrange the same line.
pub(super) fn status_bar(ui: &mut Ui, app: &App) {
    let snapshot = app.audio.snapshot();
    let audio = match app.audio.sample_rate_hz() {
        Some(rate) => app.i18n.text_with("status-audio", &[("rate", number(rate))]),
        None => app.i18n.text("status-no-audio"),
    };

    ui.horizontal(|ui| {
        if snapshot.dropped_samples > 0 {
            let dropped = app.i18n.text_with(
                "status-dropped",
                &[("samples", number(snapshot.dropped_samples as u32))],
            );
            ui.label(RichText::new(dropped).size(LABEL).color(ERROR_COLOR));
        }
        for error in [app.audio.error.as_ref(), snapshot.error.as_ref()]
            .into_iter()
            .flatten()
        {
            ui.label(RichText::new(error.to_string()).size(LABEL).color(ERROR_COLOR));
        }
        if let Some(notice) = app.notice.as_deref() {
            ui.label(RichText::new(notice.to_owned()).size(LABEL));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(audio).size(LABEL));
        });
    });
}
