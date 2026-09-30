use std::io::Read;
use std::path::Path;

use crate::error::{LocustError, Result};

pub struct EncodingDetector;

impl EncodingDetector {
    pub fn detect_and_decode(bytes: &[u8]) -> Result<(String, &'static str)> {
        // Check for UTF-8 BOM
        if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
            if let Ok(s) = std::str::from_utf8(&bytes[3..]) {
                return Ok((s.to_string(), "UTF-8"));
            }
        }

        // Valid UTF-8 is authoritative: legacy detectors can confidently
        // misidentify mostly-ASCII text with a few Unicode characters.
        if let Ok(s) = std::str::from_utf8(bytes) {
            return Ok((s.to_string(), "UTF-8"));
        }

        // Only use statistical detection for bytes that are not valid UTF-8.
        let detection = chardet::detect(bytes);
        let charset = detection.0.to_uppercase();
        let confidence = detection.1;

        // Map chardet names to encoding_rs labels
        let label = match charset.as_str() {
            "SHIFT_JIS" | "SHIFT-JIS" | "WINDOWS-31J" => "Shift_JIS",
            "EUC-JP" => "EUC-JP",
            "WINDOWS-1252" | "ISO-8859-1" => "windows-1252",
            "WINDOWS-1251" | "ISO-8859-5" => "windows-1251",
            "GB2312" | "GB18030" => "gb18030",
            "BIG5" => "Big5",
            _ => &charset,
        };

        if !label.is_empty() {
            if let Some(encoding) = encoding_rs::Encoding::for_label(label.as_bytes()) {
                let (decoded, _, had_errors) = encoding.decode(bytes);
                if !had_errors {
                    return Ok((decoded.into_owned(), encoding.name()));
                }
            }
        }

        // Fallback: try common Japanese/CJK encodings
        let fallback_encodings = [
            "Shift_JIS",
            "EUC-JP",
            "gb18030",
            "Big5",
            "windows-1252",
            "windows-1251",
        ];
        for enc_label in &fallback_encodings {
            if let Some(encoding) = encoding_rs::Encoding::for_label(enc_label.as_bytes()) {
                let (decoded, _, had_errors) = encoding.decode(bytes);
                if !had_errors {
                    return Ok((decoded.into_owned(), encoding.name()));
                }
            }
        }

