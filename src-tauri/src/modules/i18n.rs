/// Choose the first supported OS language when creating a new configuration.
/// Saved configurations keep their explicit language selection.
pub fn default_language() -> String {
    language_from_locales(sys_locale::get_locales()).to_string()
}

fn language_from_locales(locales: impl IntoIterator<Item = impl AsRef<str>>) -> &'static str {
    locales
        .into_iter()
        .find_map(|locale| supported_language(locale.as_ref()))
        .unwrap_or("en")
}

/// 将系统语言标签规范化为 Web 面板支持的语言标识。
fn supported_language(locale: &str) -> Option<&'static str> {
    let locale = locale
        .split(['.', '@'])
        .next()?
        .replace('_', "-")
        .to_ascii_lowercase();
    let mut subtags = locale.split('-');
    match subtags.next()? {
        "zh" => Some(match subtags.next() {
            // An explicit script takes precedence over the region (zh-Hans-TW).
            Some("hant" | "tw" | "hk" | "mo") => "zh-TW",
            _ => "zh",
        }),
        "en" => Some("en"),
        "ja" => Some("ja"),
        "tr" => Some("tr"),
        "vi" => Some("vi"),
        "pt" => Some("pt"),
        "ru" => Some("ru"),
        "ko" => Some("ko"),
        "ar" => Some("ar"),
        "es" => Some("es"),
        // The existing Malay translation uses the application's legacy "my" key.
        "ms" => Some("my"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::language_from_locales;

    #[test]
    fn detects_supported_languages_from_os_locale_tags() {
        for (locale, expected) in [
            ("en-US", "en"),
            ("en-GB", "en"),
            ("ru-RU", "ru"),
            ("ja-JP", "ja"),
            ("tr-TR", "tr"),
            ("vi-VN", "vi"),
            ("pt-BR", "pt"),
            ("pt-PT", "pt"),
            ("ko-KR", "ko"),
            ("ar-SA", "ar"),
            ("es-MX", "es"),
            ("ms-MY", "my"),
            ("RU_ru.UTF-8", "ru"),
            ("es_ES@euro", "es"),
        ] {
            assert_eq!(language_from_locales([locale]), expected, "{locale}");
        }
    }

    #[test]
    fn distinguishes_chinese_scripts_and_regions() {
        for (locale, expected) in [
            ("zh", "zh"),
            ("zh-CN", "zh"),
            ("zh-SG", "zh"),
            ("zh-Hans", "zh"),
            ("zh-Hans-TW", "zh"),
            ("zh-TW", "zh-TW"),
            ("zh-HK", "zh-TW"),
            ("zh-MO", "zh-TW"),
            ("zh-Hant", "zh-TW"),
            ("zh-Hant-CN", "zh-TW"),
        ] {
            assert_eq!(language_from_locales([locale]), expected, "{locale}");
        }
    }

    #[test]
    fn honors_preference_order_and_skips_unsupported_languages() {
        assert_eq!(language_from_locales(["de-DE", "ru-RU", "en-US"]), "ru");
        assert_eq!(language_from_locales(["en-GB", "zh-CN"]), "en");
        assert_eq!(language_from_locales(["zh-TW", "en-US"]), "zh-TW");
    }

    #[test]
    fn falls_back_to_english_without_a_supported_locale() {
        assert_eq!(language_from_locales(Vec::<String>::new()), "en");
        for locale in ["", "C", "POSIX", "C.UTF-8", "de-DE", "my-MM"] {
            assert_eq!(language_from_locales([locale]), "en", "{locale}");
        }
    }
}
