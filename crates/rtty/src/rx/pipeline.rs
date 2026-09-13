use alloc::vec::Vec;

use grayline_dsp::transform::{RealSpectrum, SpectrumWindow};

use crate::{
    RttyError,
    code::{Case, Ita2Decoder},
    params::{BitLength, ToneSet},
    rx::{
        afc::Afc,
        config::RxConfig,
        event::{RxEvent, RxOutcome},
        framing::{Bit, FramingOutcome, MajorityFraming},
        frontend::FrontEnd,
        monitor::{ChannelLevels, Monitor, MonitorConfig},
    },
};

/// The lowest capture rate the front end supports.
pub const MINIMUM_SAMPLE_RATE_HZ: u32 = 6_000;
/// The fewest samples a majority vote needs from one bit.
const MINIMUM_SAMPLES_PER_BIT: f64 = 8.0;
/// The widest FFT bin the AFC search accepts, in hertz; the transform length
/// is the smallest power of two that reaches it.
const AFC_MAXIMUM_BIN_HZ: f64 = 6.0;

struct AfcState {
    afc: Afc,
    spectrum: RealSpectrum,
    /// A ring over the raw input. The spectrum reads the signal before the
    /// band-pass, whose job is to hide everything a detuned transmission
    /// lives in; the search thresholds are ratios, so the unnormalized
    /// input costs nothing.
    ring: Vec<f64>,
    write: usize,
    filled: bool,
    /// The ring unrolled into transform order, refilled once per hop.
    scratch: Vec<f64>,
    magnitudes: Vec<f64>,
    hop: usize,
    since_transform: usize,
}

/// The demodulator-to-decoder wiring every offline receiver repeats.
pub struct ReceivePipeline {
    front_end: FrontEnd,
    squelch: crate::rx::squelch::Squelch,
    framing: MajorityFraming,
    decoder: Ita2Decoder,
    afc: Option<AfcState>,
    monitor: Option<Monitor>,
    /// What the comparator last compared, kept whether or not the tap is on:
    /// a tuning readout wants the newest pair rather than a stream of them.
    channels: ChannelLevels,
    ignore_framing_errors: bool,
    squelch_open: bool,
    position: u64,
    characters: usize,
    framing_errors: usize,
    parity_errors: usize,
    sample_rate_hz: f64,
}

impl ReceivePipeline {
    /// Builds a pipeline for a capture rate and a configuration.
    pub fn new(sample_rate_hz: u32, config: RxConfig) -> Result<Self, RttyError> {
        if sample_rate_hz < MINIMUM_SAMPLE_RATE_HZ {
            return Err(RttyError::SampleRateTooLow(sample_rate_hz));
        }
        if config.framing.bits != BitLength::FIVE {
            return Err(RttyError::UnsupportedBitLength);
        }
        let rate = f64::from(sample_rate_hz);
        let samples_per_bit = rate / config.framing.baud.bits_per_second();
        if !samples_per_bit.is_finite() || samples_per_bit < MINIMUM_SAMPLES_PER_BIT {
            return Err(RttyError::TooFewSamplesPerBit);
        }
        // An inverted sideband is decoded by swapping the tone roles, so the
        // reversal folds into the pair everything downstream sees.
        let tones = if config.reverse {
            config.tones.reversed()
        } else {
            config.tones
        };
        let afc = config
            .afc
            .map(|afc_config| -> Result<AfcState, RttyError> {
                let mut length = 2_usize;
                while rate / length as f64 > AFC_MAXIMUM_BIN_HZ {
                    length *= 2;
                }
                Ok(AfcState {
                    afc: Afc::new(afc_config),
                    spectrum: RealSpectrum::new(length, SpectrumWindow::Hann)?,
                    ring: Vec::with_capacity(length),
                    write: 0,
                    filled: false,
                    scratch: alloc::vec![0.0; length],
                    magnitudes: Vec::with_capacity(length / 2 + 1),
                    hop: length / 2,
                    since_transform: 0,
                })
            })
            .transpose()?;
        Ok(Self {
            front_end: FrontEnd::new(rate, tones, &config)?,
            squelch: crate::rx::squelch::Squelch::new(rate, config.squelch_threshold),
            framing: MajorityFraming::new(rate, config.framing),
            decoder: Ita2Decoder::new(config.code_set, config.unshift_on_space),
            afc,
            monitor: None,
            channels: ChannelLevels::default(),
            ignore_framing_errors: config.ignore_framing_errors,
            squelch_open: config.squelch_threshold.is_none(),
            position: 0,
            characters: 0,
            framing_errors: 0,
            parity_errors: 0,
            sample_rate_hz: rate,
        })
    }

