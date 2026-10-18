use alloc::format;

use super::*;

struct Trace(u64);

impl Trace {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn f64(&mut self, value: f64) {
        self.u64(value.to_bits());
    }

    fn debug(&mut self, value: impl core::fmt::Debug) {
        self.bytes(format!("{value:?}").as_bytes());
    }

    fn event(&mut self, event: Option<RxEvent>) {
        match event {
            None => self.u64(0),
            Some(RxEvent::SlantAdjusted {
                unit,
                effective_sample_rate_hz,
            }) => {
                self.u64(1);
                self.u64(unit as u64);
                self.f64(effective_sample_rate_hz);
            }
            Some(RxEvent::RasterAcquired {
                source_epoch,
                effective_sample_rate_hz,
            }) => {
                self.u64(2);
                self.u64(source_epoch);
                self.f64(effective_sample_rate_hz);
            }
            Some(RxEvent::RowDecoded { row }) => {
                self.u64(3);
                self.u64(row as u64);
            }
            Some(RxEvent::PhaseAdjusted {
                unit,
                displacement_samples,
                source_epoch,
            }) => {
                self.u64(4);
                self.u64(unit as u64);
                self.u64(displacement_samples as u64);
                self.u64(source_epoch);
            }
            Some(RxEvent::Stopped { reason }) => {
                self.u64(5);
                self.debug(reason);
            }
        }
    }

    fn snapshot(&mut self, decoder: &RxDecoder) {
        self.debug(decoder.state());
        self.u64(decoder.image_revision());
        self.debug(decoder.source_epoch());
        self.debug(decoder.effective_sample_rate_hz().map(f64::to_bits));
        self.u64(decoder.staged_samples_len() as u64);
    }

    fn image(&mut self, image: &RgbImage) {
        for pixel in image.pixels() {
            self.bytes(&[pixel.r, pixel.g, pixel.b]);
        }
    }

    fn refinement(&mut self, result: Result<RefinementResult, SstvError>) {
        match result {
            Ok(RefinementResult { revision, slant }) => {
                self.u64(revision);
                self.f64(slant.effective_sample_rate_hz);
                self.f64(slant.error_ppm);
                self.f64(slant.residual_samples);
                self.f64(slant.source_epoch);
                self.u64(slant.observations as u64);
            }
            Err(error) => self.debug(error),
        }
    }
}

#[derive(Clone, Copy)]
enum Ending {
    Run,
    StopAtRow(usize),
}

fn reception(
    mode: Mode,
    (frequency, sync): (Vec<f32>, Vec<f32>),
    config: RxConfig,
    chunks: &[usize],
    available: usize,
    ending: Ending,
) -> u64 {
    let absolute_start = 50_000_u64;
    let available = available.min(frequency.len());
    let mut decoder = RxDecoder::with_config(mode, SAMPLE_RATE, config).unwrap();
    let mut trace = Trace::new();
    let mut offset = 0;
    let mut chunk = 0;
    loop {
        if matches!(decoder.state(), RxState::Complete | RxState::Stopped { .. }) {
            break;
        }
        if let Ending::StopAtRow(rows) = ending
            && let RxState::Decoding { completed_rows } = decoder.state()
            && completed_rows >= rows
        {
            decoder.stop(StopReason::SynchronizationLost);
            trace.snapshot(&decoder);
            break;
        }
        let count = chunks[chunk % chunks.len()].min(available - offset);
        chunk += 1;
        let result = decoder.process(DemodulatedBlock::new(
            absolute_start + offset as u64,
            &frequency[offset..offset + count],
            &sync[offset..offset + count],
        ));
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                trace.debug(error);
                break;
            }
        };
        offset += result.consumed();
        trace.u64(result.consumed() as u64);
        trace.event(result.event());
        trace.snapshot(&decoder);
        if result.consumed() == 0 && result.event().is_none() {
            break;
        }
    }
    while let Some(event) = decoder.poll_event() {
        trace.event(Some(event));
        trace.snapshot(&decoder);
    }
    trace.image(decoder.image());
    if matches!(config.staging, Staging::Memory { .. }) {
        if decoder.state() == RxState::Complete {
            let tail = (frequency.len() - offset).min(64);
            let staged = decoder.stage_for_refinement(DemodulatedBlock::new(
                absolute_start + offset as u64,
                &frequency[offset..offset + tail],
                &sync[offset..offset + tail],
            ));
            trace.debug(staged);
        }
        trace.refinement(decoder.refine_staged());
        trace.event(decoder.poll_event());
        trace.snapshot(&decoder);
        trace.image(decoder.image());
        trace.refinement(decoder.refine_staged());
        trace.snapshot(&decoder);
        trace.image(decoder.image());
    }
    trace.debug(decoder.finish());
    trace.0
}

fn staging(samples: usize) -> RxConfig {
    RxConfig {
        staging: Staging::Memory { max_samples: samples },
        ..RxConfig::default()
    }
}

fn live(samples: usize) -> RxConfig {
    RxConfig {
        live_sync: true,
        live_slant: true,
        auto_stop: true,
        ..staging(samples)
    }
}

const WHOLE: &[usize] = &[usize::MAX];
const RAGGED: &[usize] = &[37, 1_009, 3, 4_093];

