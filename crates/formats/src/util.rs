use locust_core::error::LocustError;

pub(crate) fn parse_err(file: &str, message: impl Into<String>) -> LocustError {
    LocustError::ParseError {
        file: file.into(),
        message: message.into(),
    }
}

/// Maximum decoded bytes in one entry, script, or archive index (512 MiB).
pub(crate) const MAX_ENTRY_DECODED: u64 = 512 * 1024 * 1024;
/// Maximum declared decoded entry total for NW archives (1 GiB).
pub(crate) const MAX_ARCHIVE_DECODED: u64 = 1024 * 1024 * 1024;

/// A checked aggregate budget. Failed charges leave the remaining budget intact.
pub(crate) struct DecodeBudget {
    remaining: u64,
}

impl DecodeBudget {
    pub(crate) fn new(limit: u64) -> Self {
        Self { remaining: limit }
    }

    pub(crate) fn consume(&mut self, size: u64) -> Result<(), LocustError> {
        self.remaining = self
            .remaining
            .checked_sub(size)
            .ok_or_else(|| parse_err("archive", "decoded archive size exceeds budget"))?;
        Ok(())
    }
}

/// Read at most the effective ceiling plus one probe byte, without trusting sizes
/// for allocation. A declared size also bounds the read and must match exactly.
pub(crate) fn read_bounded(
    reader: impl std::io::Read,
    expected: Option<u64>,
    limit: u64,
) -> Result<Vec<u8>, LocustError> {
    use std::io::Read;
    if expected.is_some_and(|size| size > limit) {
        return Err(parse_err("archive", "declared decoded size exceeds limit"));
    }
    let ceiling = expected.unwrap_or(limit).min(limit);
    let probe_limit = ceiling
        .checked_add(1)
        .ok_or_else(|| parse_err("archive", "decoded size limit overflow"))?;
    let mut output = Vec::new();
    reader
        .take(probe_limit)
        .read_to_end(&mut output)
        .map_err(|e| parse_err("archive", format!("decoded read failed: {e}")))?;
    if output.len() as u64 > ceiling {
        return Err(parse_err(
            "archive",
            "decoded output exceeds limit or declared size",
        ));
    }
    if expected.is_some_and(|size| size != output.len() as u64) {
        return Err(parse_err(
            "archive",
            "decoded size differs from declared size",
        ));
    }
    Ok(output)
}

/// Inflate zlib through a bounded streaming reader. No allocation uses an
/// untrusted declared size; over-limit declarations fail before decoding.
pub(crate) fn inflate_zlib_bounded(
    input: &[u8],
    expected: Option<u64>,
    limit: u64,
) -> Result<Vec<u8>, LocustError> {
    read_bounded(flate2::read::ZlibDecoder::new(input), expected, limit)
}

/// RFC 1950 Adler-32 (initial a=1, b=0).
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + u32::from(byte)) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

/// Decode LZString base64 with a byte ceiling on both UTF-16 and UTF-8 output,
/// and a separate dictionary-memory ceiling of max(limit, 1 KiB). Dictionary
/// ranges refer to decoded output instead of cloning expanded strings.
pub(crate) fn decode_lzstring_base64_bounded(
    input: &str,
    limit: u64,
) -> Result<String, LocustError> {
    const KEY: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=";
    let invalid = || parse_err("jsono", "invalid or over-limit LZString base64 payload");
    // lz-str 0.2 ignores characters outside its alphabet; preserve that behavior.
    let values = input
        .bytes()
        .filter_map(|byte| KEY.iter().position(|&key| key == byte));
    let mut bits =
        values.flat_map(|value| (0..6).rev().map(move |bit| ((value >> bit) & 1) as u32));
    let mut read = |count: u32| -> Option<u32> {
        if count > 31 {
            return None;
        }
        let mut value = 0;
        for bit in 0..count {
            value |= bits.next()? << bit;
        }
        Some(value)
    };
    let Some(first_code) = read(2) else {
        return if input.is_empty() {
            Ok(String::new())
        } else {
            Err(invalid())
        };
    };
    let first = match first_code {
        0 => read(8).ok_or_else(invalid)? as u16,
        1 => read(16).ok_or_else(invalid)? as u16,
        2 => return Ok(String::new()),
        _ => return Err(invalid()),
    };
    if limit < 2 {
        return Err(invalid());
    }
    #[derive(Clone, Copy)]
    enum Entry {
        Literal(u16),
        Range(usize, usize),
    }
    let mut dictionary = vec![
        Entry::Literal(0),
        Entry::Literal(1),
        Entry::Literal(2),
        Entry::Literal(first),
    ];
    let check_dictionary = |len: usize| -> Result<(), LocustError> {
        if (len as u64)
            .checked_mul(std::mem::size_of::<Entry>() as u64)
            .is_none_or(|bytes| bytes > limit.max(1024))
        {
            return Err(invalid());
        }
        Ok(())
    };
    check_dictionary(dictionary.len())?;
    let mut output = vec![first];
    let mut previous_start = 0;
    let mut previous_len = 1;
    let mut width = 3;
    let mut enlarge_in = 4u64;
    loop {
        let mut code = read(width).ok_or_else(invalid)? as usize;
        match code {
            0 | 1 => {
                let literal = read(if code == 0 { 8 } else { 16 }).ok_or_else(invalid)? as u16;
                check_dictionary(dictionary.len() + 1)?;
                dictionary.push(Entry::Literal(literal));
                code = dictionary.len() - 1;
                enlarge_in -= 1;
            }
            2 => break,
            _ => {}
        }
        if enlarge_in == 0 {
            enlarge_in = 1u64 << width;
            width += 1;
        }
        let entry = if let Some(&entry) = dictionary.get(code) {
            entry
        } else if code == dictionary.len() {
            Entry::Range(previous_start, previous_len + 1)
        } else {
            return Err(invalid());
        };
        let len = match entry {
            Entry::Literal(_) => 1,
            Entry::Range(_, len) => len,
        };
        let start = output.len();
        if (start as u64 + len as u64) > limit / 2 {
            return Err(invalid());
        }
        match entry {
            Entry::Literal(literal) => output.push(literal),
            Entry::Range(range_start, range_len) => {
                if code == dictionary.len() {
                    let first = output[previous_start];
                    output.extend_from_within(previous_start..previous_start + previous_len);
                    output.push(first);
                } else {
                    output.extend_from_within(range_start..range_start + range_len);
                }
            }
        }
        check_dictionary(dictionary.len() + 1)?;
        // The previous word immediately precedes the first unit of this word.
        dictionary.push(Entry::Range(previous_start, previous_len + 1));
        previous_start = start;
        previous_len = len;
        enlarge_in -= 1;
        if enlarge_in == 0 {
            enlarge_in = 1u64 << width;
            width += 1;
        }
    }
    let mut text = String::new();
    for ch in char::decode_utf16(output) {
        let ch = ch.unwrap_or(char::REPLACEMENT_CHARACTER);
        if text.len() as u64 + ch.len_utf8() as u64 > limit {
            return Err(invalid());
        }
        text.push(ch);
    }
    Ok(text)
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
