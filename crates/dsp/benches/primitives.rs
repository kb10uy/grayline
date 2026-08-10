use std::{f64::consts::TAU, hint::black_box};

use criterion::{Criterion, criterion_group, criterion_main};
use grayline_dsp::{
    detector::{ToneDetector, ToneDetectorDesign},
    filter::{Fir, FirDesign, FirKind},
    frequency::{HilbertDiscriminator, HilbertDiscriminatorDesign},
    oscillator::Vco,
    transform::{RealSpectrum, SpectrumWindow},
};

fn tone(sample_rate_hz: f64, frequency_hz: f64, seconds: f64) -> Vec<f64> {
    let count = (sample_rate_hz * seconds) as usize;
    (0..count)
        .map(|index| (TAU * frequency_hz * index as f64 / sample_rate_hz).sin())
        .collect()
}

/// The receive band-pass both RX front ends design for a given capture rate.
fn front_end_band_pass(sample_rate_hz: f64) -> Fir {
    let mut order = (24.0 * sample_rate_hz / 11_025.0).round() as usize;
    order = order.max(12);
    if !order.is_multiple_of(2) {
        order += 1;
    }
    Fir::from_design(FirDesign {
        kind: FirKind::BandPass,
        order,
        sample_rate_hz,
        lower_frequency_hz: 1_000.0,
        upper_frequency_hz: 2_600.0_f64.min(sample_rate_hz * 0.5 - 100.0),
        attenuation_db: 20.0,
        gain: 1.0,
    })
    .unwrap()
}

fn fir(c: &mut Criterion) {
    for rate in [11_025.0, 48_000.0] {
        let input = tone(rate, 1_900.0, 1.0);
        let mut filter = front_end_band_pass(rate);
        c.bench_function(&format!("fir_band_pass_1s_{}", rate as u32), |b| {
            b.iter(|| {
                let mut acc = 0.0;
                for &sample in &input {
                    acc += filter.process_sample(sample);
                }
                black_box(acc)
            })
        });
    }
}

fn hilbert(c: &mut Criterion) {
    for rate in [11_025.0, 48_000.0] {
        let input = tone(rate, 1_900.0, 1.0);
        let mut discriminator = HilbertDiscriminator::new(HilbertDiscriminatorDesign {
            sample_rate_hz: rate,
            minimum_hz: 0.0,
            maximum_hz: 3_000.0,
            output_cutoff_hz: 1_800.0,
            initial_hz: 1_900.0,
        })
        .unwrap();
        c.bench_function(&format!("hilbert_discriminator_1s_{}", rate as u32), |b| {
            b.iter(|| {
                let mut acc = 0.0;
                for &sample in &input {
                    acc += discriminator.process_sample(sample);
                }
                black_box(acc)
            })
        });
    }
}

fn tone_detector(c: &mut Criterion) {
    for rate in [11_025.0, 48_000.0] {
        let input = tone(rate, 1_200.0, 1.0);
        let mut detector = ToneDetector::new(ToneDetectorDesign {
            sample_rate_hz: rate,
            frequency_hz: 1_200.0,
            bandwidth_hz: 80.0,
            envelope_cutoff_hz: 50.0,
        })
        .unwrap();
        c.bench_function(&format!("tone_detector_1s_{}", rate as u32), |b| {
            b.iter(|| {
                let mut acc = 0.0;
                for &sample in &input {
                    acc += detector.process_sample(sample);
                }
                black_box(acc)
            })
        });
    }
}

fn vco(c: &mut Criterion) {
    let rate = 48_000.0;
    let mut oscillator = Vco::new(rate, 1_900.0, 0.0).unwrap();
    let count = rate as usize;
    c.bench_function("vco_1s_48000", |b| {
        b.iter(|| {
            let mut acc = 0.0;
            for _ in 0..count {
                acc += oscillator.process_sample(0.0).unwrap();
            }
            black_box(acc)
        })
    });
}

fn spectrum(c: &mut Criterion) {
    let length = 2_048;
    let input = tone(48_000.0, 1_900.0, length as f64 / 48_000.0);
    let mut analyzer = RealSpectrum::new(length, SpectrumWindow::Hann).unwrap();
    c.bench_function("real_spectrum_2048", |b| {
        b.iter(|| {
            let bins = analyzer.transform(&input).unwrap();
            black_box(bins[bins.len() / 2])
        })
    });
}

criterion_group!(benches, fir, hilbert, tone_detector, vco, spectrum);
criterion_main!(benches);
