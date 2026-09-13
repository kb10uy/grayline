//! The scope window: the band the receiver is listening across, and the two
//! channels it is comparing.
//!
//! A window of its own rather than a panel, because it is watched while the
//! panel behind it is worked: a tuning display that the tuning controls cover
//! is one the tuning cannot be done against. It is a deferred viewport, so it
//! redraws on the frames the reception gives it while the main window sleeps
//! through the rest.
//!
//! What it draws is prepared by the application and read by egui from
//! whichever thread it runs the window's callback on, which is what the lock
//! here is for. Nothing in it decides anything: the pairs are the ones the
//! comparator already compared, and the band is a transform nothing in the
//! receiver reads.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};

use egui::{Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, RichText, Sense, Shape, Stroke, Ui, Vec2};
use grayline_rtty::{ToneSet, rx::ChannelLevels};
use grayline_shell::i18n::{I18n, Locale};

use crate::worker::receive::{FULL_SCALE, ScopeFrame};

const VIEWPORT: &str = "grayline-rtty-scope";

const WINDOW_SIZE: [f32; 2] = [420.0, 540.0];
const MINIMUM_WINDOW_SIZE: [f32; 2] = [240.0, 300.0];

/// Channel pairs held for the trace.
///
/// An eighth of a second at the rate the tap keeps them, which is about six
/// bits at 45.45 baud: long enough for the shape to be read, short enough
/// that what is on the screen is what is arriving now.
const TRACE_POINTS: usize = 512;

/// How far below a full-scale tone the spectrum's floor sits, in decibels.
///
/// Wide enough that a band with nothing on it still shows its noise, which is
/// how an operator tells a quiet receiver from a stopped one.
const FLOOR_DB: f32 = 80.0;

/// The strongest pair a trace is scaled against when it has nothing stronger.
///
/// The channels carry a rectified envelope of a normalized signal, so how far
/// they reach depends on how much of the band-pass the tones fill. The trace
/// is drawn against the strongest pair on it, and this is the floor under
/// that, so silence is not magnified into a picture.
const MINIMUM_TRACE_SCALE: f32 = 0.25;

/// How much of the height the band takes; the channels take the rest.
const SPECTRUM_SHARE: f32 = 0.45;
const MINIMUM_PANE: f32 = 60.0;

const TICK_HZ: f32 = 1_000.0;

/// The strip along the bottom of the band that the scale is written on.
///
/// Kept clear of the magnitudes rather than written over them: a figure drawn
/// on top of the noise floor is one nobody can read.
const SCALE_HEIGHT: f32 = 13.0;

const SCALE_LABEL_ROOM: f32 = 12.0;

/// Room left around the trace, so that it is read as a shape inside the box
/// rather than as a line along its edge.
const TRACE_INSET: f32 = 4.0;

const LABEL: f32 = 11.0;
const CORNER: f32 = 2.0;

const BACKGROUND: Color32 = Color32::from_rgb(0x0E, 0x12, 0x16);
const TRACE: Color32 = Color32::from_rgb(0x63, 0xD2, 0x97);
const MARKER: Color32 = Color32::from_rgb(0xE0, 0xA0, 0x30);
const AXIS: Color32 = Color32::from_rgb(0x44, 0x4E, 0x58);
const AXIS_TEXT: Color32 = Color32::from_rgb(0x8A, 0x96, 0xA2);

/// The two channels, named on the picture rather than in the message
/// catalogue: the header over the received text reads `M:2125 / S:2295` in
/// every language, and these are the same two letters.
const MARK_SYMBOL: &str = "M";
const SPACE_SYMBOL: &str = "S";

/// The scope window, as the application holds it.
#[derive(Default)]
pub struct Scope {
    open: bool,
    shared: Arc<Shared>,
    /// The language the labels were built in.
    labelled: Option<Locale>,
}

/// What the window's callback reaches, from wherever it is run.
#[derive(Default)]
struct Shared {
    view: Mutex<View>,
    /// Set when the operator closed the window from its own frame.
    close_requested: AtomicBool,
    /// Set when something arrived that is worth a frame of the window's own.
    moved: AtomicBool,
}

/// Everything drawn: prepared by the application, read by the window.
#[derive(Default)]
pub struct View {
    points: VecDeque<ChannelLevels>,
    spectrum: Vec<f32>,
    bin_hz: f32,
    /// The pair being detected, which is where the markers are drawn.
    tones: ToneSet,
    labels: Labels,
}

#[derive(Default)]
struct Labels {
    title: String,
    spectrum: String,
    channels: String,
    empty: String,
}

impl Scope {
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Opens or closes the window.
    ///
    /// Closing throws away what was on it: a scope shows what is arriving,
    /// and a trace kept from a minute ago would say the band is busy when it
    /// is not.
    pub fn set_open(&mut self, open: bool) {
        if self.open == open {
            return;
        }
        self.open = open;
        self.shared.close_requested.store(false, Ordering::Relaxed);
        if !open {
            let mut view = self.view();
            view.points.clear();
            view.spectrum.clear();
        }
    }