#[rstest]
#[case::martin2_plain(Mode::Martin2, 0)]
#[case::martin2(Mode::Martin2, 1)]
#[case::scottie2(Mode::Scottie2, 2)]
#[case::robot36(Mode::Robot36, 3)]
#[case::robot72(Mode::Robot72, 4)]
#[case::pd50(Mode::Pd50, 5)]
fn clean_receptions_are_unchanged(#[case] mode: Mode, #[case] variant: usize) {
    let body = sampled_image_body(mode, band_image(mode), 311);
    let length = body.0.len();
    let config = if variant == 0 {
        RxConfig::default()
    } else {
        staging(length + 64)
    };
    let print = reception(mode, body, config, RAGGED, usize::MAX, Ending::Run);
    assert_eq!(print, CLEAN[variant], "{mode:?}: {print:#018x}");
}

const CLEAN: [u64; 6] = [
    0x5199_5022_e169_9173,
    0xcb7c_2da9_7d3c_764c,
    0x2157_f3de_272d_aa20,
    0x4689_491f_5970_40ea,
    0x95f9_4d9e_9107_7952,
    0x74cb_3e0f_e176_69bd,
];

#[rstest]
#[case::martin2_fast(Mode::Martin2, 4_000.0, 0)]
#[case::martin2_slow(Mode::Martin2, -2_500.0, 1)]
#[case::robot36_fast(Mode::Robot36, 4_000.0, 2)]
#[case::pd50_slow(Mode::Pd50, -3_000.0, 3)]
#[case::scottie2_fast(Mode::Scottie2, 1_500.0, 4)]
#[case::robot72_slow(Mode::Robot72, -4_000.0, 5)]
fn live_slant_receptions_are_unchanged(#[case] mode: Mode, #[case] offset_ppm: f64, #[case] variant: usize) {
    let body = mistimed_body(mode, band_image(mode), offset_ppm);
    let length = body.0.len();
    let chunks = if variant.is_multiple_of(2) { WHOLE } else { RAGGED };
    let print = reception(mode, body, live(length + 64), chunks, usize::MAX, Ending::Run);
    assert_eq!(print, SLANT[variant], "{mode:?} {offset_ppm}: {print:#018x}");
}

const SLANT: [u64; 6] = [
    0x2de6_4144_14db_cb94,
    0x81e5_8c3d_1ef0_f42d,
    0x00d8_c67d_7d68_1d0f,
    0xc0b8_9ad7_f29a_70f2,
    0x4db0_21d9_0bcb_e59c,
    0x9dc8_ca28_7665_66ba,
];

#[rstest]
#[case::forward(0)]
#[case::backward(1)]
#[case::lost(2)]
#[case::stopped(3)]
#[case::partial(4)]
#[case::after_header(5)]
#[case::after_header_late(6)]
#[case::exhausted(7)]
fn disturbed_receptions_are_unchanged(#[case] variant: usize) {
    let print = match variant {
        0 => {
            let (frequency, sync) = sampled_body(Mode::Martin2, 311);
            let body = shift_sync(&frequency, &sync, |run| Some(if run < 6 { 0 } else { 4 }));
            let length = body.0.len();
            reception(Mode::Martin2, body, live(length), RAGGED, usize::MAX, Ending::Run)
        }
        1 => {
            let (frequency, sync) = sampled_body(Mode::Scottie2, 311);
            let body = shift_sync(&frequency, &sync, |run| Some(if run < 12 { 24 } else { 0 }));
            let length = body.0.len();
            reception(Mode::Scottie2, body, live(length), WHOLE, usize::MAX, Ending::Run)
        }
        2 => {
            let (frequency, sync) = sampled_body(Mode::Martin2, 311);
            let missing = shift_sync_runs(&sync, |run| (run < 6).then_some(0));
            let length = frequency.len();
            reception(
                Mode::Martin2,
                (frequency, missing),
                live(length),
                RAGGED,
                usize::MAX,
                Ending::Run,
            )
        }
        3 => {
            let body = mistimed_body(Mode::Robot36, band_image(Mode::Robot36), 4_000.0);
            let length = body.0.len();
            reception(
                Mode::Robot36,
                body,
                live(length),
                RAGGED,
                usize::MAX,
                Ending::StopAtRow(37),
            )
        }
        4 => {
            let body = mistimed_body(Mode::Martin2, band_image(Mode::Martin2), 3_000.0);
            let length = body.0.len();
            reception(Mode::Martin2, body, live(length), RAGGED, length / 3, Ending::Run)
        }
        5 | 6 => {
            let (frequency, sync) = sampled_image_body(Mode::Pd50, band_image(Mode::Pd50), 0);
            let skip = if variant == 5 { 0 } else { 40 };
            let body = (frequency[skip..].to_vec(), sync[skip..].to_vec());
            let length = body.0.len();
            let config = RxConfig {
                raster_start: RasterStart::AfterHeader,
                ..live(length)
            };
            reception(Mode::Pd50, body, config, RAGGED, usize::MAX, Ending::Run)
        }
        _ => {
            let body = mistimed_body(Mode::Martin2, band_image(Mode::Martin2), 4_000.0);
            let length = body.0.len();
            reception(Mode::Martin2, body, live(length / 2), WHOLE, usize::MAX, Ending::Run)
        }
    };
    assert_eq!(print, DISTURBED[variant], "variant {variant}: {print:#018x}");
}

const DISTURBED: [u64; 8] = [
    0x68d4_ba6b_e3ea_8368,
    0x457e_748d_81f0_5024,
    0x430f_51a2_c161_cad8,
    0xffa4_49f6_678c_be4c,
    0xbabe_d33c_0253_746b,
    0x2a9a_4709_5a94_6306,
    0xb0d6_9386_cd91_eee6,
    0x2d5f_8f02_f482_520d,
];
