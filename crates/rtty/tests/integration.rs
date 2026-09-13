//! End-to-end tests: every signal is synthesized in-process, either by this
//! crate's own transmitter or, for the independent vectors, by a plain sine
//! written in the test itself.

use std::f64::consts::TAU;

use grayline_rtty::{
    ReceivePipeline, RxConfig, RxEvent, RxOutcome, Transmitter, TxCode, TxConfig, encode_text,
    params::{BaudRate, Parity, ToneSet},
    rx::{AfcConfig, AtcDesign},
    tx::Diddle,
};
use rstest::rstest;

const DEFAULT_RATE: u32 = 11_025;
const DEFAULT_PACKET: usize = 1_024;
const MESSAGE: &str = "CQ CQ DE JL1HIS JL1HIS K\r\n";

fn synthesize(text: &str, sample_rate_hz: u32, config: TxConfig) -> Vec<f32> {
    let codes = encode_text(text, &config).unwrap();
    render(codes, sample_rate_hz, config)
}

fn render(codes: Vec<TxCode>, sample_rate_hz: u32, config: TxConfig) -> Vec<f32> {
    let mut transmitter = Transmitter::new(codes.into_iter(), sample_rate_hz, config).unwrap();
    let mut samples = Vec::new();
    loop {
        let mut block = [0.0; 1_024];
        let count = transmitter.process(&mut block).unwrap();
        if count == 0 {
            return samples;
        }
        samples.extend_from_slice(&block[..count]);
    }
}

fn decode(samples: &[f32], sample_rate_hz: u32, config: RxConfig, packet: usize) -> (String, RxOutcome) {
    let mut pipeline = ReceivePipeline::new(sample_rate_hz, config).unwrap();
    let mut text = String::new();
    for chunk in samples.chunks(packet) {
        pipeline
            .process(chunk, |event| {
                if let RxEvent::Character { character, .. } = event {
                    text.push(*character);
                }
            })
            .unwrap();
    }
    (text, pipeline.finish())
}

fn round_trip(text: &str, sample_rate_hz: u32, tx: TxConfig, rx: RxConfig) -> (String, RxOutcome) {
    let samples = synthesize(text, sample_rate_hz, tx);
    decode(&samples, sample_rate_hz, rx, DEFAULT_PACKET)
}

/// A deterministic uniform generator in `[-1, 1]`, so the noise tests need
/// no external dependency and reproduce exactly.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1_u64 << 52) as f64 - 1.0
    }
}

#[test]
fn text_survives_a_round_trip_at_the_amateur_defaults() {
    let (text, outcome) = round_trip(MESSAGE, DEFAULT_RATE, TxConfig::default(), RxConfig::default());
    assert_eq!(text, MESSAGE);
    assert_eq!(outcome.characters, MESSAGE.len());
    assert_eq!(outcome.framing_errors, 0);
}

#[rstest]
#[case(8_000)]
#[case(11_025)]
#[case(44_100)]
#[case(48_000)]
fn the_round_trip_holds_at_every_capture_rate(#[case] sample_rate_hz: u32) {
    let (text, _) = round_trip(MESSAGE, sample_rate_hz, TxConfig::default(), RxConfig::default());
    assert_eq!(text, MESSAGE);
}

#[rstest]
#[case(45.45)]
#[case(75.0)]
#[case(100.0)]
fn the_round_trip_holds_at_other_speeds(#[case] baud: f64) {
    let mut tx = TxConfig::default();
    tx.framing.baud = BaudRate::new(baud).unwrap();
    let mut rx = RxConfig::default();
    rx.framing.baud = tx.framing.baud;
    let (text, _) = round_trip(MESSAGE, DEFAULT_RATE, tx, rx);
    assert_eq!(text, MESSAGE);
}

#[rstest]
#[case(ToneSet::from_center_and_shift(2_210.0, 200.0))]
#[case(ToneSet::from_center_and_shift(2_210.0, 450.0))]
#[case(ToneSet { mark_hz: 1_275.0, space_hz: 1_445.0 })]
#[case(ToneSet { mark_hz: 915.0, space_hz: 1_085.0 })]
fn the_round_trip_holds_at_other_tone_pairs(#[case] tones: ToneSet) {
    let tx = TxConfig {
        tones,
        ..TxConfig::default()
    };
    let rx = RxConfig {
        tones,
        ..RxConfig::default()
    };
    let (text, _) = round_trip(MESSAGE, DEFAULT_RATE, tx, rx);
    assert_eq!(text, MESSAGE);
}

