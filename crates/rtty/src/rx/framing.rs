use crate::params::{Parity, RxFraming};

/// One comparator decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Bit {
    /// The mark tone dominated the sample.
    Mark,
    /// The space tone dominated the sample.
    Space,
}

/// What one framed character came out as.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FramingOutcome {
    /// A completed character.
    ///
    /// `stop_was_space` marks a framing error; the code is still carried so
    /// a caller ignoring framing errors can keep it, which is what MMTTY's
    /// `ignoreFream` does (`Rtty.cpp:1143`).
    Code {
        /// The data bits, first-received bit as the most significant.
        code: u8,
        /// Whether the stop element failed its majority vote.
        stop_was_space: bool,
    },
    /// The parity bit contradicted the data bits; the character is lost.
    ParityError,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Idle,
    StartHalf,
    StartRest,
    Data,
    Parity,
    Stop,
    Resync,
    Recover,
}

/// Start-stop framing by majority vote, MMTTY's default decoder.
///
/// Each bit decision counts every sample across the bit period and takes the
/// majority, so the state machine's boundaries sit on the bit transitions
/// rather than the bit centres. Bit timing is a fractional sample
/// accumulator: the original's integer down-counter loses almost two samples
/// per character at 45.45 baud, an error the accumulator does not make.
#[derive(Clone, Debug)]
pub(crate) struct MajorityFraming {
    samples_per_bit: f64,
    framing: RxFraming,
    state: State,
    elapsed: u64,
    boundary: f64,
    marks: u32,
    spaces: u32,
    data: u8,
    remaining_bits: u8,
    data_marks: u32,
}

impl MajorityFraming {
    pub(crate) fn new(sample_rate_hz: f64, framing: RxFraming) -> Self {
        Self {
            samples_per_bit: sample_rate_hz / framing.baud.bits_per_second(),
            framing,
            state: State::Idle,
            elapsed: 0,
            boundary: 0.0,
            marks: 0,
            spaces: 0,
            data: 0,
            remaining_bits: 0,
            data_marks: 0,
        }
    }

    /// Returns whether the machine is between characters.
    ///
    /// The squelch may clamp the comparator to mark only in this state, so a
    /// character already in progress when the squelch closes still finishes.
    pub(crate) fn is_idle(&self) -> bool {
        matches!(self.state, State::Idle | State::Resync | State::Recover)
    }

    /// Consumes one comparator decision, completing at most one character.
    pub(crate) fn process(&mut self, bit: Bit) -> Option<FramingOutcome> {
        self.elapsed += 1;
        match self.state {
            State::Idle => {
                if bit == Bit::Space {
                    // The falling edge: vote over the first half of the
                    // start bit only, as the original does (`Rtty.cpp:881`).
                    self.marks = 0;
                    self.spaces = 1;
                    self.boundary = self.elapsed as f64 + self.samples_per_bit * 0.5;
                    self.state = State::StartHalf;
                }
                None
            }
            State::StartHalf => {
                self.vote(bit);
                if self.expired() {
                    if self.majority_is_mark() {
                        self.state = State::Idle;
                    } else {
                        // The second half passes without voting
                        // (`Rtty.cpp:1039`).
                        self.boundary += self.samples_per_bit * 0.5;
                        self.state = State::StartRest;
                    }
                }
                None
            }
            State::StartRest => {
                if self.expired() {
                    self.begin_bit();
                    self.data = 0;
                    self.data_marks = 0;
                    self.remaining_bits = self.framing.bits.bits();
                    self.state = State::Data;
                }
                None
            }
            State::Data => {
                self.vote(bit);
                if self.expired() {
                    let is_mark = self.majority_is_mark();
                    self.data = (self.data << 1) | u8::from(is_mark);
                    self.data_marks += u32::from(is_mark);
                    self.remaining_bits -= 1;
                    if self.remaining_bits > 0 {
                        self.begin_bit();
                    } else if self.framing.parity == Parity::None {
                        self.begin_stop();
                    } else {
                        self.begin_bit();
                        self.state = State::Parity;
                    }
                }
                None
            }
            State::Parity => {
                self.vote(bit);
                if self.expired() {
                    if self.framing.parity.is_valid(self.data_marks, self.majority_is_mark()) {
                        self.begin_stop();
                        None
                    } else {
                        // Wait one bit before hunting again, as the original's
                        // parity recovery does.
                        self.boundary += self.samples_per_bit;
                        self.state = State::Resync;
                        Some(FramingOutcome::ParityError)
                    }
                } else {
                    None
                }
            }
            State::Stop => {
                self.vote(bit);
                if self.expired() {
                    let stop_was_space = !self.majority_is_mark();
                    if stop_was_space {
                        self.state = State::Recover;
                    } else {
                        let resync = self.framing.stop.resync_bits();
                        if resync > 0.0 {
                            self.boundary += resync * self.samples_per_bit;
                            self.state = State::Resync;
                        } else {
                            self.state = State::Idle;
                        }
                    }
                    Some(FramingOutcome::Code {
                        code: self.data,
                        stop_was_space,
                    })
                } else {
                    None
                }
            }
            State::Resync => {
                if self.expired() {
                    self.state = State::Idle;
                    if bit == Bit::Space {
                        return self.process_reentry();
                    }
                }
                None
            }
            State::Recover => {
                // A framing error means the line is spacing where a stop
                // belongs; hunting again immediately would frame noise, so
                // wait for the line to return to mark.
                if bit == Bit::Mark {
                    self.state = State::Idle;
                }
                None
            }
        }
    }