    /// Adds what the receive worker tapped since the last frame.
    pub fn push(&self, frame: ScopeFrame) {
        let mut view = self.view();
        view.points.extend(frame.points);
        let excess = view.points.len().saturating_sub(TRACE_POINTS);
        view.points.drain(..excess);
        // A frame from a transform that has not filled yet carries no band,
        // and the last one drawn is a better answer than an empty box.
        if !frame.spectrum.is_empty() {
            view.spectrum = frame.spectrum;
            view.bin_hz = frame.bin_hz;
        }
        drop(view);
        self.shared.moved.store(true, Ordering::Relaxed);
    }

    /// Moves the markers to the pair the receiver is detecting.
    pub fn retune(&self, tones: ToneSet) {
        let mut view = self.view();
        if view.tones != tones {
            view.tones = tones;
            drop(view);
            self.shared.moved.store(true, Ordering::Relaxed);
        }
    }

    /// Names what the window draws, in the operator's language.
    pub fn describe(&mut self, i18n: &I18n) {
        if self.labelled == Some(i18n.locale()) {
            return;
        }
        self.labelled = Some(i18n.locale());
        let labels = Labels {
            title: format!("{} — {}", crate::identity::DISPLAY_NAME, i18n.text("window-scope")),
            spectrum: i18n.text("label-spectrum"),
            channels: i18n.text("label-channels"),
            empty: i18n.text("hint-scope"),
        };
        self.view().labels = labels;
        self.shared.moved.store(true, Ordering::Relaxed);
    }

    /// Takes the operator's request to close the window, if they made one.
    pub fn take_close_request(&self) -> bool {
        self.shared.close_requested.swap(false, Ordering::Relaxed)
    }

    /// How many pairs the trace is holding.
    #[cfg(test)]
    pub fn drawn_points(&self) -> usize {
        self.view().points.len()
    }

    /// Asks to close the window, as its own frame does.
    #[cfg(test)]
    pub fn request_close(&self) {
        self.shared.close_requested.store(true, Ordering::Relaxed);
    }

    fn view(&self) -> MutexGuard<'_, View> {
        self.shared.view.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl core::fmt::Debug for Scope {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("Scope").field("open", &self.open).finish()
    }
}

/// Shows the window for as long as the application says it is open.
///
/// The callback is handed over again on every frame the main window draws,
/// which is how the title follows the language.
pub fn window(ctx: &egui::Context, scope: &Scope) {
    if !scope.is_open() {
        return;
    }
    let id = egui::ViewportId::from_hash_of(VIEWPORT);
    let shared = Arc::clone(&scope.shared);
    let builder = egui::ViewportBuilder::default()
        .with_title(scope.view().labels.title.clone())
        .with_inner_size(WINDOW_SIZE)
        .with_min_inner_size(MINIMUM_WINDOW_SIZE)
        .with_always_on_top();

    ctx.show_viewport_deferred(id, builder, move |ui, _class| {
        if ui.ctx().input(|input| input.viewport().close_requested()) {
            shared.close_requested.store(true, Ordering::Relaxed);
        }
        let view = shared.view.lock().unwrap_or_else(PoisonError::into_inner);
        egui::CentralPanel::default().show(ui, |ui| contents(ui, &view));
    });

    // The window is asked for a frame rather than left polling for one: it
    // sleeps like any other viewport, and everything it draws arrives here.
    if scope.shared.moved.swap(false, Ordering::Relaxed) {
        ctx.request_repaint_of(id);
    }
}

fn contents(ui: &mut Ui, view: &View) {
    ui.style_mut().interaction.selectable_labels = false;
    let spacing = ui.spacing().item_spacing.y;

    ui.label(RichText::new(&view.labels.spectrum).size(LABEL).weak());
    let height = ((ui.available_height() - spacing * 3.0) * SPECTRUM_SHARE).max(MINIMUM_PANE);
    spectrum(ui, view, height);

    ui.add_space(spacing);
    ui.label(RichText::new(&view.labels.channels).size(LABEL).weak());
    channels(ui, view);
}

fn spectrum(ui: &mut Ui, view: &View, height: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::from(CORNER), BACKGROUND);

    let span_hz = view.bin_hz * view.spectrum.len() as f32;
    if view.spectrum.len() < 2 || span_hz <= 0.0 {
        say_nothing(&painter, rect, &view.labels.empty);
        return;
    }

    let scale = Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - SCALE_HEIGHT), rect.max);
    let band = Rect::from_min_max(rect.min, egui::pos2(rect.right(), scale.top()));

    let columns = band.width().round().max(1.0) as usize;
    let mut bars = Vec::with_capacity(columns);
    for column in 0..columns {
        let from = bin_at(column, columns, view.spectrum.len());
        let to = bin_at(column + 1, columns, view.spectrum.len()).max(from + 1);
        // A column narrower than a bin repeats it, and a wider one takes the
        // strongest bin under it: a carrier is what a band is read for, and
        // averaging it away would hide it as the window narrowed.
        let peak = view.spectrum[from..to].iter().copied().fold(0.0_f32, f32::max);
        let level = level(peak);
        if level <= 0.0 {
            continue;
        }
        let x = band.left() + column as f32 + 0.5;
        bars.push(Shape::line_segment(
            [
                egui::pos2(x, band.bottom()),
                egui::pos2(x, band.bottom() - band.height() * level),
            ],
            Stroke::new(1.0, TRACE),
        ));
    }
    painter.extend(bars);

    ticks(&painter, band, scale, span_hz);
    for (frequency_hz, symbol) in [
        (view.tones.mark_hz as f32, MARK_SYMBOL),
        (view.tones.space_hz as f32, SPACE_SYMBOL),
    ] {
        marker(&painter, band, span_hz, frequency_hz, symbol);
    }
}

