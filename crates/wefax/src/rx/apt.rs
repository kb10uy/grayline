use grayline_dsp::detector::{ToneDetector, ToneDetectorDesign};

use crate::{
    error::WefaxError,
    format::{APT_STOP_HZ, Ioc},
};

/// Detection bandwidth of each tone detector, in hertz.
const BANDWIDTH_HZ: f64 = 25.0;
/// Envelope cutoff of each tone detector, in hertz.
///
/// A tone lasts five seconds, so nothing is gained by following it quickly,
/// and a wider envelope would let picture content ripple through.
const ENVELOPE_CUTOFF_HZ: f64 = 4.0;
/// Fraction of a full-deviation square wave a tone has to reach.
const MIN_STRENGTH: f64 = 0.55;
/// Share of the total strength the winning tone has to hold.
const MIN_DOMINANCE: f64 = 0.70;
/// How long a tone has to qualify continuously before it is accepted.
const HOLD_SECONDS: f64 = 2.0;
/// How long the calibration probe runs, in seconds.
const CALIBRATION_SECONDS: f64 = 1.0;
/// Fraction of the calibration probe averaged into the normalizer.
const CALIBRATION_TAIL: f64 = 0.05;

/// Which automatic picture transmission tone was heard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AptEvent {
    /// A start tone held long enough to be trusted, naming its index.
    StartAccepted {
        /// The index of cooperation the tone's rate announces.
        ioc: Ioc,
        /// Absolute position the tone was accepted at.
        sample: u64,
    },
    /// The accepted start tone stopped; the phasing signal follows.
    StartEnded {
        /// Absolute position of the tone's last sample.
        sample: u64,
    },
    /// A stop tone held long enough to be trusted.
    StopAccepted {
        /// Absolute position the tone was accepted at.
        sample: u64,
    },
}

impl AptEvent {
    /// Returns the absolute position the decision was made at.
    pub const fn sample(self) -> u64 {
        match self {
            Self::StartAccepted { sample, .. } | Self::StartEnded { sample } | Self::StopAccepted { sample } => sample,
        }
    }
}

/// How strongly each automatic picture transmission tone is present.
///
/// Each value is a fraction of what a full-deviation square wave at that rate
/// would produce, so the three are comparable with one another and with the
/// same reading taken at another capture rate.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AptStrengths {
    /// The 300 Hz start tone, which announces IOC 576.
    pub start_576: f32,
    /// The 675 Hz start tone, which announces IOC 288.
    pub start_288: f32,
    /// The 450 Hz stop tone.
    pub stop: f32,
}

/// What a channel means when it wins.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Meaning {
    Start(Ioc),
    Stop,
}

const MEANINGS: [Meaning; 3] = [Meaning::Start(Ioc::Ioc576), Meaning::Start(Ioc::Ioc288), Meaning::Stop];

#[derive(Clone, Debug)]
struct Channel {
    detector: ToneDetector,
    normalizer: f64,
    strength: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    /// No tone accepted; `candidate` is what has been qualifying, and since when.
    Hunting { candidate: Option<(usize, u64)> },
    /// A tone was accepted and is still being heard.
    Holding { channel: usize },
}

/// Detects the tones that frame a transmission.
///
/// The start and stop tones are the picture itself keyed between black and
/// white at 300, 450, or 675 Hz. On the audio that is a keyed carrier, but on
/// the demodulated stream it is a square wave at that rate, so this detector
/// consumes the normalized deviation rather than the waveform. That also
/// makes it independent of the receiver's gain.
///
/// The detectors' responses differ by more than a factor of two across the
/// three rates, because a resonator's input gain follows the sine of its
/// angular frequency. Each is therefore normalized at construction against a
/// probe of its own tone, which is what lets one pair of thresholds hold at
/// every capture rate.
#[derive(Clone, Debug)]
pub(crate) struct AptDetector {
    channels: [Channel; 3],
    hold_samples: u64,
    state: State,
}

impl AptDetector {
    pub(crate) fn new(sample_rate_hz: f64) -> Result<Self, WefaxError> {
        let frequencies = [Ioc::Ioc576.apt_start_hz(), Ioc::Ioc288.apt_start_hz(), APT_STOP_HZ];
        let mut channels = [const { None }; 3];
        for (channel, frequency_hz) in channels.iter_mut().zip(frequencies) {
            let detector = ToneDetector::new(ToneDetectorDesign {
                sample_rate_hz,
                frequency_hz,
                bandwidth_hz: BANDWIDTH_HZ,
                envelope_cutoff_hz: ENVELOPE_CUTOFF_HZ,
            })?;
            let normalizer = calibrate(&detector, frequency_hz, sample_rate_hz);
            *channel = Some(Channel {
                detector,
                normalizer,
                strength: 0.0,
            });
        }
        Ok(Self {
            channels: channels.map(|channel| channel.expect("every channel was built")),
            hold_samples: (sample_rate_hz * HOLD_SECONDS) as u64,
            state: State::Hunting { candidate: None },
        })
    }

