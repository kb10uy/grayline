use std::{collections::BTreeSet, fs, path::Path};

use crate::{
    Identity,
    i18n::{Catalog, Locale},
    platform,
};

/// Returns every message key `catalog` defines in `locale`.
pub fn message_keys(catalog: &Catalog, locale: Locale) -> BTreeSet<String> {
    let source = match locale {
        Locale::En => catalog.en,
        Locale::Ja => catalog.ja,
    };
    source
        .lines()
        .filter_map(|line| line.split_once(" ="))
        .filter(|(key, _)| !key.is_empty() && !key.starts_with([' ', '#', '.', '*', '[']))
        .map(|(key, _)| key.to_owned())
        .collect()
}

/// Panics unless every locale of `catalog` defines the same keys.
pub fn assert_locales_agree(catalog: &Catalog) {
    let reference = message_keys(catalog, Locale::default());
    assert!(!reference.is_empty());
    for locale in Locale::ALL {
        assert_eq!(
            message_keys(catalog, locale),
            reference,
            "{locale:?} does not match the default locale"
        );
    }
}

/// Collects every key the sources under `directory` ask for by name.
///
/// A key that is not defined is answered with the key itself, so a mistyped
/// one reaches the operator as the key rather than as a failure. Nothing else
/// notices, which is why this reads the call sites. A file named `locales.rs`
/// defines the catalogue rather than reaching it, and is skipped.
pub fn requested_keys(directory: &Path) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    walk(directory, &mut keys);
    keys
}

/// Panics unless every key the sources under `directory` ask for is defined.
///
/// `minimum` guards the scanner itself: finding fewer call sites than that
/// means it stopped recognizing them, not that the application stopped
/// asking.
pub fn assert_requested_keys_defined(catalog: &Catalog, directory: &Path, minimum: usize) {
    let defined = message_keys(catalog, Locale::default());
    let requested = requested_keys(directory);
    assert!(
        requested.len() > minimum,
        "the call sites should have been found: {requested:?}"
    );
    let missing = requested.difference(&defined).collect::<Vec<_>>();
    assert!(missing.is_empty(), "keys asked for but not defined: {missing:?}");
}

/// Panics unless the icon `identity` carries decodes and the window gets one.
///
/// The carried copy has to stay decodable even where nothing reads it, so a
/// platform that starts needing it is not surprised at runtime. Windows
/// prefers the icon in the executable's resource section, so the window icon
/// does not come from the carried copy there.
pub fn assert_icon_loads(identity: &Identity) {
    let carried = image::load_from_memory_with_format(identity.icon_png, image::ImageFormat::Png)
        .expect("the carried icon should decode")
        .into_rgba8();
    assert_eq!(carried.width(), carried.height());

    let icon = platform::window_icon(identity).expect("the application icon should be available");
    assert!(icon.width > 0 && icon.height > 0);
    assert_eq!(icon.rgba.len(), icon.width as usize * icon.height as usize * 4);
    assert!(
        icon.rgba.as_chunks::<4>().0.iter().any(|pixel| pixel[3] != 0),
        "the icon should not be fully transparent"
    );
}

fn walk(directory: &Path, keys: &mut BTreeSet<String>) {
    for entry in fs::read_dir(directory).expect("the source tree is readable") {
        let path = entry.expect("the source tree is readable").path();
        if path.is_dir() {
            walk(&path, keys);
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && path.file_name().is_some_and(|name| name != "locales.rs")
        {
            collect(&fs::read_to_string(&path).expect("a source file"), keys);
        }
    }
}

fn collect(source: &str, keys: &mut BTreeSet<String>) {
    for (index, _) in source.match_indices("text(").chain(source.match_indices("text_with(")) {
        if source[..index]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_alphanumeric() || character == '_')
        {
            continue;
        }
        let rest = &source[index..];
        let Some(open) = rest.find('(') else { continue };
        let Some(quoted) = rest[open + 1..].strip_prefix('"') else {
            continue;
        };
        let Some(end) = quoted.find('"') else {
            continue;
        };
        keys.insert(quoted[..end].to_owned());
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(r#"i18n.text("menu-file")"#, &["menu-file"])]
    #[case(r#"i18n.text_with("status-audio", &[("rate", number(rate))])"#, &["status-audio"])]
    #[case(r#"encode_text("RY", &config)"#, &[])]
    #[case(r#"i18n.text(key)"#, &[])]
    #[case(r#"a.text("one"); b.text_with("two", &[])"#, &["one", "two"])]
    fn call_sites_are_read_by_what_precedes_the_name(#[case] source: &str, #[case] expected: &[&str]) {
        let mut keys = BTreeSet::new();
        collect(source, &mut keys);
        assert_eq!(keys, expected.iter().map(|key| (*key).to_owned()).collect());
    }

    #[test]
    fn message_keys_skip_attributes_comments_and_continuations() {
        let catalog = Catalog {
            en: "# a comment\nfirst = First\n    .tooltip = Hint\nsecond =\n    Continued\n",
            ja: "first = 一\nsecond = 二\n",
        };
        assert_eq!(
            message_keys(&catalog, Locale::En),
            ["first".to_owned(), "second".to_owned()].into()
        );
        assert_locales_agree(&catalog);
    }
}