#[test]
fn figures_and_letters_survive_shifting_both_ways() {
    // The receiver unshifts on space by default, so digit groups separated
    // by spaces only survive when the transmitter re-announces FIGS after
    // each one — which is exactly what TX unshift-on-space is for.
    let tx = TxConfig {
        tx_unshift_on_space: true,
        ..TxConfig::default()
    };
    let message = "RST 599 599 QTH TOKYO ES 73 ?\r\n";
    let (text, _) = round_trip(message, DEFAULT_RATE, tx, RxConfig::default());
    assert_eq!(text, message);
}

#[test]
fn receive_uos_alone_breaks_a_spaced_digit_group() {
    // The failure mode `docs/memo/rtty/protocol.md` warns about, pinned so
    // it stays a documented consequence rather than a surprise.
    let (text, _) = round_trip("599 599\r\n", DEFAULT_RATE, TxConfig::default(), RxConfig::default());
    assert_eq!(text, "599 TOO\r\n");
}

#[test]
fn unshift_on_space_recovers_from_a_lost_letters_shift() {
    // FIGS, "5", space, then a letters character with the LTRS deliberately
    // missing, as a burst of noise would leave it.
    let codes = vec![
        TxCode::Character(0b11011),
        TxCode::Character(0b00001),
        TxCode::Character(0b00100),
        TxCode::Character(0b11000),
    ];
    let samples = render(codes, DEFAULT_RATE, TxConfig::default());

    let (with_uos, _) = decode(&samples, DEFAULT_RATE, RxConfig::default(), DEFAULT_PACKET);
    assert_eq!(with_uos, "5 A");

    let without_uos = RxConfig {
        unshift_on_space: false,
        ..RxConfig::default()
    };
    let (text, _) = decode(&samples, DEFAULT_RATE, without_uos, DEFAULT_PACKET);
    assert_eq!(text, "5 -");
}

#[test]
fn reverse_on_both_sides_is_the_same_text_and_on_one_side_is_not() {
    let tx = TxConfig {
        reverse: true,
        ..TxConfig::default()
    };
    let samples = synthesize(MESSAGE, DEFAULT_RATE, tx);

    let rx = RxConfig {
        reverse: true,
        ..RxConfig::default()
    };
    let (text, _) = decode(&samples, DEFAULT_RATE, rx, DEFAULT_PACKET);
    assert_eq!(text, MESSAGE);

    let (inverted, _) = decode(&samples, DEFAULT_RATE, RxConfig::default(), DEFAULT_PACKET);
    assert_ne!(inverted, MESSAGE);
}

#[rstest]
#[case(None)]
#[case(Some(100.0))]
fn the_transmit_smoothing_filter_does_not_break_the_round_trip(#[case] smoothing_hz: Option<f64>) {
    let tx = TxConfig {
        smoothing_hz,
        ..TxConfig::default()
    };
    let (text, _) = round_trip(MESSAGE, DEFAULT_RATE, tx, RxConfig::default());
    assert_eq!(text, MESSAGE);
}

#[test]
fn a_parity_bit_survives_the_round_trip() {
    let mut tx = TxConfig::default();
    tx.framing.parity = Parity::Even;
    let mut rx = RxConfig::default();
    rx.framing.parity = Parity::Even;
    let (text, outcome) = round_trip(MESSAGE, DEFAULT_RATE, tx, rx);
    assert_eq!(text, MESSAGE);
    assert_eq!(outcome.parity_errors, 0);
}

#[rstest]
#[case(Diddle::None)]
#[case(Diddle::Ltrs)]
#[case(Diddle::Blank)]
fn diddle_between_characters_does_not_change_the_text(#[case] diddle: Diddle) {
    let tx = TxConfig {
        diddle,
        char_gap_bits: 9.0,
        ..TxConfig::default()
    };
    let message = "AB 12 CD\r\n";
    let (text, _) = round_trip(message, DEFAULT_RATE, tx, RxConfig::default());
    assert_eq!(text, message);
}

#[rstest]
#[case(73)]
#[case(1_024)]
#[case(4_093)]
fn the_packet_size_does_not_change_the_text(#[case] packet: usize) {
    let samples = synthesize(MESSAGE, DEFAULT_RATE, TxConfig::default());
    let (text, _) = decode(&samples, DEFAULT_RATE, RxConfig::default(), packet);
    assert_eq!(text, MESSAGE);
}

