use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use grayline_rtty::{ReceivePipeline, RxConfig, Transmitter, TxConfig, encode_text};

fn one_second_of_text(rate: u32) -> Vec<f32> {
    let config = TxConfig::default();
    let codes = encode_text("CQ CQ DE JL1HIS", &config).unwrap();
    let mut transmitter = Transmitter::new(codes.into_iter(), rate, config).unwrap();
    let mut samples = Vec::new();
    loop {
        let mut block = [0.0; 1_024];
        let count = transmitter.process(&mut block).unwrap();
        if count == 0 {
            break;
        }
        samples.extend_from_slice(&block[..count]);
    }
    samples.truncate(rate as usize);
    samples
}

fn receive(c: &mut Criterion) {
    for rate in [11_025_u32, 48_000] {
        let samples = one_second_of_text(rate);
        c.bench_function(&format!("receive_pipeline_1s_{rate}"), |b| {
            b.iter(|| {
                let mut pipeline = ReceivePipeline::new(rate, RxConfig::default()).unwrap();
                let mut characters = 0;
                for chunk in samples.chunks(1_024) {
                    pipeline.process(chunk, |_| characters += 1).unwrap();
                }
                black_box(characters)
            })
        });
    }
}

fn transmit(c: &mut Criterion) {
    for rate in [11_025_u32, 48_000] {
        let config = TxConfig::default();
        let codes = encode_text("CQ CQ DE JL1HIS", &config).unwrap();
        c.bench_function(&format!("transmitter_1s_{rate}"), |b| {
            b.iter(|| {
                let mut transmitter = Transmitter::new(codes.clone().into_iter(), rate, config).unwrap();
                let mut written = 0;
                let mut block = [0.0; 1_024];
                while written < rate as usize {
                    let count = transmitter.process(&mut block).unwrap();
                    if count == 0 {
                        break;
                    }
                    written += count;
                }
                black_box(written)
            })
        });
    }
}

criterion_group!(benches, receive, transmit);
criterion_main!(benches);
