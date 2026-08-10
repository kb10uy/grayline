use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use grayline_sstv::{
    TxEncoder,
    image::{ImageSize, Rgb8, RgbImage},
    mode::Mode,
    rx::{DemodulatedBlock, RxConfig, RxDecoder, RxOutcome, RxState, SlantEstimator, Staging, SyncObservation},
};

const SAMPLE_RATE_HZ: u32 = 48_000;
const BLOCK_SIZE: usize = 1_024;

fn gradient_image(mode: Mode) -> RgbImage {
    let width = mode.spec().width() as usize;
    let height = mode.spec().height() as usize;
    let size = ImageSize::new(width, height).unwrap();
    let pixels = (0..width * height)
        .map(|index| {
            let x = index % width;
            let y = index / width;
            Rgb8::new(
                (x * 255 / width) as u8,
                (y * 255 / height) as u8,
                ((x ^ y) & 0xff) as u8,
            )
        })
        .collect();
    RgbImage::from_pixels(size, pixels).unwrap()
}

/// An ideal demodulated stream: tone frequencies held over their timing, with
/// full sync confidence during the 1200 Hz pulses, as an aligned front end
/// would report them.
fn demodulated_stream(mode: Mode) -> (Vec<f32>, Vec<f32>) {
    let mut frequency = Vec::new();
    let mut sync = Vec::new();
    for tone in TxEncoder::new(mode, gradient_image(mode)).unwrap() {
        let deadline = tone.until().to_samples(SAMPLE_RATE_HZ);
        let hz = tone.frequency().as_hz() as f32;
        let strength = if (hz - 1_200.0).abs() < 50.0 { 1.0 } else { 0.0 };
        while (frequency.len() as u64) < deadline {
            frequency.push(hz);
            sync.push(strength);
        }
    }
    frequency.resize(frequency.len() + SAMPLE_RATE_HZ as usize, 1_900.0);
    sync.resize(frequency.len(), 0.0);
    (frequency, sync)
}

fn decode_stream(mode: Mode, frequency: &[f32], sync: &[f32], live_slant: bool) -> RxOutcome {
    let config = RxConfig {
        live_sync: true,
        live_slant,
        staging: if live_slant {
            Staging::Memory {
                max_samples: frequency.len(),
            }
        } else {
            Staging::Disabled
        },
        ..RxConfig::default()
    };
    let mut decoder = RxDecoder::with_config(mode, SAMPLE_RATE_HZ, config).unwrap();
    let mut offset = 0;
    while offset < frequency.len() {
        if matches!(decoder.state(), RxState::Complete | RxState::Stopped { .. }) {
            break;
        }
        let end = (offset + BLOCK_SIZE).min(frequency.len());
        let mut inner = offset;
        while inner < end {
            let block = DemodulatedBlock::new(inner as u64, &frequency[inner..end], &sync[inner..end]);
            let result = decoder.process(block).unwrap();
            if result.consumed() == 0 && result.event().is_none() {
                panic!("decoder stalled at sample {inner}");
            }
            inner += result.consumed();
            if matches!(decoder.state(), RxState::Complete | RxState::Stopped { .. }) {
                break;
            }
        }
        offset = end.max(inner);
    }
    decoder.finish()
}

fn rx_decoder(c: &mut Criterion) {
    let mode = Mode::Robot36;
    let (frequency, sync) = demodulated_stream(mode);
    assert!(matches!(
        decode_stream(mode, &frequency, &sync, false),
        RxOutcome::Complete(_)
    ));
    let mut group = c.benchmark_group("sstv_rx_decoder");
    group.sample_size(20);
    for live_slant in [false, true] {
        let name = if live_slant { "robot36_live_slant" } else { "robot36" };
        group.bench_function(name, |b| {
            b.iter(|| black_box(decode_stream(mode, &frequency, &sync, live_slant)))
        });
    }
    group.finish();
}

fn slant(c: &mut Criterion) {
    let mode = Mode::Martin1;
    let estimator = SlantEstimator::for_mode(SAMPLE_RATE_HZ, mode).unwrap();
    let samples_per_line = mode.spec().period().to_samples(SAMPLE_RATE_HZ);
    for count in [64_usize, 256] {
        let observations: Vec<_> = (0..count)
            .map(|unit| {
                let center = unit as u64 * samples_per_line + (unit % 5) as u64;
                SyncObservation {
                    unit,
                    peak_sample: center + 40,
                    center_sample: center,
                    confidence: 0.9,
                    contrast: 0.5,
                }
            })
            .collect();
        c.bench_function(&format!("slant_estimate_{count}"), |b| {
            b.iter(|| black_box(estimator.estimate(&observations)))
        });
    }
}

fn tx_encode(c: &mut Criterion) {
    for mode in [Mode::Robot36, Mode::Scottie1] {
        let image = gradient_image(mode);
        let name = format!("tx_encode_{}", mode.spec().name().replace(' ', "_").to_lowercase());
        c.bench_function(&name, |b| {
            b.iter(|| {
                let mut count = 0_u64;
                for tone in TxEncoder::new(mode, image.clone()).unwrap() {
                    count += tone.frequency().as_hz() as u64;
                }
                black_box(count)
            })
        });
    }
}

criterion_group!(benches, rx_decoder, slant, tx_encode);
criterion_main!(benches);
