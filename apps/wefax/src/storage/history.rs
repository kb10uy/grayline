use std::path::{Path, PathBuf};

use grayline_wefax::Format;
use jiff::Zoned;

/// Writes a received chart into the operator's own pictures directory.
///
/// The name carries the local time and the geometry, because a directory of
/// charts is browsed by a person: a file named for what it is and when it
/// arrived does not have to be opened to be identified.
pub fn save(directory: &Path, format: Format, width: usize, height: usize, gray: &[u8]) -> Result<PathBuf, String> {
    if width == 0 || height == 0 || gray.len() != width * height {
        return Err("the chart has no pixels to save".to_owned());
    }
    let path = directory.join(file_name(format, &Zoned::now()));
    let image = image::GrayImage::from_raw(width as u32, height as u32, gray.to_vec())
        .ok_or_else(|| "the chart dimensions are invalid".to_owned())?;
    image.save(&path).map_err(|error| error.to_string())?;
    Ok(path)
}

fn file_name(format: Format, at: &Zoned) -> String {
    format!(
        "{}-IOC{}-{}LPM.png",
        at.strftime("%Y%m%d-%H%M%S"),
        format.ioc.index(),
        format.lines_per_minute.as_lpm()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;
    use grayline_wefax::{Ioc, LinesPerMinute};

    #[test]
    fn a_chart_is_named_for_when_it_arrived_and_what_it_is() {
        let at: Zoned = "2026-08-10T09:07:05+09:00[Asia/Tokyo]".parse().unwrap();
        assert_eq!(file_name(Format::MARINE, &at), "20260810-090705-IOC576-120LPM.png");
        let narrow = Format {
            ioc: Ioc::Ioc288,
            lines_per_minute: LinesPerMinute::L240,
        };
        assert_eq!(file_name(narrow, &at), "20260810-090705-IOC288-240LPM.png");
    }

    #[test]
    fn a_chart_is_written_where_it_was_asked_for() {
        let root = TempDir::new();
        let gray: Vec<u8> = (0..24).map(|index| index as u8).collect();
        let path = save(root.path(), Format::MARINE, 8, 3, &gray).unwrap();
        assert!(path.is_file());
        assert!(path.starts_with(root.path()));
    }

    #[test]
    fn a_chart_with_no_pixels_is_refused() {
        let root = TempDir::new();
        assert!(save(root.path(), Format::MARINE, 0, 0, &[]).is_err());
        assert!(save(root.path(), Format::MARINE, 4, 2, &[0; 3]).is_err());
    }
}
