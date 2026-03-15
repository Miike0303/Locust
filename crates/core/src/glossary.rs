use crate::database::{Database, GlossaryEntry};
use crate::error::Result;
use std::sync::Arc;

pub struct Glossary {
    db: Arc<Database>,
}

impl Glossary {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    pub fn add(
        &self,
        term: &str,
        translation: &str,
        lang_pair: &str,
        context: Option<&str>,
    ) -> Result<()> {
        self.db.save_glossary_entry(&GlossaryEntry {
            term: term.to_string(),
            translation: translation.to_string(),
            lang_pair: lang_pair.to_string(),
            context: context.map(|s| s.to_string()),
            case_sensitive: false,
        })
    }

    pub fn get_all(&self, lang_pair: &str) -> Result<Vec<GlossaryEntry>> {
        self.db.get_glossary(lang_pair)
    }

    pub fn delete(&self, term: &str, lang_pair: &str) -> Result<()> {
        self.db.delete_glossary_entry(term, lang_pair)
    }

    pub fn build_hint(&self, source_lang: &str, target_lang: &str) -> Option<String> {
        let lang_pair = format!("{}-{}", source_lang, target_lang);
        let entries = self.get_all(&lang_pair).ok()?;
        if entries.is_empty() {
            return None;
        }
        let mut hint = String::from("Glossary (use these translations consistently):\n");
        for entry in entries.iter().take(50) {
            hint.push_str(&format!("  {} → {}\n", entry.term, entry.translation));
        }
        Some(hint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;

    fn setup() -> (Arc<Database>, Glossary) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let glossary = Glossary::new(db.clone());
        (db, glossary)
    }

    #[test]
    fn test_add_and_get() {
        let (_db, glossary) = setup();
        glossary.add("HP", "Health Points", "ja-en", None).unwrap();
        glossary.add("MP", "Magic Points", "ja-en", None).unwrap();
        let entries = glossary.get_all("ja-en").unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_delete_entry() {
        let (_db, glossary) = setup();
        glossary.add("HP", "Health Points", "ja-en", None).unwrap();
        glossary.delete("HP", "ja-en").unwrap();
        let entries = glossary.get_all("ja-en").unwrap();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_build_hint_empty() {
        let (_db, glossary) = setup();
        assert!(glossary.build_hint("ja", "en").is_none());
    }

    #[test]
    fn test_build_hint_format() {
        let (_db, glossary) = setup();
        glossary.add("HP", "Health Points", "ja-en", None).unwrap();
        glossary.add("MP", "Magic Points", "ja-en", None).unwrap();
        let hint = glossary.build_hint("ja", "en").unwrap();
        assert!(hint.contains("HP → Health Points"));
        assert!(hint.contains("MP → Magic Points"));
        assert!(hint.starts_with("Glossary (use these translations consistently):\n"));
    }

    #[test]
    fn test_build_hint_max_50() {
        let (_db, glossary) = setup();
        for i in 0..60 {
            glossary
                .add(&format!("term{}", i), &format!("trans{}", i), "ja-en", None)
                .unwrap();
        }
        let hint = glossary.build_hint("ja", "en").unwrap();
        let lines: Vec<&str> = hint.lines().collect();
        // 1 header line + 50 entry lines
        assert_eq!(lines.len(), 51);
    }

    #[test]
    fn test_lang_pair_isolation() {
        let (_db, glossary) = setup();
        glossary.add("HP", "Health Points", "ja-en", None).unwrap();
        glossary.add("vida", "life", "es-en", None).unwrap();
        let ja_entries = glossary.get_all("ja-en").unwrap();
        assert_eq!(ja_entries.len(), 1);
        assert_eq!(ja_entries[0].term, "HP");
    }

    #[test]
    fn test_duplicate_term_upserts() {
        let (_db, glossary) = setup();
        glossary.add("HP", "Health Points", "ja-en", None).unwrap();
        glossary.add("HP", "Hit Points", "ja-en", None).unwrap();
        let entries = glossary.get_all("ja-en").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].translation, "Hit Points");
    }
}