    /// Demodulates and decodes one packet of normalized mono PCM.
    pub fn process(&mut self, samples: &[f32], mut on_event: impl FnMut(&RxEvent)) -> Result<(), RttyError> {
        for (index, &sample) in samples.iter().enumerate() {
            if !sample.is_finite() {
                return Err(RttyError::NonFiniteSample { index });
            }
            let input = f64::from(sample);
            self.run_afc(input, &mut on_event)?;
            let output = self.front_end.process(input);
            self.channels = output.levels;
            if let Some(monitor) = &mut self.monitor {
                monitor.observe(output.levels);
            }
            let open = self.squelch.process_sample(output.levels.difference().abs());
            if open != self.squelch_open {
                self.squelch_open = open;
                on_event(&RxEvent::SquelchChanged {
                    open,
                    sample: self.position,
                });
            }
            // A closed squelch clamps the comparator to mark, but only while
            // the framing machine is hunting for a start bit: a character
            // already in progress is allowed to finish (`Rtty.cpp:859`).
            let bit = if !open && self.framing.is_idle() {
                Bit::Mark
            } else {
                output.bit
            };
            if let Some(outcome) = self.framing.process(bit) {
                self.settle(outcome, &mut on_event);
            }
            self.position += 1;
        }
        Ok(())
    }

    /// Returns the smoothed `|mark − space|` strength reading.
    pub fn signal_strength(&self) -> f64 {
        self.squelch.signal_strength()
    }

    /// Returns the case the decoder is reading in.
    pub fn case(&self) -> Case {
        self.decoder.case()
    }

    /// Returns what the comparator last compared.
    ///
    /// The signed difference is the tuning reading: a pair sitting on the
    /// tones swings between its two extremes, while one that is off them
    /// stays near zero.
    pub const fn channels(&self) -> ChannelLevels {
        self.channels
    }

    /// Starts, reconfigures, or stops the monitor tap.
    ///
    /// A display is opened and closed while a reception runs, so this is a
    /// setting rather than a construction argument: rebuilding the pipeline
    /// to open a scope would throw away the reception being watched. A
    /// configuration that is already in effect keeps what has been collected.
    pub fn set_monitor(&mut self, config: Option<MonitorConfig>) {
        match config {
            Some(config) => {
                if self.monitor.as_ref().is_some_and(|monitor| monitor.config() == config) {
                    return;
                }
                self.monitor = Some(Monitor::new(config));
            }
            None => self.monitor = None,
        }
    }