#[test]
fn noise_alone_produces_no_text() {
    let mut generator = Lcg(0x1234_5678_9abc_def0);
    let samples: Vec<f32> = (0..DEFAULT_RATE * 3).map(|_| (generator.next() * 0.5) as f32).collect();
    let (text, outcome) = decode(&samples, DEFAULT_RATE, RxConfig::default(), DEFAULT_PACKET);
    assert_eq!(text, "", "noise printed {text:?}");
    assert_eq!(outcome.characters, 0);
}

#[test]
fn additive_noise_at_ten_decibels_still_decodes() {
    let mut samples = synthesize(MESSAGE, DEFAULT_RATE, TxConfig::default());
    // Signal power is 0.9²/2; uniform noise at ±amplitude carries
    // amplitude²/3, so this amplitude puts the broadband SNR at 10 dB.
    let amplitude = (3.0_f64 * 0.9 * 0.9 / 2.0 / 10.0).sqrt();
    let mut generator = Lcg(0x0dd0_13aa_5555_aaaa);
    for sample in &mut samples {
        *sample += (generator.next() * amplitude) as f32;
    }
    let (text, _) = decode(&samples, DEFAULT_RATE, RxConfig::default(), DEFAULT_PACKET);
    assert_eq!(text, MESSAGE);
}

#[test]
fn the_atc_does_not_break_a_clean_round_trip() {
    let rx = RxConfig {
        atc: Some(AtcDesign::default()),
        ..RxConfig::default()
    };
    let (text, _) = round_trip(MESSAGE, DEFAULT_RATE, TxConfig::default(), rx);
    assert_eq!(text, MESSAGE);
}

/// A bit-level synthesizer independent of the transmitter: a plain phase
/// accumulator and `sin`, driven by hand-written bit sequences.
struct HandSynth {
    samples: Vec<f32>,
    phase: f64,
    written: f64,
    sample_rate_hz: f64,
    samples_per_bit: f64,
}

impl HandSynth {
    fn new(sample_rate_hz: f64) -> Self {
        Self {
            samples: Vec::new(),
            phase: 0.0,
            written: 0.0,
            sample_rate_hz,
            samples_per_bit: sample_rate_hz / 45.45,
        }
    }

    fn tone(&mut self, frequency_hz: f64, bits: f64) {
        self.written += bits * self.samples_per_bit;
        while (self.samples.len() as f64) < self.written {
            self.samples.push((0.9 * self.phase.sin()) as f32);
            self.phase += TAU * frequency_hz / self.sample_rate_hz;
        }
    }

    /// One character from its transmission-order bit string, b1 first.
    fn character(&mut self, bits: &str, stop_bits: f64) {
        self.tone(ToneSet::AFSK_170.space_hz, 1.0);
        for bit in bits.chars() {
            let frequency = if bit == '1' {
                ToneSet::AFSK_170.mark_hz
            } else {
                ToneSet::AFSK_170.space_hz
            };
            self.tone(frequency, 1.0);
        }
        self.tone(ToneSet::AFSK_170.mark_hz, stop_bits);
    }
}

/// The independence check the self-round-trip cannot make: these bits were
/// derived by hand from the table in `docs/memo/rtty/protocol.md`, and the
/// waveform never touches the transmitter, so a bit-order mistake shared by
/// both sides would still fail here.
#[test]
fn a_hand_derived_bit_pattern_decodes() {
    let mut synth = HandSynth::new(f64::from(DEFAULT_RATE));
    synth.tone(ToneSet::AFSK_170.mark_hz, 30.0);
    for bits in ["01010", "10101", "01010", "10101"] {
        synth.character(bits, 1.5);
    }
    synth.tone(ToneSet::AFSK_170.mark_hz, 4.0);
    let (text, _) = decode(&synth.samples, DEFAULT_RATE, RxConfig::default(), DEFAULT_PACKET);
    assert_eq!(text, "RYRY");
}

