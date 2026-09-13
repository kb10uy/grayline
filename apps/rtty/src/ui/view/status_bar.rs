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
        // The faults are drawn inside one scope of their own rather than
        // straight into the row. egui hands identifiers out by position, so a
        // fault turning up on the left would otherwise renumber the reading on
        // the right, which has not moved: it would lose what the window knew
        // about it, and a debug build draws a red frame around a widget it
        // catches changing identity. A scope takes one place in the row
        // whether it draws three labels or none.
        ui.scope(|ui| {
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
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(audio).size(LABEL));
            // After the reading rather than before it, so that the reading
            // keeps the right edge: a countdown put outside it would shove the
            // rate along by a digit's width every second, and would renumber
            // it every time a message started.
            if let Some(remaining) = app.transmission_remaining() {
                let left = app
                    .i18n
                    .text_with("status-remaining", &[("seconds", number(remaining.as_secs() as u32))]);
                ui.label(
                    RichText::new(left)
                        .size(LABEL)
                        .color(scrollback::sent_color(ui.visuals())),
                );
            }
        });
    });
}