    /// Consumes one normalized deviation sample and reports what it decided.
    pub(crate) fn process(&mut self, normalized: f64, sample: u64) -> Option<AptEvent> {
        let mut best = 0;
        let mut competing = 0.0_f64;
        for channel in &mut self.channels {
            channel.strength = channel.detector.process_sample(normalized) / channel.normalizer;
        }
        for index in 1..self.channels.len() {
            if self.channels[index].strength > self.channels[best].strength {
                best = index;
            }
        }
        for (index, channel) in self.channels.iter().enumerate() {
            if index != best {
                competing = competing.max(channel.strength);
            }
        }
        let strength = self.channels[best].strength;
        let dominance = strength / (strength + competing).max(f64::MIN_POSITIVE);
        let qualifies = strength >= MIN_STRENGTH && dominance >= MIN_DOMINANCE;

        match self.state {
            State::Hunting { candidate } => {
                if !qualifies {
                    self.state = State::Hunting { candidate: None };
                    return None;
                }
                match candidate {
                    Some((channel, since)) if channel == best => {
                        if sample.saturating_sub(since) + 1 >= self.hold_samples {
                            self.state = State::Holding { channel: best };
                            return Some(match MEANINGS[best] {
                                Meaning::Start(ioc) => AptEvent::StartAccepted { ioc, sample },
                                Meaning::Stop => AptEvent::StopAccepted { sample },
                            });
                        }
                    }
                    _ => {
                        self.state = State::Hunting {
                            candidate: Some((best, sample)),
                        };
                    }
                }
                None
            }
            State::Holding { channel } => {
                if qualifies && channel == best {
                    return None;
                }
                self.state = State::Hunting { candidate: None };
                match MEANINGS[channel] {
                    // This sample is the first that is no longer the tone, so
                    // the one before it is the last that was. Every event here
                    // carries the last sample its tone owns, which is what
                    // lets a caller split a packet at one.
                    Meaning::Start(_) => Some(AptEvent::StartEnded {
                        sample: sample.saturating_sub(1),
                    }),
                    // Nothing follows a stop tone, so its end says nothing.
                    Meaning::Stop => None,
                }
            }
        }
    }

    pub(crate) fn strengths(&self) -> AptStrengths {
        AptStrengths {
            start_576: self.channels[0].strength as f32,
            start_288: self.channels[1].strength as f32,
            stop: self.channels[2].strength as f32,
        }
    }

    pub(crate) fn reset(&mut self) {
        for channel in &mut self.channels {
            channel.detector.reset();
            channel.strength = 0.0;
        }
        self.state = State::Hunting { candidate: None };
    }
}