/// The transmitter against the same hand-derived keying: each bit window of
/// its output must sit on the tone the hand sequence says.
#[test]
fn the_transmitter_matches_the_hand_derived_keying() {
    let tx = TxConfig {
        smoothing_hz: None,
        band_pass: false,
        ramp_seconds: 0.0,
        lead_in_seconds: 0.0,
        tail_seconds: 0.0,
        amplitude: 1.0,
        ..TxConfig::default()
    };
    let rate = 48_000;
    let samples = render(vec![TxCode::Character(0b01010)], rate, tx);
    // R is 01010: start space, then space mark space mark space, then the
    // 1.5-bit mark stop element.
    let expected = ["0", "0", "1", "0", "1", "0"];
    let samples_per_bit = f64::from(rate) / 45.45;
    for (index, bit) in expected.iter().enumerate() {
        let start = (index as f64 * samples_per_bit) as usize;
        let end = ((index + 1) as f64 * samples_per_bit) as usize;
        let window = &samples[start..end];
        let mut crossings = 0;
        for pair in window.windows(2) {
            if pair[0] < 0.0 && pair[1] >= 0.0 {
                crossings += 1;
            }
        }
        let tone = f64::from(crossings) * f64::from(rate) / window.len() as f64;
        let expected_tone = if *bit == "1" { 2_125.0 } else { 2_295.0 };
        assert!(
            (tone - expected_tone).abs() <= 60.0,
            "bit {index} measured {tone}, wanted {expected_tone}"
        );
    }
}

#[test]
fn ignoring_framing_errors_keeps_the_character_and_counts_the_error() {
    // A, then M with its stop element broken to space, then B.
    let mut synth = HandSynth::new(f64::from(DEFAULT_RATE));
    synth.tone(ToneSet::AFSK_170.mark_hz, 30.0);
    synth.character("11000", 1.5);
    synth.character("00111", 0.0);
    synth.tone(ToneSet::AFSK_170.space_hz, 1.0);
    synth.tone(ToneSet::AFSK_170.mark_hz, 3.0);
    synth.character("10011", 1.5);
    synth.tone(ToneSet::AFSK_170.mark_hz, 4.0);

    let (text, outcome) = decode(&synth.samples, DEFAULT_RATE, RxConfig::default(), DEFAULT_PACKET);
    assert_eq!(text, "AB");
    assert_eq!(outcome.framing_errors, 1);

    let ignore = RxConfig {
        ignore_framing_errors: true,
        ..RxConfig::default()
    };
    let (text, outcome) = decode(&synth.samples, DEFAULT_RATE, ignore, DEFAULT_PACKET);
    assert_eq!(text, "AMB");
    assert_eq!(outcome.framing_errors, 1);
}

#[test]
fn afc_pulls_a_detuned_transmission_back() {
    // The transmitter sits 90 Hz high: past the comparator midpoint, so
    // nothing decodes until the AFC has walked the detectors up onto it.
    let detuned = ToneSet {
        mark_hz: 2_215.0,
        space_hz: 2_385.0,
    };
    let tx = TxConfig {
        tones: detuned,
        ..TxConfig::default()
    };
    let samples = synthesize("RYRYRYRYRYRYRYRYRYRY CQ TEST DE JL1HIS\r\n", DEFAULT_RATE, tx);

    let rx = RxConfig {
        afc: Some(AfcConfig::default()),
        ..RxConfig::default()
    };
    let (text, outcome) = decode(&samples, DEFAULT_RATE, rx, DEFAULT_PACKET);
    assert!(text.contains("CQ TEST DE JL1HIS"), "decoded {text:?}");
    // The keying sidebands bias the spectral peaks a few hertz, so the
    // settled pair sits near the true tones rather than exactly on them;
    // well within the 60 Hz resonators.
    assert!(
        (outcome.tones.mark_hz - detuned.mark_hz).abs() <= 10.0,
        "mark settled at {}",
        outcome.tones.mark_hz
    );

    let deaf = RxConfig::default();
    let (text, _) = decode(&samples, DEFAULT_RATE, deaf, DEFAULT_PACKET);
    assert!(!text.contains("CQ TEST DE JL1HIS"), "decoded without AFC: {text:?}");
}

#[test]
fn the_afc_result_does_not_depend_on_the_packet_size() {
    let tx = TxConfig {
        tones: ToneSet {
            mark_hz: 2_215.0,
            space_hz: 2_385.0,
        },
        ..TxConfig::default()
    };
    let samples = synthesize("RYRYRYRYRYRYRYRYRYRY CQ TEST\r\n", DEFAULT_RATE, tx);
    let rx = RxConfig {
        afc: Some(AfcConfig::default()),
        ..RxConfig::default()
    };
    let (_, small) = decode(&samples, DEFAULT_RATE, rx, 73);
    let (_, large) = decode(&samples, DEFAULT_RATE, rx, 1_024);
    assert_eq!(small.tones, large.tones);
}
