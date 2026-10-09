use locust_core::error::LocustError;

pub(crate) fn parse_err(file: &str, message: impl Into<String>) -> LocustError {
    LocustError::ParseError {
        file: file.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_parse_error(error: LocustError, expected_file: &str, expected_message: &str) {
        match error {
            LocustError::ParseError { file, message } => {
                assert_eq!(file, expected_file);
                assert_eq!(message, expected_message);
            }
            other => panic!("expected ParseError, got {other:?}"),
        }
    }

    fn assert_borrowed_errors(file: &str, message: &str) {
        assert_parse_error(parse_err(file, message), file, message);
    }

    fn assert_owned_errors(file: &str, message: &str) {
        assert_parse_error(parse_err(file, message.to_owned()), file, message);
    }

    #[test]
    fn parse_err_preserves_borrowed_literals() {
        assert_borrowed_errors("scenario.ks", "invalid payload");
    }

    #[test]
    fn parse_err_preserves_owned_literals() {
        assert_owned_errors("script.ybn", "truncated header");
    }

    #[test]
    fn parse_err_preserves_empty_literals() {
        assert_borrowed_errors("", "");
        assert_owned_errors("", "");
    }

    #[test]
    fn parse_err_preserves_unicode_literals() {
        assert_borrowed_errors("場面/🌍.ks", "café: 無効 🦀");
        assert_owned_errors("場面/🌍.ks", "café: 無効 🦀");
    }

    #[test]
    fn parse_err_preserves_cr_lf_literals() {
        assert_borrowed_errors("file\r\nname\r.ks", "first\rsecond\nthird\r\n");
        assert_owned_errors("file\r\nname\r.ks", "first\rsecond\nthird\r\n");
    }
}
