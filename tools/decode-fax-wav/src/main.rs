use std::{env, ffi::OsString, path::PathBuf, process::ExitCode};

use anyhow::{Result, bail};
use grayline_decode_fax_wav::{DecodeOptions, DecodeStatus, decode_file_with_options};
use grayline_wefax::{Format, Ioc, LinesPerMinute, WefaxBand};

fn main() -> ExitCode {
    match run() {
        Ok(DecodeStatus::Complete) => ExitCode::SUCCESS,
        Ok(status) => {
            eprintln!("warning: saved a partial image ({status:?})");
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<DecodeStatus> {
    let mut args = env::args_os().skip(1);
    let mut options = DecodeOptions::default();
    let mut ioc = None;
    let mut lines_per_minute = None;
    let mut positional = Vec::with_capacity(2);
    while let Some(argument) = args.next() {
        match argument.to_string_lossy().as_ref() {
            "--packet-size" => options.pcm_packet_size = number(args.next(), "--packet-size")?,
            "--ioc" => {
                ioc = Some(match number::<u32>(args.next(), "--ioc")? {
                    576 => Ioc::Ioc576,
                    288 => Ioc::Ioc288,
                    other => bail!("--ioc must be 576 or 288, not {other}"),
                });
            }
            "--lpm" => {
                lines_per_minute = Some(match number::<u32>(args.next(), "--lpm")? {
                    60 => LinesPerMinute::L60,
                    90 => LinesPerMinute::L90,
                    100 => LinesPerMinute::L100,
                    120 => LinesPerMinute::L120,
                    180 => LinesPerMinute::L180,
                    240 => LinesPerMinute::L240,
                    other => bail!("--lpm must be 60, 90, 100, 120, 180 or 240, not {other}"),
                });
            }
            "--narrow" => options.config.band = WefaxBand::NARROW,
            "--invert" => options.config.inverted = true,
            "--no-auto-start" => options.config.auto_start = false,
            "--no-auto-stop" => options.config.auto_stop = false,
            "--no-slant" => options.config.slant_tracking = false,
            "--phase-shift" => {
                options.config.phase_offset_fraction = number::<f64>(args.next(), "--phase-shift")?;
            }
            "--max-minutes" => {
                let minutes = number::<f64>(args.next(), "--max-minutes")?;
                options.config.max_lines = Some((minutes * 240.0).ceil() as usize);
            }
            other if other.starts_with("--") => return usage(),
            _ => positional.push(argument),
        }
    }

    // The start tone announces the index of cooperation and nothing else, so
    // naming a line rate is what lets a decode skip the tone entirely.
    match (ioc, lines_per_minute) {
        (Some(ioc), Some(lines_per_minute)) => {
            options.config.format = Some(Format { ioc, lines_per_minute });
            options.config.infer_lines_per_minute = false;
            options.config.auto_start = false;
        }
        (Some(_), None) | (None, Some(_)) => {
            bail!("--ioc and --lpm have to be given together, because a start tone names only the index")
        }
        (None, None) => {}
    }
    if !options.config.auto_start && options.config.format.is_none() {
        bail!("--no-auto-start needs --ioc and --lpm, because nothing else would name the geometry");
    }

    let [input, output] = positional.as_slice() else {
        return usage();
    };
    let report = decode_file_with_options(&PathBuf::from(input), &PathBuf::from(output), options)?;
    match report.format {
        Some(format) => println!(
            "format: IOC {}, {} LPM",
            format.ioc.index(),
            format.lines_per_minute.as_lpm()
        ),
        None => println!("format: not identified"),
    }
    println!("lines: {}", report.lines);
    if let (Some(samples), Some(error_ppm)) = (report.samples_per_line, report.error_ppm) {
        println!("line rate: {samples:.1} samples ({error_ppm:+.0} ppm)");
    }
    if let (Some(offset), Some(contrast)) = (report.phasing_offset_pixels, report.phasing_contrast) {
        println!("phasing: pulse at {offset:.0} px, contrast {contrast:.2}");
    }
    Ok(report.status)
}

fn number<T: std::str::FromStr>(argument: Option<OsString>, flag: &str) -> Result<T> {
    argument
        .as_ref()
        .and_then(|value| value.to_str())
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("{flag} needs a value"))
}

fn usage<T>() -> Result<T> {
    bail!(
        "usage: decode-fax-wav [--packet-size SAMPLES] [--ioc 576|288] [--lpm 60|90|100|120|180|240]\n\
         \x20                     [--narrow] [--invert] [--no-auto-start] [--no-auto-stop] [--no-slant]\n\
         \x20                     [--phase-shift FRACTION] [--max-minutes N] <INPUT.wav> <OUTPUT_IMAGE>"
    )
}
