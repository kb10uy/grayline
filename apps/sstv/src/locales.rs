//! This application's own message catalogue.
//!
//! The lookup machinery is `grayline-shell`'s; the text is not. Every message
//! here names a control this application has, so a second application in the
//! family shares the former and none of the latter.

use grayline_shell::i18n::Catalog;

pub const CATALOG: Catalog = Catalog {
    en: include_str!("../locales/en.ftl"),
    ja: include_str!("../locales/ja.ftl"),
};

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use grayline_shell::i18n::{I18n, Locale, number};
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(Locale::En)]
    #[case(Locale::Ja)]
    fn resources_parse_and_resolve(#[case] locale: Locale) {
        let i18n = I18n::new(locale, &CATALOG);
        assert_eq!(i18n.locale(), locale);
        assert_ne!(i18n.text("tab-receive"), "tab-receive");
    }

    fn message_keys(locale: Locale) -> BTreeSet<String> {
        let source = match locale {
            Locale::En => CATALOG.en,
            Locale::Ja => CATALOG.ja,
        };
        source
            .lines()
            .filter_map(|line| line.split_once(" ="))
            .filter(|(key, _)| !key.is_empty() && !key.starts_with([' ', '#', '.', '*', '[']))
            .map(|(key, _)| key.to_owned())
            .collect()
    }

    #[test]
    fn every_locale_defines_the_same_keys() {
        let reference = message_keys(Locale::default());
        assert!(!reference.is_empty());
        for locale in Locale::ALL {
            assert_eq!(
                message_keys(locale),
                reference,
                "{locale:?} does not match the default locale"
            );
        }
    }

    #[test]
    fn arguments_are_substituted_without_isolation_marks() {
        let formatted = I18n::new(Locale::En, &CATALOG).text_with("state-receiving", &[("percent", number(94))]);
        assert_eq!(formatted, "RECEIVING \u{b7} 94%");
    }

    #[test]
    fn missing_keys_fall_back_to_the_key() {
        assert_eq!(I18n::new(Locale::En, &CATALOG).text("no-such-key"), "no-such-key");
    }

    /// Collects every key the application asks for by name.
    ///
    /// A key that is not defined is answered with the key itself, so a
    /// mistyped one reaches the operator as `station-titel` rather than as a
    /// failure. Nothing else notices, which is why this reads the call sites.
    fn requested_keys() -> BTreeSet<String> {
        fn walk(directory: &std::path::Path, keys: &mut BTreeSet<String>) {
            for entry in std::fs::read_dir(directory).expect("the source tree is readable") {
                let path = entry.expect("the source tree is readable").path();
                if path.is_dir() {
                    walk(&path, keys);
                } else if path.extension().is_some_and(|extension| extension == "rs")
                    // This module defines the catalogue rather than reaching
                    // it, and scanning it would find this scanner's own
                    // strings.
                    && path.file_name().is_some_and(|name| name != "locales.rs")
                {
                    collect(&std::fs::read_to_string(&path).expect("a source file"), keys);
                }
            }
        }

        // `text("key")` and `text_with("key", ...)`, which is every way the
        // bundle is reached, plus the `label_key` arms that answer with one.
        fn collect(source: &str, keys: &mut BTreeSet<String>) {
            for (index, _) in source.match_indices("text(").chain(source.match_indices("text_with(")) {
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

        let mut keys = BTreeSet::new();
        walk(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").as_path(),
            &mut keys,
        );
        keys
    }

    #[test]
    fn every_requested_key_is_defined() {
        let defined = message_keys(Locale::default());
        let requested = requested_keys();
        assert!(
            requested.len() > 50,
            "the call sites should have been found: {requested:?}"
        );
        let missing = requested.difference(&defined).collect::<Vec<_>>();
        assert!(missing.is_empty(), "keys asked for but not defined: {missing:?}");
    }

    /// Every label a menu or a control is named by has to resolve.
    ///
    /// These reach the bundle through a `label_key` rather than by a literal
    /// at the call site, so the check above does not see them.
    #[test]
    fn every_label_key_is_defined() {
        let defined = message_keys(Locale::default());
        let labels = crate::app::Tab::ALL
            .map(crate::app::Tab::label_key)
            .into_iter()
            .chain(crate::app::Dsp::ALL.map(crate::app::Dsp::label_key))
            .chain(crate::storage::history::HistoryFormat::ALL.map(crate::storage::history::HistoryFormat::label_key))
            .chain(crate::storage::paths::Folder::ALL.map(crate::storage::paths::Folder::label_key))
            .chain(
                [
                    crate::worker::rig::RigState::Disconnected,
                    crate::worker::rig::RigState::Connecting,
                    crate::worker::rig::RigState::Receiving,
                    crate::worker::rig::RigState::Transmitting,
                    crate::worker::rig::RigState::Failed,
                ]
                .map(crate::worker::rig::RigState::label_key),
            );
        for label in labels {
            assert!(defined.contains(label), "`{label}` is not defined");
        }
    }

    #[test]
    fn every_well_known_contact_key_is_labelled() {
        let defined = message_keys(Locale::default());
        for key in grayline_qso::WELL_KNOWN_KEYS {
            let label = crate::ui::view::contact_label_key(key);
            assert!(defined.contains(&label), "`{label}` is not defined");
        }
    }

    /// The carried copy has to stay decodable even where nothing reads it, so
    /// a platform that starts needing it is not surprised at runtime.
    ///
    /// Windows prefers the icon in the executable's resource section, so the
    /// check below does not reach this one there.
    #[test]
    fn the_carried_icon_decodes() {
        let icon = image::load_from_memory_with_format(crate::identity::IDENTITY.icon_png, image::ImageFormat::Png)
            .expect("the carried icon should decode")
            .into_rgba8();
        assert_eq!(icon.width(), icon.height());
    }

    #[test]
    fn the_application_icon_loads() {
        let icon = grayline_shell::platform::window_icon(&crate::identity::IDENTITY)
            .expect("the application icon should be available");
        assert!(icon.width > 0 && icon.height > 0);
        assert_eq!(icon.rgba.len(), icon.width as usize * icon.height as usize * 4);
        assert!(
            icon.rgba.as_chunks::<4>().0.iter().any(|pixel| pixel[3] != 0),
            "the icon should not be fully transparent"
        );
    }
}