fn bin_at(column: usize, columns: usize, bins: usize) -> usize {
    (column * bins / columns).min(bins - 1)
}

/// Where a magnitude sits between the floor and a full-scale tone.
fn level(magnitude: f32) -> f32 {
    if magnitude <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * (magnitude / FULL_SCALE).log10();
    ((db + FLOOR_DB) / FLOOR_DB).clamp(0.0, 1.0)
}

fn x_of(rect: Rect, span_hz: f32, frequency_hz: f32) -> f32 {
    rect.left() + rect.width() * (frequency_hz / span_hz).clamp(0.0, 1.0)
}

fn ticks(painter: &Painter, band: Rect, scale: Rect, span_hz: f32) {
    let mut frequency_hz = TICK_HZ;
    while frequency_hz < span_hz {
        let x = x_of(band, span_hz, frequency_hz);
        painter.line_segment(
            [egui::pos2(x, band.top()), egui::pos2(x, band.bottom())],
            Stroke::new(1.0, AXIS),
        );
        if x - SCALE_LABEL_ROOM > scale.left() && x + SCALE_LABEL_ROOM < scale.right() {
            painter.text(
                egui::pos2(x, scale.center().y),
                Align2::CENTER_CENTER,
                format!("{:.0}k", frequency_hz / TICK_HZ),
                FontId::proportional(LABEL),
                AXIS_TEXT,
            );
        }
        frequency_hz += TICK_HZ;
    }
}

/// One of the two tones, where the receiver is listening for it now.
fn marker(painter: &Painter, rect: Rect, span_hz: f32, frequency_hz: f32, symbol: &str) {
    if frequency_hz <= 0.0 || frequency_hz >= span_hz {
        return;
    }
    let x = x_of(rect, span_hz, frequency_hz);
    painter.line_segment(
        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
        Stroke::new(1.0, MARKER),
    );
    painter.text(
        egui::pos2(x + 2.0, rect.top()),
        Align2::LEFT_TOP,
        symbol,
        FontId::proportional(LABEL),
        MARKER,
    );
}

/// The two channels against each other, which is what tuning is read from.
///
/// A pair sitting on the tones swings between the two axes; one off them
/// collapses towards the line where the two channels are equal, which is the
/// line the comparator decides on.
fn channels(ui: &mut Ui, view: &View) {
    let side = ui.available_height().min(ui.available_width()).max(MINIMUM_PANE);
    let (outer, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), side), Sense::hover());
    let rect = Rect::from_center_size(outer.center(), Vec2::splat(side));
    let painter = ui.painter_at(outer);
    painter.rect_filled(rect, CornerRadius::from(CORNER), BACKGROUND);
    let plot = rect.shrink(TRACE_INSET);
    painter.line_segment([plot.left_bottom(), plot.right_top()], Stroke::new(1.0, AXIS));
    painter.text(
        egui::pos2(rect.right() - 2.0, rect.bottom() - 2.0),
        Align2::RIGHT_BOTTOM,
        MARK_SYMBOL,
        FontId::proportional(LABEL),
        AXIS_TEXT,
    );
    painter.text(
        egui::pos2(rect.left() + 2.0, rect.top() + 2.0),
        Align2::LEFT_TOP,
        SPACE_SYMBOL,
        FontId::proportional(LABEL),
        AXIS_TEXT,
    );

    if view.points.len() < 2 {
        say_nothing(&painter, rect, &view.labels.empty);
        return;
    }
    let scale = trace_scale(&view.points);
    let trace = view.points.iter().map(|pair| point(plot, *pair, scale)).collect();
    painter.add(Shape::line(trace, Stroke::new(1.0, TRACE)));
}

/// The strongest channel reading on the trace, which it is drawn against.
fn trace_scale(points: &VecDeque<ChannelLevels>) -> f32 {
    points.iter().fold(MINIMUM_TRACE_SCALE, |peak, pair| {
        peak.max(pair.mark as f32).max(pair.space as f32)
    })
}

/// Puts one pair in the square: mark to the right, space upwards.
fn point(rect: Rect, levels: ChannelLevels, scale: f32) -> Pos2 {
    let mark = (levels.mark as f32 / scale).clamp(0.0, 1.0);
    let space = (levels.space as f32 / scale).clamp(0.0, 1.0);
    egui::pos2(rect.left() + rect.width() * mark, rect.bottom() - rect.height() * space)
}

fn say_nothing(painter: &Painter, rect: Rect, text: &str) {
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(LABEL),
        AXIS_TEXT,
    );
}

#[cfg(test)]
mod tests;
