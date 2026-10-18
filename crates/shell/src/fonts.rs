use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily};

use crate::{
    log,
    platform::{MONOSPACE_FONTS, UI_FONTS},
};

/// Draws the interface with the platform's UI font, and monospaced text with
/// its coding font.
///
/// The system families are installed ahead of the bundled fonts, so Latin and
/// Japanese come from one face, while a machine with none of them installed
/// still renders text.
///
/// The font database is queried directly because the face index matters:
/// Windows ships `Yu Gothic UI` as the second face of a collection whose first
/// face is `Yu Gothic`, and that first face carries a half-em line gap.
/// Loading it would leave every label sitting high in its row.
pub fn install(ctx: &egui::Context) {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    ctx.set_fonts(definitions(&database));
}

fn definitions(database: &fontdb::Database) -> FontDefinitions {
    let mut definitions = FontDefinitions::default();
    let ui = install_faces(&mut definitions, database, UI_FONTS);
    let monospace = install_faces(&mut definitions, database, MONOSPACE_FONTS);

    if ui.is_empty() {
        log::note("no system UI font matched; using the bundled fonts");
    }
    if monospace.is_empty() {
        log::note("no system monospaced font matched; using the bundled fonts");
    }

    prefer(&mut definitions, FontFamily::Proportional, &ui);
    // The UI face is put in the monospaced family first, so that the coding
    // font then lands in front of it: a coding font is drawn for program text
    // and need not carry Japanese, and monospaced text still has to show it.
    prefer(&mut definitions, FontFamily::Monospace, &ui);
    prefer(&mut definitions, FontFamily::Monospace, &monospace);
    definitions
}

fn install_faces(definitions: &mut FontDefinitions, database: &fontdb::Database, families: &[&str]) -> Vec<String> {
    let mut installed = Vec::new();
    for family in families {
        let Some((data, index)) = load_face(database, family) else {
            continue;
        };
        definitions.font_data.insert(
            (*family).to_owned(),
            Arc::new(FontData {
                font: data.into(),
                index,
                tweak: egui::FontTweak::default(),
            }),
        );
        installed.push((*family).to_owned());
    }
    installed
}

fn prefer(definitions: &mut FontDefinitions, target: FontFamily, families: &[String]) {
    let installed = definitions.families.entry(target).or_default();
    for family in families.iter().rev() {
        installed.insert(0, family.clone());
    }
}

fn load_face(database: &fontdb::Database, family: &str) -> Option<(Vec<u8>, u32)> {
    let id = database.query(&fontdb::Query {
        families: &[fontdb::Family::Name(family)],
        weight: fontdb::Weight::NORMAL,
        stretch: fontdb::Stretch::Normal,
        style: fontdb::Style::Normal,
    })?;
    let (source, index) = database.face_source(id)?;
    let data = match source {
        fontdb::Source::File(path) => std::fs::read(path).ok()?,
        fontdb::Source::Binary(data) => data.as_ref().as_ref().to_vec(),
        #[allow(unreachable_patterns)]
        _ => return None,
    };
    Some((data, index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_database_leaves_the_bundled_fonts_usable() {
        let definitions = definitions(&fontdb::Database::new());
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            assert!(!definitions.families[&family].is_empty());
        }
    }

    fn first_available(database: &fontdb::Database, families: &[&str]) -> Option<String> {
        families
            .iter()
            .find(|family| load_face(database, family).is_some())
            .map(|family| (*family).to_owned())
    }

    fn system_database() -> fontdb::Database {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        database
    }

    #[test]
    fn a_matched_family_is_installed_ahead_of_the_bundled_fonts() {
        let database = system_database();
        let Some(wanted) = first_available(&database, UI_FONTS) else {
            return;
        };

        let definitions = definitions(&database);
        let bundled = FontDefinitions::default();
        let installed = &definitions.families[&FontFamily::Proportional];
        assert_eq!(installed.first(), Some(&wanted));
        assert!(
            installed.len() > bundled.families[&FontFamily::Proportional].len(),
            "the bundled fonts should still be behind the system ones"
        );
    }

    #[test]
    fn the_monospaced_family_is_led_by_a_coding_font() {
        let database = system_database();
        let Some(wanted) = first_available(&database, MONOSPACE_FONTS) else {
            return;
        };

        let definitions = definitions(&database);
        let installed = &definitions.families[&FontFamily::Monospace];
        assert_eq!(installed.first(), Some(&wanted));

        if let Some(ui) = first_available(&database, UI_FONTS) {
            assert!(
                installed.iter().position(|family| *family == ui) > Some(0),
                "the UI face should sit behind the coding font: {installed:?}"
            );
        }
    }

    #[test]
    fn the_proportional_family_is_not_led_by_a_coding_font() {
        let database = system_database();
        let Some(wanted) = first_available(&database, MONOSPACE_FONTS) else {
            return;
        };
        if first_available(&database, UI_FONTS).is_none() {
            return;
        }

        let definitions = definitions(&database);
        let installed = &definitions.families[&FontFamily::Proportional];
        assert!(
            !installed.contains(&wanted),
            "the coding font has no business in the proportional family: {installed:?}"
        );
    }
}