/// Measures what a full-deviation square wave at `frequency_hz` produces.
///
/// Deterministic, and a few tens of thousands of iterations once per
/// reception. A table of thresholds per rate would be the alternative, and it
/// would be wrong the first time a rate outside the table was used.
fn calibrate(detector: &ToneDetector, frequency_hz: f64, sample_rate_hz: f64) -> f64 {
    let mut probe = detector.clone();
    let count = (sample_rate_hz * CALIBRATION_SECONDS) as usize;
    let tail = (count as f64 * CALIBRATION_TAIL) as usize;
    let averaged_from = count.saturating_sub(tail.max(1));
    let mut phase = 0.0_f64;
    let mut sum = 0.0_f64;
    let mut averaged = 0.0_f64;
    for index in 0..count {
        let envelope = probe.process_sample(if phase < 0.5 { 1.0 } else { -1.0 });
        phase += frequency_hz / sample_rate_hz;
        if phase >= 1.0 {
            phase -= 1.0;
        }
        if index >= averaged_from {
            sum += envelope;
            averaged += 1.0;
        }
    }
    (sum / averaged.max(1.0)).max(f64::MIN_POSITIVE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use rstest::rstest;

    /// A full-deviation keyed picture at `alternation_hz`, as the demodulator
    /// would report it.
    fn keyed(rate: u32, alternation_hz: f64, seconds: f64) -> Vec<f64> {
        let count = (f64::from(rate) * seconds) as usize;
        let mut phase = 0.0_f64;
        let mut samples = Vec::with_capacity(count);
        for _ in 0..count {
            samples.push(if phase < 0.5 { 1.0 } else { -1.0 });
            phase += alternation_hz / f64::from(rate);
            if phase >= 1.0 {
                phase -= 1.0;
            }
        }
        samples
    }

    /// Reproducible picture content, as a normalized deviation stream.
    fn picture(rate: u32, seconds: f64) -> Vec<f64> {
        let count = (f64::from(rate) * seconds) as usize;
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        (0..count)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                f64::from((state >> 40) as u32) / f64::from(1_u32 << 24) * 2.0 - 1.0
            })
            .collect()
    }

    fn run(detector: &mut AptDetector, samples: &[f64], from: u64) -> Vec<AptEvent> {
        samples
            .iter()
            .enumerate()
            .filter_map(|(offset, value)| detector.process(*value, from + offset as u64))
            .collect()
    }

    #[rstest]
    #[case(300.0, Some(Ioc::Ioc576))]
    #[case(675.0, Some(Ioc::Ioc288))]
    #[case(450.0, None)]
    fn each_tone_is_accepted_and_named(#[case] alternation_hz: f64, #[case] expected: Option<Ioc>) {
        let rate = 11_025;
        let mut detector = AptDetector::new(f64::from(rate)).unwrap();
        let events = run(&mut detector, &keyed(rate, alternation_hz, 5.0), 0);
        let accepted = events.first().copied();
        match (accepted, expected) {
            (Some(AptEvent::StartAccepted { ioc, .. }), Some(want)) => assert_eq!(ioc, want),
            (Some(AptEvent::StopAccepted { .. }), None) => {}
            other => panic!("{alternation_hz} Hz produced {other:?}"),
        }
    }

    #[test]
    fn a_start_tone_reports_where_it_ended() {
        let rate = 11_025;
        let mut detector = AptDetector::new(f64::from(rate)).unwrap();
        let mut samples = keyed(rate, 300.0, 5.0);
        samples.extend(picture(rate, 3.0));
        let events = run(&mut detector, &samples, 0);
        assert!(matches!(events.first(), Some(AptEvent::StartAccepted { .. })));
        let ended = events
            .iter()
            .find_map(|event| match event {
                AptEvent::StartEnded { sample } => Some(*sample),
                _ => None,
            })
            .expect("the tone ended");
        let tone_samples = f64::from(rate) * 5.0;
        assert!(
            (ended as f64 - tone_samples).abs() < f64::from(rate),
            "the tone ended at {tone_samples} but was reported at {ended}"
        );
    }

    #[test]
    fn a_tone_shorter_than_the_hold_is_not_accepted() {
        let rate = 11_025;
        let mut detector = AptDetector::new(f64::from(rate)).unwrap();
        let events = run(&mut detector, &keyed(rate, 300.0, 1.0), 0);
        assert!(events.is_empty(), "{events:?}");
    }

    #[test]
    fn picture_content_accepts_nothing() {
        let rate = 11_025;
        let mut detector = AptDetector::new(f64::from(rate)).unwrap();
        let events = run(&mut detector, &picture(rate, 20.0), 0);
        assert!(events.is_empty(), "{events:?}");
        let strengths = detector.strengths();
        assert!(strengths.start_576 < MIN_STRENGTH as f32, "{strengths:?}");
        assert!(strengths.start_288 < MIN_STRENGTH as f32, "{strengths:?}");
        assert!(strengths.stop < MIN_STRENGTH as f32, "{strengths:?}");
    }

    #[rstest]
    #[case(300.0)]
    #[case(450.0)]
    #[case(675.0)]
    fn one_tone_leaves_the_other_detectors_quiet(#[case] alternation_hz: f64) {
        let rate = 11_025;
        let mut detector = AptDetector::new(f64::from(rate)).unwrap();
        run(&mut detector, &keyed(rate, alternation_hz, 5.0), 0);
        let strengths = detector.strengths();
        let readings = [
            (300.0, strengths.start_576),
            (675.0, strengths.start_288),
            (450.0, strengths.stop),
        ];
        for (frequency, strength) in readings {
            if frequency == alternation_hz {
                assert!(strength > MIN_STRENGTH as f32, "{frequency} Hz read {strength}");
            } else {
                assert!(strength < MIN_STRENGTH as f32, "{frequency} Hz read {strength}");
            }
        }
    }

    #[rstest]
    #[case(8_000)]
    #[case(11_025)]
    #[case(48_000)]
    fn calibration_makes_the_strengths_comparable_across_rates(#[case] rate: u32) {
        // A full-deviation tone is a strength of one by construction, whatever
        // the rate and whichever of the three it is.
        let mut detector = AptDetector::new(f64::from(rate)).unwrap();
        run(&mut detector, &keyed(rate, 450.0, 4.0), 0);
        let strength = f64::from(detector.strengths().stop);
        assert!((strength - 1.0).abs() < 0.05, "{rate} Hz read {strength}");
    }

    #[test]
    fn reset_forgets_a_tone_in_progress() {
        let rate = 11_025;
        let mut detector = AptDetector::new(f64::from(rate)).unwrap();
        run(&mut detector, &keyed(rate, 300.0, 1.5), 0);
        detector.reset();
        let events = run(&mut detector, &keyed(rate, 300.0, 1.5), 0);
        assert!(events.is_empty(), "{events:?}");
    }
}
