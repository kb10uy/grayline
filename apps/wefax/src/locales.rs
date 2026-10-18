//! This application's own message catalogue.
//!
//! The lookup machinery is `grayline-shell`'s; the text is not. Every message
//! here names a control this application has.

use grayline_shell::i18n::Catalog;

pub const CATALOG: Catalog = Catalog {
    en: include_str!("../locales/en.ftl"),
    ja: include_str!("../locales/ja.ftl"),
};

#[cfg(test)]
mod tests {
    use grayline_shell::{
        check::{assert_icon_loads, assert_locales_agree, assert_requested_keys_defined, message_keys},
        i18n::{I18n, Locale, owned},
    };
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(Locale::En)]
    #[case(Locale::Ja)]
    fn resources_parse_and_resolve(#[case] locale: Locale) {
        let i18n = I18n::new(locale, &CATALOG);
        assert_eq!(i18n.locale(), locale);
        assert_ne!(i18n.text("state-idle"), "state-idle");
    }

    #[test]
    fn every_locale_defines_the_same_keys() {
        assert_locales_agree(&CATALOG);
    }

    #[test]
    fn arguments_are_substituted_without_isolation_marks() {
        let formatted = I18n::new(Locale::En, &CATALOG).text_with(
            "state-with-format",
            &[
                ("state", owned("IMAGING".to_owned())),
                ("format", owned("IOC 576".to_owned())),
            ],
        );
        assert_eq!(formatted, "IMAGING \u{b7} IOC 576");
    }

    #[test]
    fn missing_keys_fall_back_to_the_key() {
        assert_eq!(I18n::new(Locale::En, &CATALOG).text("no-such-key"), "no-such-key");
    }

    #[test]
    fn every_requested_key_is_defined() {
        assert_requested_keys_defined(
            &CATALOG,
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            20,
        );
    }

    /// Every label a menu or a readout is named by has to resolve.
    ///
    /// These reach the bundle through a `label_key` rather than by a literal
    /// at the call site, so the check above does not see them.
    #[test]
    fn every_label_key_is_defined() {
        let defined = message_keys(&CATALOG, Locale::default());
        let labels = crate::storage::paths::Folder::ALL
            .map(crate::storage::paths::Folder::label_key)
            .into_iter()
            .chain(
                [
                    crate::worker::receive::RxProgress::Idle,
                    crate::worker::receive::RxProgress::Starting,
                    crate::worker::receive::RxProgress::Phasing,
                    crate::worker::receive::RxProgress::Imaging { lines: 0 },
                    crate::worker::receive::RxProgress::Complete { lines: 0 },
                    crate::worker::receive::RxProgress::Stopped { lines: 0 },
                ]
                .map(crate::worker::receive::RxProgress::label_key),
            );
        for label in labels {
            assert!(defined.contains(label), "`{label}` is not defined");
        }
    }

    #[test]
    fn the_application_icon_loads() {
        assert_icon_loads(&crate::identity::IDENTITY);
    }
}
