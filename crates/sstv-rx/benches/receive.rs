use std::{f32::consts::TAU, hint::black_box};

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use grayline_sstv::{
    TxEncoder,
    image::{ImageSize, Rgb8, RgbImage},
    mode::Mode,
    rx::RxOutcome,
};
use grayline_sstv_rx::{Demodulator, PipelineOptions, ReceivePipeline};
use grayline_tone_tx::Modulator;

fn tone(sample_rate_hz: u32, frequency_hz: f32, seconds: f32) -> Vec<f32> {
    let count = (sample_rate_hz as f32 * seconds) as usize;
    (0..count)
        .map(|index| (TAU * frequency_hz * index as f32 / sample_rate_hz as f32).sin())
        .collect()
}

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

fn transmission_pcm(mode: Mode, sample_rate_hz: u32) -> Vec<f32> {
    let encoder = TxEncoder::new(mode, gradient_image(mode)).unwrap();
    let mut modulator = Modulator::new(encoder, sample_rate_hz).unwrap();
    let mut block = [0.0_f32; 1_024];
    let mut samples = Vec::new();
    loop {
        let count = modulator.process(&mut block).unwrap();
        if count == 0 {
            break;
        }
        samples.extend_from_slice(&block[..count]);
    }
    samples.extend(tone(sample_rate_hz, 1_900.0, 1.0));
    samples
}

fn decode(pcm: &[f32], sample_rate_hz: u32, live_slant: bool) -> RxOutcome {
    let mut pipeline = ReceivePipeline::new(
        sample_rate_hz,
        PipelineOptions {
            live_slant,
            staging_max_samples: pcm.len(),
        },
    )
    .unwrap();
    for packet in pcm.chunks(1_024) {
        pipeline.process(packet, |_| {}).unwrap();
    }
    pipeline.finish().unwrap().outcome
}

fn demodulator(c: &mut Criterion) {
    for rate in [11_025_u32, 48_000] {
        let input = tone(rate, 1_900.0, 5.0);
        c.bench_function(&format!("sstv_demodulator_5s_{rate}"), |b| {
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

fn pipeline(c: &mut Criterion) {
    let rate = 48_000;
    let pcm = transmission_pcm(Mode::Robot36, rate);
    assert!(matches!(decode(&pcm, rate, true), RxOutcome::Complete(_)));
    let mut group = c.benchmark_group("sstv_pipeline");
    group.sample_size(10);
    for live_slant in [false, true] {
        let name = if live_slant {
            "robot36_48k_live_slant"
        } else {
            "robot36_48k"
        };
        group.bench_function(name, |b| b.iter(|| black_box(decode(&pcm, rate, live_slant))));
    }
    group.finish();
}

criterion_group!(benches, demodulator, pipeline);
criterion_main!(benches);
