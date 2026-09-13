use std::sync::Arc;

use egui::{Color32, ColorImage, Context, Rect, Sense, TextureHandle, TextureOptions, Ui, Vec2, pos2};

use crate::worker::receive::StripUpdate;

/// How the strip is sampled.
///
/// A chart is nearly always shown smaller than it was decoded — 1810 pixels of
/// line against a window a few hundred points tall — and picking one pixel in
/// three of line art drops half of it and leaves the rest looking torn.
/// Averaging instead keeps a coastline continuous. Magnifying still takes the
/// nearest pixel, so an operator who has come in close is looking at what was
/// decoded rather than at an interpolation of it.
const SAMPLING: TextureOptions = TextureOptions {
    magnification: egui::TextureFilter::Nearest,
    minification: egui::TextureFilter::Linear,
    ..TextureOptions::NEAREST
};

/// Size of the text drawn where a picture has not arrived, in points.
const HINT_TEXT_SIZE: f32 = 16.0;
/// Columns the strip starts out able to hold.
const INITIAL_CAPACITY: usize = 1_024;
/// Columns the strip will grow to, and no further.
///
/// The decoder's own default limit is half an hour, which is 7200 lines at the
/// fastest rate it decodes, so in practice the strip never wraps. It still
/// does the arithmetic, because a limit raised by hand should cost the oldest
/// columns rather than the newest ones.
const MAX_CAPACITY: usize = 8_192;

/// The received chart, drawn the way a fax arrives: a line at a time, left to
/// right.
///
/// WEFAX sends lines, and a line is 1810 pixels against a window that is a few
/// hundred points tall, so the picture is turned a quarter turn: one decoded
/// line is one column, and time runs across the window rather than down it.
///
/// The pixels are kept here as well as on the card. A correction moves lines
/// already drawn, and the operator can save the chart at any point, so the
/// interface needs the picture in a form it can read back rather than only one
/// it can display.
#[derive(Default)]
pub struct Strip {
    /// Pixels in one line, which is the strip's height.
    width: usize,
    /// Columns the texture holds.
    capacity: usize,
    /// Lines decoded so far, whether or not they still fit.
    lines: usize,
    /// The picture, line-major, as the decoder produced it.
    gray: Vec<u8>,
    texture: Option<TextureHandle>,
}

impl core::fmt::Debug for Strip {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Strip")
            .field("width", &self.width)
            .field("lines", &self.lines)
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl Strip {
    pub const fn width(&self) -> usize {
        self.width
    }

    pub const fn lines(&self) -> usize {
        self.lines
    }

    pub const fn is_empty(&self) -> bool {
        self.lines == 0
    }

    /// Returns the picture, line-major, for saving it.
    pub fn gray(&self) -> &[u8] {
        &self.gray
    }

    /// Forgets everything, so the next reception starts on an empty strip.
    pub fn clear(&mut self) {
        self.width = 0;
        self.capacity = 0;
        self.lines = 0;
        self.gray.clear();
        self.texture = None;
    }

    /// Takes the columns a worker published.
    ///
    /// Returns whether they were adopted. A run that does not continue from
    /// where the last one ended says nothing usable, and is dropped rather
    /// than drawn in the wrong place.
    pub fn apply(&mut self, ctx: &Context, update: &StripUpdate) -> bool {
        if update.width == 0 {
            return false;
        }
        if update.width != self.width {
            self.clear();
            self.width = update.width;
        }
        if update.replaces_all || update.first_line == 0 {
            self.gray.clear();
            self.lines = 0;
        } else if update.first_line != self.lines {
            return false;
        }
        self.gray.extend_from_slice(&update.gray);
        let first = self.lines;
        self.lines += update.lines();

        if self.grow(ctx) || update.replaces_all || update.first_line == 0 {
            self.redraw(ctx);
        } else {
            self.upload(ctx, first, self.lines);
        }
        true
    }

    /// Makes sure the texture can hold what has been decoded.
    ///
    /// Returns whether it was rebuilt.
    fn grow(&mut self, ctx: &Context) -> bool {
        let wanted = self.lines.max(INITIAL_CAPACITY).next_power_of_two().min(MAX_CAPACITY);
        if self.texture.is_some() && wanted <= self.capacity {
            return false;
        }
        self.capacity = wanted;
        let blank = ColorImage::new(
            [self.capacity, self.width],
            vec![Color32::BLACK; self.capacity * self.width],
        );
        self.texture = Some(ctx.load_texture("wefax-strip", Arc::new(blank), SAMPLING));
        true
    }

    fn redraw(&mut self, ctx: &Context) {
        let first = self.lines.saturating_sub(self.capacity);
        self.upload(ctx, first, self.lines);
    }

    /// Writes columns `first..end` into the texture.
    ///
    /// The columns wrap, so a run that crosses the end of the texture is
    /// written as two.
    fn upload(&mut self, ctx: &Context, first: usize, end: usize) {
        let _ = ctx;
        let Some(texture) = self.texture.as_mut() else {
            return;
        };
        let first = first.max(end.saturating_sub(self.capacity));
        let mut line = first;
        while line < end {
            let offset = line % self.capacity;
            let run = (end - line).min(self.capacity - offset);
            let image = rotate(&self.gray, self.width, line, run);
            texture.set_partial([offset, 0], image, SAMPLING);
            line += run;
        }
    }

    /// Draws the strip, newest at the right.
    ///
    /// The whole of a line is always visible: the vertical scale follows the
    /// space available, and the horizontal one follows it, so a chart is shown
    /// at its own proportions until it is longer than the window can hold and
    /// then keeps its newest end against the right edge.
    pub fn paint(&self, ui: &mut Ui, hint: &str) {
        let available = ui.available_size();
        let (rect, _) = ui.allocate_exact_size(available, Sense::hover());
        ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
        let (Some(texture), true) = (self.texture.as_ref(), self.width > 0 && self.lines > 0) else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                hint,
                egui::FontId::proportional(HINT_TEXT_SIZE),
                Color32::from_gray(0x60),
            );
            return;
        };

