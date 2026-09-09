//! Resolve macOS preferred language, not a locale inferred from region or shell.
use paste_domain::{Language, LanguagePreference, LanguageSettings};
use std::sync::atomic::{AtomicBool, Ordering};
static ENGLISH: AtomicBool = AtomicBool::new(false);

pub fn system_language() -> String {
    #[cfg(target_os = "macos")]
    {
        objc2_foundation::NSLocale::preferredLanguages()
            .firstObject()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "en".into())
    }
    #[cfg(not(target_os = "macos"))]
    {
        "en".into()
    }
}

pub fn set_language(preference: LanguagePreference) -> LanguageSettings {
    let effective = preference.resolve(&system_language());
    ENGLISH.store(effective == Language::English, Ordering::Relaxed);
    LanguageSettings {
        preference,
        effective,
    }
}
pub fn language() -> Language {
    if ENGLISH.load(Ordering::Relaxed) {
        Language::English
    } else {
        Language::Chinese
    }
}
pub fn t(source: &str) -> &str {
    paste_domain::i18n::translate(language(), source)
}
