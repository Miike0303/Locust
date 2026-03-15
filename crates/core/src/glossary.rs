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

    pub fn build_hint(&self, lang_pair: &str) -> Result<Option<String>> {
        let entries = self.db.get_glossary(lang_pair)?;
        if entries.is_empty() {
            return Ok(None);
        }
        let lines: Vec<String> = entries
            .iter()
            .map(|e| format!("{} = {}", e.term, e.translation))
            .collect();
        Ok(Some(lines.join("; ")))
    }

    pub fn add_entry(&self, entry: &GlossaryEntry) -> Result<()> {
        self.db.save_glossary_entry(entry)
    }

    pub fn get_entries(&self, lang_pair: &str) -> Result<Vec<GlossaryEntry>> {
        self.db.get_glossary(lang_pair)
    }

    pub fn delete_entry(&self, term: &str, lang_pair: &str) -> Result<()> {
        self.db.delete_glossary_entry(term, lang_pair)
    }
}