        let scale = (rect.height() / self.width as f32).max(f32::MIN_POSITIVE);
        let visible = ((rect.width() / scale) as usize).max(1);
        let held = self.lines.min(self.capacity);
        let shown = held.min(visible);
        let first = self.lines - shown;

        let mut x = rect.left();
        let mut line = first;
        while line < self.lines {
            let offset = line % self.capacity;
            let run = (self.lines - line).min(self.capacity - offset);
            let left = offset as f32 / self.capacity as f32;
            let right = (offset + run) as f32 / self.capacity as f32;
            let size = Vec2::new(run as f32 * scale, rect.height());
            let target = Rect::from_min_size(pos2(x, rect.top()), size);
            egui::Image::new(texture)
                .uv(Rect::from_min_max(pos2(left, 0.0), pos2(right, 1.0)))
                .paint_at(ui, target);
            x += size.x;
            line += run;
        }
    }
}

/// Turns `run` lines of a line-major picture a quarter turn anticlockwise.
///
/// Transposing alone would be quicker and wrong: it reflects the picture about
/// its diagonal, which leaves the chart mirrored and its lettering unreadable.
/// Turning the printed page anticlockwise is what puts time along the top from
/// left to right, and that puts the start of each line at the bottom.
fn rotate(gray: &[u8], width: usize, first: usize, run: usize) -> ColorImage {
    let mut pixels = Vec::with_capacity(run * width);
    for pixel in (0..width).rev() {
        for line in first..first + run {
            let level = gray.get(line * width + pixel).copied().unwrap_or_default();
            pixels.push(Color32::from_gray(level));
        }
    }
    ColorImage::new([run, width], pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(width: usize, first_line: usize, lines: usize, replaces_all: bool) -> StripUpdate {
        StripUpdate {
            width,
            first_line,
            gray: (0..lines * width).map(|index| (first_line + index) as u8).collect(),
            replaces_all,
        }
    }

    #[test]
    fn columns_accumulate_as_lines_arrive() {
        let ctx = Context::default();
        let mut strip = Strip::default();
        assert!(strip.is_empty());

        assert!(strip.apply(&ctx, &update(8, 0, 3, false)));
        assert_eq!(strip.lines(), 3);
        assert_eq!(strip.width(), 8);

        assert!(strip.apply(&ctx, &update(8, 3, 2, false)));
        assert_eq!(strip.lines(), 5);
        assert_eq!(strip.gray().len(), 5 * 8);
    }

    #[test]
    fn a_redraw_replaces_everything() {
        let ctx = Context::default();
        let mut strip = Strip::default();
        strip.apply(&ctx, &update(8, 0, 5, false));
        strip.apply(&ctx, &update(8, 0, 2, true));
        assert_eq!(strip.lines(), 2);
        assert_eq!(strip.gray().len(), 2 * 8);
    }

    #[test]
    fn a_gap_in_the_columns_is_refused() {
        let ctx = Context::default();
        let mut strip = Strip::default();
        strip.apply(&ctx, &update(8, 0, 3, false));
        assert!(!strip.apply(&ctx, &update(8, 9, 2, false)));
        assert_eq!(strip.lines(), 3);
    }

    #[test]
    fn a_new_line_length_starts_the_strip_over() {
        let ctx = Context::default();
        let mut strip = Strip::default();
        strip.apply(&ctx, &update(8, 0, 4, false));
        strip.apply(&ctx, &update(16, 0, 1, false));
        assert_eq!(strip.width(), 16);
        assert_eq!(strip.lines(), 1);
    }

    #[test]
    fn an_empty_update_says_nothing() {
        let ctx = Context::default();
        let mut strip = Strip::default();
        assert!(!strip.apply(&ctx, &update(0, 0, 0, false)));
        assert!(strip.is_empty());
    }

    #[test]
    fn clearing_forgets_the_picture() {
        let ctx = Context::default();
        let mut strip = Strip::default();
        strip.apply(&ctx, &update(8, 0, 4, false));
        strip.clear();
        assert!(strip.is_empty());
        assert_eq!(strip.width(), 0);
        assert!(strip.gray().is_empty());
    }

    /// A quarter turn anticlockwise, not a transpose: the two differ by a
    /// mirroring, and a mirrored weather chart cannot be read.
    #[test]
    fn a_line_becomes_a_column_with_its_start_at_the_bottom() {
        let gray = [0_u8, 1, 2, 3, 4, 5];
        let image = rotate(&gray, 3, 0, 2);
        assert_eq!(image.size, [2, 3]);
        assert_eq!(image.pixels[0], Color32::from_gray(2));
        assert_eq!(image.pixels[1], Color32::from_gray(5));
        assert_eq!(image.pixels[4], Color32::from_gray(0));
        assert_eq!(image.pixels[5], Color32::from_gray(3));
    }

    #[test]
    fn the_texture_grows_by_doubling_up_to_its_limit() {
        let ctx = Context::default();
        let mut strip = Strip::default();
        strip.apply(&ctx, &update(4, 0, 1, false));
        assert_eq!(strip.capacity, INITIAL_CAPACITY);

        strip.apply(&ctx, &update(4, 1, INITIAL_CAPACITY, false));
        assert_eq!(strip.capacity, INITIAL_CAPACITY * 2);
    }
}
