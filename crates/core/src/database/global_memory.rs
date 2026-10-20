use std::path::Path;

use crate::error::Result;

use super::{Database, MemoryEntry};

/// Global translation memory database — shared across projects.
pub struct GlobalMemoryDb {
    db: Database,
}

impl GlobalMemoryDb {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Database::open(path)?;
        Ok(Self { db })
    }

    pub fn open_in_memory() -> Result<Self> {
        let db = Database::open_in_memory()?;
        Ok(Self { db })
    }

    pub fn open_default() -> Result<Self> {
        let config_dir = crate::config::AppConfig::config_dir();
        let path = config_dir.join("global_memory.db");
        Self::open(&path)
    }

    pub fn lookup_memory(&self, source_hash: &str, lang_pair: &str) -> Result<Option<String>> {
        self.db.lookup_memory(source_hash, lang_pair)
    }

    pub async fn save_memory(
        &self,
        hash: &str,
        source: &str,
        translation: &str,
        lang_pair: &str,
    ) -> Result<()> {
        self.db
            .save_memory(hash, source, translation, lang_pair)
            .await
    }

    pub fn memory_count(&self) -> Result<usize> {
        self.db.memory_count()
    }

    pub fn memory_candidates(
        &self,
        lang_pair: &str,
        source_len: usize,
        max_rows: usize,
    ) -> Result<Vec<MemoryEntry>> {
        self.db.memory_candidates(lang_pair, source_len, max_rows)
    }

    pub fn list_memory(
        &self,
        search: Option<&str>,
        lang_pair: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<MemoryEntry>, usize)> {
        self.db.list_memory(search, lang_pair, limit, offset)
    }

    pub fn delete_memory(&self, source_hash: &str, lang_pair: &str) -> Result<()> {
        self.db.delete_memory(source_hash, lang_pair)
    }

    pub fn clear_memory(&self) -> Result<()> {
        self.db.clear_memory()
    }

    pub fn memory_lang_pairs(&self) -> Result<Vec<String>> {
        self.db.memory_lang_pairs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn global_memory_candidates_use_the_same_filters_order_and_cap() {
        let global = GlobalMemoryDb::open_in_memory().unwrap();
        let items = (0..3001)
            .map(|i| (format!("hash{i}"), "界".repeat(10), "Translation".into()))
            .collect::<Vec<_>>();
        global.db.save_memory_batch(&items, "en-es").await.unwrap();
        global
            .save_memory("hash1", &"界".repeat(10), "Popular", "en-es")
            .await
            .unwrap();
        global
            .save_memory("long", &"界".repeat(15), "Long", "en-es")
            .await
            .unwrap();
        global
            .save_memory("other", &"界".repeat(10), "Other", "en-fr")
            .await
            .unwrap();
        let rows = global.memory_candidates("en-es", 10, usize::MAX).unwrap();
        assert_eq!(rows.len(), 3000);
        assert_eq!(rows[0].source_hash, "hash1");
        assert!(rows
            .iter()
            .all(|row| row.lang_pair == "en-es" && row.source.chars().count() == 10));
        assert!(global.memory_candidates("en-es", 10, 0).unwrap().is_empty());
        assert_eq!(
            global.memory_candidates("en-fr", 10, 1).unwrap()[0].source_hash,
            "other"
        );
    }

    #[test]
    fn test_global_memory_saves_across_calls() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let gm = GlobalMemoryDb::open_in_memory().unwrap();
        rt.block_on(async {
            gm.save_memory("hash_g1", "Hello", "Hola", "en-es")
                .await
                .unwrap();
        });
        let result = gm.lookup_memory("hash_g1", "en-es").unwrap();
        assert_eq!(result, Some("Hola".to_string()));
        assert_eq!(gm.memory_count().unwrap(), 1);
    }

    #[test]
    fn test_global_memory_used_in_new_project() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let gm = GlobalMemoryDb::open_in_memory().unwrap();
        rt.block_on(async {
            gm.save_memory("hash_g2", "World", "Mundo", "en-es")
                .await
                .unwrap();
        });
        // Simulate new project checking global memory
        let result = gm.lookup_memory("hash_g2", "en-es").unwrap();
        assert_eq!(result, Some("Mundo".to_string()));
    }
}
