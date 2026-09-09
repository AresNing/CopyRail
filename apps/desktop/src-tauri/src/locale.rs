//! Process-local language for native controls; persisted in desktop preferences.
use paste_domain::Language;
use std::sync::atomic::{AtomicBool, Ordering};
static ENGLISH: AtomicBool = AtomicBool::new(false);
pub fn set_language(language: Language) {
    ENGLISH.store(language == Language::English, Ordering::Relaxed);
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
