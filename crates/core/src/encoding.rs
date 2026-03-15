use std::path::Path;

use crate::error::{LocustError, Result};

pub struct EncodingDetector;

impl EncodingDetector {
    pub fn detect_and_decode(bytes: &[u8]) -> Result<(String, &'static str)> {
        // Check for UTF-8 BOM
        if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
            match std::str::from_utf8(&bytes[3..]) {
                Ok(s) => return Ok((s.to_string(), "UTF-8")),
                Err(_) => {}
            }
        }

        // Try chardet detection
        let detection = chardet::detect(bytes);
        let charset = detection.0.to_uppercase();
        let confidence = detection.1;

        // Low confidence or detected as UTF-8: try UTF-8 first
        if confidence < 0.6 || charset == "UTF-8" || charset == "ASCII" {
            if let Ok(s) = String::from_utf8(bytes.to_vec()) {
                return Ok((s, "UTF-8"));
            }
        }

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
            "Shift_JIS", "EUC-JP", "gb18030", "Big5", "windows-1252", "windows-1251",
        ];
        for enc_label in &fallback_encodings {
            if let Some(encoding) = encoding_rs::Encoding::for_label(enc_label.as_bytes()) {
                let (decoded, _, had_errors) = encoding.decode(bytes);
                if !had_errors {
                    return Ok((decoded.into_owned(), encoding.name()));
                }
            }
        }

        // Final fallback: try UTF-8
        if let Ok(s) = String::from_utf8(bytes.to_vec()) {
            return Ok((s, "UTF-8"));
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
        let bytes = Self::encode_to_original(text, encoding_name)?;
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
    fn test_detect_shift_jis() {
        // Encode "テスト" to Shift-JIS via encoding_rs
        let text = "テスト";
        let sjis_bytes = EncodingDetector::encode_to_original(text, "Shift_JIS").unwrap();
        let (decoded, _enc) = EncodingDetector::detect_and_decode(&sjis_bytes).unwrap();
        assert_eq!(decoded, text);
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
}
