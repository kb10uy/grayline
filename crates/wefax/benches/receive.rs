use std::{f32::consts::TAU, hint::black_box};

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use grayline_wefax::{
    Format,
    format::PHASING_WHITE_FRACTION,
    rx::{DemodulatedBlock, Demodulator, WefaxDecoder},
};

const SAMPLE_RATE_HZ: u32 = 48_000;
const BLOCK_SIZE: usize = 1_024;

fn tone(sample_rate_hz: u32, frequency_hz: f32, seconds: f32) -> Vec<f32> {
    let count = (sample_rate_hz as f32 * seconds) as usize;
    (0..count)
        .map(|index| (TAU * frequency_hz * index as f32 / sample_rate_hz as f32).sin())
        .collect()
}

/// A demodulated reception: phasing lines, then alternating two-pixel bars.
fn reception_frequencies(format: Format, phasing_seconds: f64, picture_seconds: f64) -> Vec<f32> {
    let samples_per_line = format.samples_per_line(SAMPLE_RATE_HZ);
    let samples_per_pixel = format.samples_per_pixel(SAMPLE_RATE_HZ);
    let phasing_count = (f64::from(SAMPLE_RATE_HZ) * phasing_seconds) as usize;
    let picture_count = (f64::from(SAMPLE_RATE_HZ) * picture_seconds) as usize;
    let mut frequencies = Vec::with_capacity(phasing_count + picture_count);
    for index in 0..phasing_count {
        let position = index as f64 / samples_per_line;
        let in_pulse = position.fract() < PHASING_WHITE_FRACTION;
        frequencies.push(if in_pulse { 2_300.0 } else { 1_500.0 });
    }
    for index in 0..picture_count {
        let pixel = (index as f64 / samples_per_pixel) as u64;
        frequencies.push(if pixel % 4 < 2 { 1_500.0 } else { 2_300.0 });
    }
    frequencies
}

fn decode(frequency: &[f32], format: Format) -> (usize, WefaxDecoder) {
    let mut decoder = WefaxDecoder::new(SAMPLE_RATE_HZ).unwrap();
    decoder.set_auto_start(false);
    decoder.set_auto_stop(false);
    decoder.start_manually(format, 0).unwrap();
    let mut offset = 0;
    while offset < frequency.len() {
        if decoder.state().is_terminal() {
            break;
        }
        let end = (offset + BLOCK_SIZE).min(frequency.len());
        let mut inner = offset;
        while inner < end {
            let block = DemodulatedBlock::new(inner as u64, &frequency[inner..end]).unwrap();
            let result = decoder.process(block).unwrap();
            if result.consumed == 0 {
                panic!("decoder stalled at sample {inner}");
            }
            inner += result.consumed;
            if decoder.state().is_terminal() {
                break;
            }
        }
        offset = end.max(inner);
    }
    (decoder.line_count(), decoder)
}

fn demodulator(c: &mut Criterion) {
    for rate in [11_025_u32, 48_000] {
        let input = tone(rate, 1_900.0, 5.0);
        c.bench_function(&format!("wefax_demodulator_5s_{rate}"), |b| {
            b.iter_batched(
                || Demodulator::new(rate).unwrap(),
                |mut demodulator| {
                    for packet in input.chunks(1_024) {
                        black_box(demodulator.process(packet).unwrap());
                    }
                },
                BatchSize::LargeInput,
            )
        });
    }
}

fn decoder(c: &mut Criterion) {
    let format = Format::MARINE;
    let frequency = reception_frequencies(format, 10.0, 60.0);
    let (lines, probe) = decode(&frequency, format);
    assert!(lines >= 100, "only {lines} lines decoded, state {:?}", probe.state());
    let mut group = c.benchmark_group("wefax_decoder");
    group.sample_size(10);
    group.bench_function("marine_70s_48k", |b| b.iter(|| black_box(decode(&frequency, format).0)));
    group.bench_function("marine_70s_48k_finish", |b| {
        b.iter_batched(
            || decode(&frequency, format).1,
            |decoder| black_box(decoder.finish()),
            BatchSize::LargeInput,
        )
    });
    group.finish();
}

criterion_group!(benches, demodulator, decoder);
criterion_main!(benches);
