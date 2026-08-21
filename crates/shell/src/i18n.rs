use std::{borrow::Cow, fmt};

use fluent_bundle::{FluentArgs, FluentBundle, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

/// The Fluent sources one application supplies, one per [`Locale`].
///
/// Held by the application rather than by this crate: the messages name that
/// application's own controls, and a second one shares the machinery here
/// without sharing a word of the text. Written as a field per locale so
/// adding a language is a compile error at every catalogue until it is
/// answered, which is the point at which a missing translation is cheapest to
/// notice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Catalog {
    /// The English source.
    pub en: &'static str,
    /// The Japanese source.
    pub ja: &'static str,
}

impl Catalog {
    const fn source(&self, locale: Locale) -> &'static str {
        match locale {
            Locale::En => self.en,
            Locale::Ja => self.ja,
        }
    }
}

/// A language the interface can be drawn in.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Locale {
    /// English, the language the messages are authored in.
    #[default]
    En,
    /// Japanese.
    Ja,
}

impl Locale {
    /// Every language, in the order the interface offers them.
    pub const ALL: [Self; 2] = [Self::En, Self::Ja];

    /// Returns the language tag this is stored and looked up under.
    pub const fn tag(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Ja => "ja",
        }
    }

    /// Resolves a stored language tag, tolerating a hand-edited difference in
    /// case.
    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|locale| locale.tag().eq_ignore_ascii_case(tag))
    }

    fn identifier(self) -> LanguageIdentifier {
        self.tag().parse().expect("locale tag is well formed")
    }
}

impl fmt::Display for Locale {
    /// Language names are endonyms and are deliberately not translated.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::En => "English",
            Self::Ja => "日本語",
        })
    }
}

/// One application's messages, in one language.
pub struct I18n {
    locale: Locale,
    bundle: FluentBundle<FluentResource>,
}

impl fmt::Debug for I18n {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("I18n")
            .field("locale", &self.locale)
            .finish_non_exhaustive()
    }
}

impl I18n {
    /// Builds the lookup for `locale` over `catalog`.
    pub fn new(locale: Locale, catalog: &Catalog) -> Self {
        let resource =
            FluentResource::try_new(catalog.source(locale).to_owned()).expect("bundled locale resource parses");
        let mut bundle = FluentBundle::new(vec![locale.identifier()]);
        bundle.set_use_isolating(false);
        bundle
            .add_resource(resource)
            .expect("bundled locale resource has no conflicting messages");
        Self { locale, bundle }
    }

    /// Returns the language this is looking messages up in.
    pub const fn locale(&self) -> Locale {
        self.locale
    }

    /// Returns the message `key` names, or `key` itself if it has none.
    pub fn text(&self, key: &str) -> String {
        self.format(key, None)
    }

    /// Returns the message `key` names, or nothing when the catalogue has none.
    ///
    /// [`I18n::text`] answers with the key itself for a message that is not
    /// there, which is what a missing label should look like in the interface
    /// while it is being written. A caller naming keys it did not choose —
    /// one labelling whatever a configuration file asked for — needs to tell
    /// the two apart, and has something better than the key to fall back on.
    pub fn message(&self, key: &str) -> Option<String> {
        self.bundle.get_message(key)?.value()?;
        Some(self.text(key))
    }

    /// Returns the message `key` names, with `args` substituted into it.
    pub fn text_with(&self, key: &str, args: &[(&str, Value<'_>)]) -> String {
        let mut arguments = FluentArgs::new();
        for (name, value) in args {
            arguments.set(*name, value.clone());
        }
        self.format(key, Some(&arguments))
    }

    fn format(&self, key: &str, args: Option<&FluentArgs<'_>>) -> String {
        let Some(message) = self.bundle.get_message(key) else {
            return key.to_owned();
        };
        let Some(pattern) = message.value() else {
            return key.to_owned();
        };
        let mut errors = Vec::new();
        self.bundle.format_pattern(pattern, args, &mut errors).into_owned()
    }
}

/// A value passed to a message, built by [`arg`], [`owned`], or [`number`].
pub type Value<'a> = FluentValue<'a>;

/// Passes borrowed text to a message.
pub fn arg(value: &str) -> Value<'_> {
    Value::String(Cow::Borrowed(value))
}

/// Passes text a message has to own, such as a formatted error.
pub fn owned(value: String) -> Value<'static> {
    Value::String(Cow::Owned(value))
}

/// Passes a number to a message, formatted for the locale.
pub fn number(value: impl Into<f64>) -> Value<'static> {
    Value::from(value.into())
}