    /// Re-runs the idle test for the sample that ended a resync wait, so a
    /// start bit arriving exactly then is not consumed by the wait.
    fn process_reentry(&mut self) -> Option<FramingOutcome> {
        self.marks = 0;
        self.spaces = 1;
        self.boundary = self.elapsed as f64 + self.samples_per_bit * 0.5;
        self.state = State::StartHalf;
        None
    }

    fn vote(&mut self, bit: Bit) {
        match bit {
            Bit::Mark => self.marks += 1,
            Bit::Space => self.spaces += 1,
        }
    }

    fn expired(&self) -> bool {
        self.elapsed as f64 >= self.boundary
    }

    fn majority_is_mark(&self) -> bool {
        self.marks >= self.spaces
    }

    fn begin_bit(&mut self) {
        self.marks = 0;
        self.spaces = 0;
        self.boundary += self.samples_per_bit;
    }

    fn begin_stop(&mut self) {
        self.marks = 0;
        self.spaces = 0;
        self.boundary += self.framing.stop.confirm_bits() * self.samples_per_bit;
        self.state = State::Stop;
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::*;
    use crate::params::{BaudRate, BitLength, StopTolerance};
    use rstest::rstest;

    fn framing(baud: f64, stop: StopTolerance) -> RxFraming {
        RxFraming {
            baud: BaudRate::new(baud).unwrap(),
            bits: BitLength::FIVE,
            parity: Parity::None,
            stop,
        }
    }

    /// Feeds `bit` for `bits` bit periods at the machine's own rate,
    /// tracking fractional bit boundaries the way a transmitter would.
    struct Driver {
        machine: MajorityFraming,
        rate: f64,
        baud: f64,
        written: u64,
        deadline: f64,
        outcomes: Vec<FramingOutcome>,
    }

    impl Driver {
        fn new(rate: f64, framing: RxFraming) -> Self {
            Self {
                machine: MajorityFraming::new(rate, framing),
                rate,
                baud: framing.baud.bits_per_second(),
                written: 0,
                deadline: 0.0,
                outcomes: Vec::new(),
            }
        }

        fn feed(&mut self, bit: Bit, bits: f64) {
            self.deadline += bits * self.rate / self.baud;
            while (self.written as f64) < self.deadline {
                if let Some(outcome) = self.machine.process(bit) {
                    self.outcomes.push(outcome);
                }
                self.written += 1;
            }
        }

        fn character(&mut self, code: u8, stop_bits: f64) {
            self.feed(Bit::Space, 1.0);
            for index in (0..5).rev() {
                let bit = if (code >> index) & 1 == 1 {
                    Bit::Mark
                } else {
                    Bit::Space
                };
                self.feed(bit, 1.0);
            }
            self.feed(Bit::Mark, stop_bits);
        }
    }

    #[rstest]
    #[case(8_000.0, 22.0)]
    #[case(11_025.0, 45.45)]
    #[case(44_100.0, 75.0)]
    #[case(48_000.0, 300.0)]
    fn characters_decode_across_rates_and_speeds(#[case] rate: f64, #[case] baud: f64) {
        let mut driver = Driver::new(rate, framing(baud, StopTolerance::Ratio142));
        driver.feed(Bit::Mark, 4.0);
        for code in [0b11000, 0b10011, 0b00111] {
            driver.character(code, 1.5);
        }
        let expected: Vec<FramingOutcome> = [0b11000, 0b10011, 0b00111]
            .into_iter()
            .map(|code| FramingOutcome::Code {
                code,
                stop_was_space: false,
            })
            .collect();
        assert_eq!(driver.outcomes, expected);
    }

    #[test]
    fn a_glitch_shorter_than_half_a_bit_is_rejected_as_noise() {
        let mut driver = Driver::new(11_025.0, framing(45.45, StopTolerance::Ratio142));
        driver.feed(Bit::Mark, 2.0);
        driver.feed(Bit::Space, 0.2);
        driver.feed(Bit::Mark, 4.0);
        assert!(driver.outcomes.is_empty());
        driver.character(0b10101, 1.5);
        assert_eq!(driver.outcomes.len(), 1);
    }

    #[test]
    fn a_spaced_stop_element_reports_the_framing_error_and_recovers() {
        let mut driver = Driver::new(11_025.0, framing(45.45, StopTolerance::Ratio142));
        driver.feed(Bit::Mark, 2.0);
        // A character whose stop element is space: the code still comes out,
        // flagged, and the next clean character decodes.
        driver.feed(Bit::Space, 1.0);
        for _ in 0..5 {
            driver.feed(Bit::Mark, 1.0);
        }
        driver.feed(Bit::Space, 1.5);
        driver.feed(Bit::Mark, 2.0);
        driver.character(0b00111, 1.5);
        assert_eq!(
            driver.outcomes,
            [
                FramingOutcome::Code {
                    code: 0b11111,
                    stop_was_space: true
                },
                FramingOutcome::Code {
                    code: 0b00111,
                    stop_was_space: false
                },
            ]
        );
    }

    #[test]
    fn the_one_bit_tolerance_confirms_within_seven_eighths_of_a_bit() {
        // With a 1-bit stop and no idle between characters, back-to-back
        // characters only decode because the confirmation window ends before
        // the next start bit begins.
        let mut driver = Driver::new(11_025.0, framing(45.45, StopTolerance::One));
        driver.feed(Bit::Mark, 2.0);
        for _ in 0..4 {
            driver.character(0b01010, 1.0);
        }
        assert_eq!(driver.outcomes.len(), 4);
        assert!(driver.outcomes.iter().all(|outcome| matches!(
            outcome,
            FramingOutcome::Code {
                code: 0b01010,
                stop_was_space: false
            }
        )));
    }

    #[rstest]
    #[case(Parity::Even)]
    #[case(Parity::Odd)]
    #[case(Parity::Mark)]
    #[case(Parity::Space)]
    fn a_correct_parity_bit_passes_and_a_wrong_one_is_an_error(#[case] parity: Parity) {
        let mut with_parity = framing(45.45, StopTolerance::Ratio142);
        with_parity.parity = parity;
        let code = 0b10110_u8;
        let good = parity.transmitted_bit(code.count_ones()).unwrap();

        for (sent, expected) in [
            (
                good,
                FramingOutcome::Code {
                    code,
                    stop_was_space: false,
                },
            ),
            (!good, FramingOutcome::ParityError),
        ] {
            let mut driver = Driver::new(11_025.0, with_parity);
            driver.feed(Bit::Mark, 2.0);
            driver.feed(Bit::Space, 1.0);
            for index in (0..5).rev() {
                let bit = if (code >> index) & 1 == 1 {
                    Bit::Mark
                } else {
                    Bit::Space
                };
                driver.feed(bit, 1.0);
            }
            driver.feed(if sent { Bit::Mark } else { Bit::Space }, 1.0);
            driver.feed(Bit::Mark, 3.0);
            assert_eq!(driver.outcomes, [expected], "{parity:?} sent {sent}");
        }
    }

    #[rstest]
    #[case(6)]
    #[case(7)]
    #[case(8)]
    fn wider_characters_frame_at_the_framing_layer(#[case] bits: u8) {
        let mut wide = framing(45.45, StopTolerance::Ratio142);
        wide.bits = BitLength::new(bits).unwrap();
        let code = 0b1011_0101_u8 >> (8 - bits);
        let mut driver = Driver::new(11_025.0, wide);
        driver.feed(Bit::Mark, 2.0);
        driver.feed(Bit::Space, 1.0);
        for index in (0..bits).rev() {
            let bit = if (code >> index) & 1 == 1 {
                Bit::Mark
            } else {
                Bit::Space
            };
            driver.feed(bit, 1.0);
        }
        driver.feed(Bit::Mark, 3.0);
        assert_eq!(
            driver.outcomes,
            [FramingOutcome::Code {
                code,
                stop_was_space: false
            }]
        );
    }

    /// The resync fractions exist so a fast transmitter's next start bit
    /// still lands inside the hunt window.
    #[rstest]
    #[case(0.98)]
    #[case(1.0)]
    #[case(1.02)]
    fn a_two_percent_fast_or_slow_transmitter_still_decodes(#[case] speed: f64) {
        let rate = 11_025.0;
        let mut driver = Driver::new(rate, framing(45.45, StopTolerance::Ratio142));
        // The transmitter's bit lasts 1/speed of the receiver's, so feed in
        // transmitter bits by scaling the durations.
        let scale = 1.0 / speed;
        driver.feed(Bit::Mark, 4.0);
        let text = [0b11000, 0b10011, 0b00111, 0b10101, 0b01010];
        for code in text {
            driver.feed(Bit::Space, scale);
            for index in (0..5_u8).rev() {
                let bit = if (code >> index) & 1 == 1 {
                    Bit::Mark
                } else {
                    Bit::Space
                };
                driver.feed(bit, scale);
            }
            driver.feed(Bit::Mark, 1.5 * scale);
        }
        let codes: Vec<u8> = driver
            .outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                FramingOutcome::Code {
                    code,
                    stop_was_space: false,
                } => Some(*code),
                _ => None,
            })
            .collect();
        assert_eq!(codes, text, "at speed {speed}");
    }

    #[test]
    fn the_machine_reports_idle_only_between_characters() {
        let mut machine = MajorityFraming::new(11_025.0, framing(45.45, StopTolerance::Ratio142));
        assert!(machine.is_idle());
        machine.process(Bit::Space);
        assert!(!machine.is_idle());
    }
}
