use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use grayline_sstv::{
    TxEncoder,
    image::{ImageSize, Rgb8, RgbImage},
    mode::Mode,
};
use grayline_tone_tx::Modulator;

const SAMPLE_RATE_HZ: u32 = 48_000;

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

fn modulate(c: &mut Criterion) {
    let mode = Mode::Robot36;
    let image = gradient_image(mode);
    let mut group = c.benchmark_group("tone_tx");
    group.sample_size(20);
    group.bench_function("modulator_robot36_48k", |b| {
        b.iter(|| {
            let encoder = TxEncoder::new(mode, image.clone()).unwrap();
            let mut modulator = Modulator::new(encoder, SAMPLE_RATE_HZ).unwrap();
            let mut block = [0.0_f32; 1_024];
            let mut total = 0_usize;
            loop {
                let count = modulator.process(&mut block).unwrap();
                if count == 0 {
                    break;
                }
                total += count;
            }
            black_box(total)
        })
    });
    group.finish();
}

criterion_group!(benches, modulate);
criterion_main!(benches);