        Err(LocustError::EncodingError(format!(
            "could not decode bytes (detected: {}, confidence: {:.2})",
            charset, confidence
        )))
    }

    pub fn encode_to_original(text: &str, encoding_name: &str) -> Result<Vec<u8>> {
        if encoding_name == "UTF-8" {
            return Ok(text.as_bytes().to_vec());
        }
        if let Some(encoding) = encoding_rs::Encoding::for_label(encoding_name.as_bytes()) {
            let (bytes, _, had_errors) = encoding.encode(text);
            if had_errors {
                return Err(LocustError::EncodingError(format!(
                    "failed to encode text to {}",
                    encoding_name
                )));
            }
            Ok(bytes.into_owned())
        } else {
            Err(LocustError::EncodingError(format!(
                "unknown encoding: {}",
                encoding_name
            )))
        }
    }

    pub fn read_file_auto(path: &Path) -> Result<(String, &'static str)> {
        let bytes = std::fs::read(path)?;
        let result = Self::detect_and_decode(&bytes)?;
        tracing::debug!(
            "Detected encoding '{}' for file: {}",
            result.1,
            path.display()
        );
        Ok(result)
    }

    pub fn write_file_encoded(path: &Path, text: &str, encoding_name: &str) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut bytes = Self::encode_to_original(text, encoding_name)?;
        if encoding_name == "UTF-8" {
            // read_file_auto strips the BOM. Preserve it when overwriting
            // the original file, while new UTF-8 files remain BOM-free.
            match std::fs::File::open(path) {
                Ok(file) => {
                    let mut prefix = Vec::new();
                    file.take(3).read_to_end(&mut prefix)?;
                    if prefix == [0xEF, 0xBB, 0xBF] {
                        bytes.splice(0..0, prefix);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        std::fs::write(path, bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_enc_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_detect_utf8() {
        let text = "Hello, world! 日本語テスト";
        let bytes = text.as_bytes();
        let (decoded, enc) = EncodingDetector::detect_and_decode(bytes).unwrap();
        assert_eq!(decoded, text);
        assert_eq!(enc, "UTF-8");
    }

    #[test]
    fn test_detect_utf8_bom() {
        let text = "Hello BOM";
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(text.as_bytes());
        let (decoded, enc) = EncodingDetector::detect_and_decode(&bytes).unwrap();
        assert_eq!(decoded, text);
        assert_eq!(enc, "UTF-8");
    }

    #[test]
    fn test_detect_utf8_json_overrides_confident_legacy_detection() {
        // Keep each rare character in its own mostly-ASCII JSON document:
        // combining them raises chardet's UTF-8 confidence and hides the bug.
        for text in [
            "Ice Ⅰ",
            "Wait…",
            "I didn’t expect help from a human!",
            "Hello\u{3000}world",
        ] {
            let json = serde_json::json!({ "text": text }).to_string();
            let (charset, confidence, _) = chardet::detect(json.as_bytes());
            assert!(
                matches!(
                    charset.to_uppercase().as_str(),
                    "WINDOWS-1252" | "ISO-8859-1"
                ) && confidence >= 0.6,
                "fixture must reproduce confident legacy detection: {charset}, {confidence}"
            );
            for bom in [false, true] {
                let mut bytes = if bom { vec![0xEF, 0xBB, 0xBF] } else { vec![] };
                bytes.extend_from_slice(json.as_bytes());
                let (decoded, enc) = EncodingDetector::detect_and_decode(&bytes).unwrap();
                assert_eq!(decoded, json, "BOM: {bom}");
                assert_eq!(enc, "UTF-8");
                assert_eq!(
                    EncodingDetector::encode_to_original(&decoded, enc).unwrap(),
                    json.as_bytes()
                );
            }
        }
    }

    #[test]
    fn test_detect_windows_1252_invalid_utf8() {
        let text = "Café: it’s £5…";
        let bytes = EncodingDetector::encode_to_original(text, "windows-1252").unwrap();
        assert!(std::str::from_utf8(&bytes).is_err());
        let (decoded, enc) = EncodingDetector::detect_and_decode(&bytes).unwrap();
        assert_eq!(decoded, text);
        assert_eq!(enc, "windows-1252");
        assert_eq!(
            EncodingDetector::encode_to_original(&decoded, enc).unwrap(),
            bytes
        );
    }

    #[test]
    fn test_detect_shift_jis() {
        // Encode "テスト" to Shift-JIS via encoding_rs
        let text = "テスト";
        let sjis_bytes = EncodingDetector::encode_to_original(text, "Shift_JIS").unwrap();
        assert!(std::str::from_utf8(&sjis_bytes).is_err());
        let (decoded, enc) = EncodingDetector::detect_and_decode(&sjis_bytes).unwrap();
        assert_eq!(decoded, text);
        assert_eq!(enc, "Shift_JIS");
    }

    #[test]
    fn test_roundtrip_shift_jis() {
        let text = "これはテストです。日本語の文章を書いています。";
        let encoded = EncodingDetector::encode_to_original(text, "Shift_JIS").unwrap();
        let (decoded, _) = EncodingDetector::detect_and_decode(&encoded).unwrap();
        assert_eq!(decoded, text);
    }

    #[test]
    fn test_read_file_auto_utf8() {
        let tmp = tempdir();
        let path = tmp.join("test.txt");
        std::fs::write(&path, "Hello UTF-8 file").unwrap();
        let (text, enc) = EncodingDetector::read_file_auto(&path).unwrap();
        assert_eq!(text, "Hello UTF-8 file");
        assert_eq!(enc, "UTF-8");
    }

    #[test]
    fn test_write_file_encoded_creates_dirs() {
        let tmp = tempdir();
        let path = tmp.join("deep").join("nested").join("file.txt");
        EncodingDetector::write_file_encoded(&path, "hello", "UTF-8").unwrap();
        assert!(path.exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn test_write_file_encoded_preserves_utf8_bom() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text.json");
        let text = "Ice Ⅰ";
        for bom in [false, true] {
            let mut bytes = if bom { vec![0xEF, 0xBB, 0xBF] } else { vec![] };
            bytes.extend_from_slice(text.as_bytes());
            std::fs::write(&path, &bytes).unwrap();
            let (decoded, enc) = EncodingDetector::read_file_auto(&path).unwrap();
            EncodingDetector::write_file_encoded(&path, &decoded, enc).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
}
