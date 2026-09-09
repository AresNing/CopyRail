use leptos::prelude::*;
use paste_domain::Language;

thread_local! {
    // One webview owns one App. Keep the same signal across component callbacks
    // so switching language updates text without remounting editors or cards.
    static LANGUAGE: std::cell::Cell<Option<RwSignal<Language>>> = const { std::cell::Cell::new(None) };
}

pub fn init() {
    LANGUAGE.with(|value| value.set(Some(RwSignal::new(Language::default()))));
}
pub fn language() -> Language {
    LANGUAGE.with(|value| value.get().map(|signal| signal.get()).unwrap_or_default())
}
pub fn set_language(language: Language) {
    LANGUAGE.with(|value| {
        if let Some(signal) = value.get() {
            signal.set(language);
        }
    });
    if let Some(document) = web_sys::window().and_then(|window| window.document())
        && let Some(root) = document.document_element()
    {
        let _ = root.set_attribute("lang", language.code());
    }
}
pub fn t(source: &str) -> &str {
    paste_domain::i18n::translate(language(), source)
}

macro_rules! localized_format {
    ($zh:literal, $en:literal $(, $($args:tt)*)?) => {
        if crate::i18n::language() == paste_domain::Language::English {
            format!($en $(, $($args)*)?)
        } else { format!($zh $(, $($args)*)?) }
    };
}
pub(crate) use localized_format;
