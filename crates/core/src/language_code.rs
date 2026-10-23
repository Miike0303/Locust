use crate::error::{LocustError, Result};

/// Validate a target language before it can become a directory or pack name.
/// This intentionally accepts the existing BCP-47-like ASCII codes, including
/// underscores, without imposing stricter language-tag semantics.
pub fn validate_language_code(code: &str) -> Result<()> {
    if !(1..=35).contains(&code.len())
        || !code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(LocustError::InjectionError(
            "invalid target language code: use 1 to 35 ASCII letters, digits, '-' or '_'".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_language_code;

    #[test]
    fn accepts_supported_language_codes_and_length_boundaries() {
        for code in [
            "es", "pt-BR", "zh-Hans", "zh_CN", "es-419", "a", "0", "_", "-",
        ] {
            assert!(validate_language_code(code).is_ok(), "{code:?}");
        }
        assert!(validate_language_code(&"a".repeat(35)).is_ok());
    }

    #[test]
    fn rejects_unsafe_characters_and_invalid_lengths() {
        for code in [
            "",
            ".",
            "..",
            "../x",
            "..\\x",
            "a/b",
            "a\\b",
            "C:x",
            "C:\\x",
            "/es",
            "\\\\server\\share",
            "es\0",
            " es",
            "es ",
            "es\t",
            "es\n",
            "es\r",
            "es.ES",
            "es:ES",
            "es@ES",
            "es%2fES",
            "es+ES",
            "es\u{a0}",
            "é",
            "中文",
        ] {
            let error = validate_language_code(code).unwrap_err().to_string();
            assert!(
                error.contains("invalid target language code"),
                "{code:?}: {error}"
            );
        }
        assert!(validate_language_code(&"a".repeat(36)).is_err());
    }

    #[test]
    fn accepts_every_desktop_language_option() {
        let catalog = include_str!("../../../apps/desktop/src/lib/languages.ts");
        let codes: Vec<_> = catalog
            .lines()
            .filter_map(|line| {
                line.split_once("code: \"")?
                    .1
                    .split_once('"')
                    .map(|(code, _)| code)
            })
            .collect();
        assert!(
            !codes.is_empty(),
            "desktop language options must be checked"
        );
        for code in codes {
            assert!(validate_language_code(code).is_ok(), "{code:?}");
        }
    }
}
