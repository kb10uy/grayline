use crate::format::{Format, LinesPerMinute, WefaxBand};

/// Longest reception the default line limit allows, in seconds.
pub const MAX_RECEPTION_SECONDS: f64 = 1_800.0;

/// What to do when the phasing signal never produces a confident pulse.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PhasingFallback {
    /// Start drawing anyway, wherever the fold happened to begin.
    ///
    /// A picture rolled sideways is still readable, and the operator can
    /// shift it; nothing decoded is nothing.
    #[default]
    StartImaging,
    /// Give up on the reception.
    Stop,
}

/// How a receive decoder should behave.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RxConfig {
    /// The shift the gray scale is read against.
    pub band: WefaxBand,
    /// The geometry to use, when the operator knows it.
    pub format: Option<Format>,
    /// Whether a start tone may begin a reception.
    pub auto_start: bool,
    /// Whether a stop tone may end one.
    pub auto_stop: bool,
    /// Whether to choose the line rate from the phasing signal.
    ///
    /// A start tone announces the index of cooperation and nothing else, so
    /// without this the rate has to come from [`format`](Self::format).
    pub infer_lines_per_minute: bool,
    /// Whether to refit the line clock from the picture as it arrives.
    pub slant_tracking: bool,
    /// Whether black and white are the other way round.
    pub inverted: bool,
    /// Where in the line the phasing pulse should be placed, as a fraction.
    pub phase_offset_fraction: f64,
    /// How long to fold before a pulse may be accepted, in seconds.
    pub phasing_min_seconds: f64,
    /// How long to fold before giving up on one, in seconds.
    pub phasing_max_seconds: f64,
    /// What to do when that happens.
    pub phasing_fallback: PhasingFallback,
    /// The line limit, or `None` to derive it from the line rate.
    pub max_lines: Option<usize>,
}

impl Default for RxConfig {
    fn default() -> Self {
        Self {
            band: WefaxBand::WIDE,
            format: None,
            auto_start: true,
            auto_stop: true,
            infer_lines_per_minute: true,
            slant_tracking: true,
            inverted: false,
            phase_offset_fraction: 0.0,
            phasing_min_seconds: 5.0,
            phasing_max_seconds: 45.0,
            phasing_fallback: PhasingFallback::StartImaging,
            max_lines: None,
        }
    }
}

impl RxConfig {
    /// Returns the line rates the phasing fold should choose between.
    pub(crate) fn candidates(&self) -> &'static [LinesPerMinute] {
        match (self.infer_lines_per_minute, self.format) {
            (false, Some(format)) => match format.lines_per_minute {
                LinesPerMinute::L60 => &[LinesPerMinute::L60],
                LinesPerMinute::L90 => &[LinesPerMinute::L90],
                LinesPerMinute::L100 => &[LinesPerMinute::L100],
                LinesPerMinute::L120 => &[LinesPerMinute::L120],
                LinesPerMinute::L180 => &[LinesPerMinute::L180],
                LinesPerMinute::L240 => &[LinesPerMinute::L240],
            },
            _ => &LinesPerMinute::ALL,
        }
    }

    /// Returns the line limit for a geometry.
    pub(crate) fn line_limit(&self, format: Format) -> usize {
        self.max_lines
            .unwrap_or_else(|| format.lines_per_minute.lines_in(MAX_RECEPTION_SECONDS))
            .max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::Ioc;

    #[test]
    fn inference_offers_every_rate() {
        let config = RxConfig::default();
        assert_eq!(config.candidates(), &LinesPerMinute::ALL);
    }

    #[test]
    fn a_configured_rate_is_the_only_candidate_once_inference_is_off() {
        let config = RxConfig {
            infer_lines_per_minute: false,
            format: Some(Format::MARINE),
            ..RxConfig::default()
        };
        assert_eq!(config.candidates(), &[LinesPerMinute::L120]);
    }

    #[test]
    fn a_configured_rate_is_still_only_a_hint_while_inference_is_on() {
        let config = RxConfig {
            format: Some(Format::MARINE),
            ..RxConfig::default()
        };
        assert_eq!(config.candidates(), &LinesPerMinute::ALL);
    }

    #[test]
    fn the_default_line_limit_is_half_an_hour_of_the_chosen_rate() {
        let config = RxConfig::default();
        assert_eq!(config.line_limit(Format::MARINE), 3_600);
        assert_eq!(
            config.line_limit(Format {
                ioc: Ioc::Ioc576,
                lines_per_minute: LinesPerMinute::L60
            }),
            1_800
        );
    }

    #[test]
    fn a_stated_line_limit_is_used_as_it_stands() {
        let config = RxConfig {
            max_lines: Some(120),
            ..RxConfig::default()
        };
        assert_eq!(config.line_limit(Format::MARINE), 120);
    }
}