    /// Takes the pairs the tap collected since the last call.
    pub fn drain_monitor(&mut self) -> impl Iterator<Item = ChannelLevels> + '_ {
        self.monitor.as_mut().into_iter().flat_map(Monitor::drain)
    }

    /// Returns the pair being detected, after any AFC movement.
    pub fn tones(&self) -> ToneSet {
        self.front_end.tones()
    }

    /// Ends the reception and hands back its summary.
    pub fn finish(self) -> RxOutcome {
        RxOutcome {
            characters: self.characters,
            framing_errors: self.framing_errors,
            parity_errors: self.parity_errors,
            tones: self.front_end.tones(),
            case: self.decoder.case(),
        }
    }

    fn settle(&mut self, outcome: FramingOutcome, on_event: &mut impl FnMut(&RxEvent)) {
        match outcome {
            FramingOutcome::Code { code, stop_was_space } => {
                if stop_was_space {
                    self.framing_errors += 1;
                    on_event(&RxEvent::FramingError { sample: self.position });
                    if !self.ignore_framing_errors {
                        return;
                    }
                }
                let case_before = self.decoder.case();
                let character = self.decoder.decode(code);
                if self.decoder.case() != case_before {
                    on_event(&RxEvent::CaseChanged {
                        case: self.decoder.case(),
                        sample: self.position,
                    });
                }
                if let Some(character) = character {
                    self.characters += 1;
                    on_event(&RxEvent::Character {
                        character,
                        sample: self.position,
                    });
                }
            }
            FramingOutcome::ParityError => {
                self.parity_errors += 1;
                on_event(&RxEvent::ParityError { sample: self.position });
            }
        }
    }

    /// Accumulates the raw input and runs one AFC step per hop.
    ///
    /// The window slides inside the pipeline, so what the AFC sees does not
    /// depend on how the caller cuts its packets.
    fn run_afc(&mut self, input: f64, on_event: &mut impl FnMut(&RxEvent)) -> Result<(), RttyError> {
        let Some(state) = &mut self.afc else {
            return Ok(());
        };
        let length = state.spectrum.len();
        if !state.filled {
            state.ring.push(input);
            if state.ring.len() < length {
                return Ok(());
            }
            state.filled = true;
        } else {
            state.ring[state.write] = input;
            state.write = (state.write + 1) % length;
            state.since_transform += 1;
            if state.since_transform < state.hop {
                return Ok(());
            }
            state.since_transform = 0;
        }
        // Unroll the ring into transform order; once per hop, so the
        // amortized cost stays constant per sample and nothing allocates.
        let split = length - state.write;
        state.scratch[..split].copy_from_slice(&state.ring[state.write..]);
        state.scratch[split..].copy_from_slice(&state.ring[..state.write]);
        let bins = state.spectrum.transform(&state.scratch)?;
        state.magnitudes.clear();
        state.magnitudes.extend(bins.iter().map(|bin| bin.magnitude()));
        let bin_hz = self.sample_rate_hz / length as f64;
        if let Some(tones) = state.afc.adjust(&state.magnitudes, bin_hz, self.front_end.tones()) {
            self.front_end.set_tones(tones)?;
            on_event(&RxEvent::TonesAdjusted {
                tones,
                sample: self.position,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use core::f64::consts::TAU;

    use super::*;
    use crate::params::ToneSet;

    const RATE: u32 = 11_025;

    fn tone(frequency_hz: f64, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|index| libm::sin(TAU * frequency_hz * index as f64 / f64::from(RATE)) as f32)
            .collect()
    }

    fn settled(frequency_hz: f64) -> ReceivePipeline {
        let mut pipeline = ReceivePipeline::new(RATE, RxConfig::default()).unwrap();
        pipeline
            .process(&tone(frequency_hz, RATE as usize / 4), |_| {})
            .unwrap();
        pipeline
    }

    #[test]
    fn the_channel_reading_leans_towards_the_tone_being_heard() {
        let tones = ToneSet::AFSK_170;
        let mark = settled(tones.mark_hz).channels();
        assert!(mark.difference() > 0.0, "{mark:?}");
        let space = settled(tones.space_hz).channels();
        assert!(space.difference() < 0.0, "{space:?}");
    }

    #[test]
    fn nothing_is_collected_until_the_tap_is_opened() {
        let mut pipeline = settled(ToneSet::AFSK_170.mark_hz);
        assert_eq!(pipeline.drain_monitor().count(), 0);
    }

    #[test]
    fn the_tap_collects_one_pair_per_decimation_window() {
        let mut pipeline = ReceivePipeline::new(RATE, RxConfig::default()).unwrap();
        pipeline.set_monitor(Some(MonitorConfig {
            decimation: 4,
            capacity: 1_024,
        }));
        pipeline.process(&tone(ToneSet::AFSK_170.mark_hz, 400), |_| {}).unwrap();

        assert_eq!(pipeline.drain_monitor().count(), 100);
        assert_eq!(pipeline.drain_monitor().count(), 0);
    }

    #[test]
    fn closing_the_tap_stops_the_collection() {
        let mut pipeline = ReceivePipeline::new(RATE, RxConfig::default()).unwrap();
        pipeline.set_monitor(Some(MonitorConfig {
            decimation: 1,
            capacity: 64,
        }));
        pipeline.set_monitor(None);
        pipeline.process(&tone(ToneSet::AFSK_170.mark_hz, 128), |_| {}).unwrap();
        assert_eq!(pipeline.drain_monitor().count(), 0);
    }

    #[test]
    fn a_tap_that_is_already_open_keeps_what_it_has() {
        let config = MonitorConfig {
            decimation: 1,
            capacity: 64,
        };
        let mut pipeline = ReceivePipeline::new(RATE, RxConfig::default()).unwrap();
        pipeline.set_monitor(Some(config));
        pipeline.process(&tone(ToneSet::AFSK_170.mark_hz, 16), |_| {}).unwrap();
        pipeline.set_monitor(Some(config));
        assert_eq!(pipeline.drain_monitor().count(), 16);
    }
}
