use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::{Path, PathBuf};

static MIGRATIONS: &[M] = &[M::up(
    "CREATE TABLE facts (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        text TEXT NOT NULL,
        source_transcript TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        superseded_by INTEGER REFERENCES facts(id) DEFAULT NULL
    );
    CREATE INDEX idx_facts_created_at ON facts(created_at);",
)];

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct Fact {
    pub id: i64,
    pub text: String,
    pub source_transcript: String,
    pub created_at: i64,
}

pub struct MemoryManager {
    db_path: PathBuf,
}

impl MemoryManager {
    pub fn new(app_handle: &tauri::AppHandle) -> Result<Self> {
        let path = crate::portable::app_data_dir(app_handle)?.join("memory.db");
        Self::open(path)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db_path = path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut connection = Connection::open(&db_path)?;
        Migrations::new(MIGRATIONS.to_vec()).to_latest(&mut connection)?;
        Ok(Self { db_path })
    }

    fn get_connection(&self) -> Result<Connection> {
        Ok(Connection::open(&self.db_path)?)
    }

    pub fn forget(&self, id: i64) -> Result<()> {
        self.get_connection()?
            .execute("DELETE FROM facts WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn recall(&self, query: &str, limit: usize) -> Result<Vec<Fact>> {
        let query_words = significant_words(query);
        if query_words.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let facts = self.list_all()?;
        Ok(facts
            .into_iter()
            .filter(|fact| {
                fact.text
                    .split(|character: char| !character.is_alphanumeric())
                    .filter(|word| !word.is_empty())
                    .any(|word| {
                        is_significant_word(word)
                            && query_words
                                .iter()
                                .any(|query_word| words_match(word, query_word))
                    })
            })
            .take(limit)
            .collect())
    }

    pub fn list_all(&self) -> Result<Vec<Fact>> {
        let connection = self.get_connection()?;
        let mut statement = connection.prepare(
            "SELECT id, text, source_transcript, created_at FROM facts ORDER BY created_at DESC, id DESC",
        )?;
        let facts = statement
            .query_map([], |row| {
                Ok(Fact {
                    id: row.get("id")?,
                    text: row.get("text")?,
                    source_transcript: row.get("source_transcript")?,
                    created_at: row.get("created_at")?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(facts)
    }

    pub fn remember(&self, fact_text: &str, source_transcript: &str) -> Result<i64> {
        let connection = self.get_connection()?;
        connection.execute(
            "INSERT INTO facts (text, source_transcript, created_at) VALUES (?1, ?2, ?3)",
            params![fact_text, source_transcript, Utc::now().timestamp_millis()],
        )?;
        Ok(connection.last_insert_rowid())
    }

    pub fn extract_fact(transcript: &str) -> Option<String> {
        const TRIGGERS: &[&str] = &[
            "remember that ",
            "remember I ",
            "remember my ",
            "don't forget that ",
            "note that ",
        ];

        let match_start = transcript.char_indices().find_map(|(start, _)| {
            TRIGGERS.iter().find_map(|trigger| {
                transcript
                    .get(start..start + trigger.len())
                    .filter(|candidate| candidate.eq_ignore_ascii_case(trigger))
                    .map(|_| (start, trigger.len()))
            })
        })?;
        let fact = transcript.get(match_start.0 + match_start.1..)?.trim();
        (!fact.is_empty()).then(|| fact.to_string())
    }
}

const STOP_WORDS: &[&str] = &[
    "the", "a", "an", "is", "are", "i", "my", "that", "to", "of", "in", "on", "do", "what", "does",
    "it", "and", "over",
];

fn significant_words(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| is_significant_word(word))
        .map(|word| normalize_word(word).to_ascii_lowercase())
        .collect()
}

fn is_significant_word(word: &str) -> bool {
    !word.is_empty()
        && !STOP_WORDS
            .iter()
            .any(|stop_word| word.eq_ignore_ascii_case(stop_word))
}

fn normalize_word(word: &str) -> &str {
    if word.len() > 3 && word.ends_with('s') {
        &word[..word.len() - 1]
    } else {
        word
    }
}

fn words_match(left: &str, right: &str) -> bool {
    normalize_word(left).eq_ignore_ascii_case(normalize_word(right))
}

#[cfg(test)]
mod tests {
    use super::MemoryManager;

    #[test]
    fn recall_matches_prefer_inflection() {
        let dir = tempfile::tempdir().unwrap();
        let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        manager
            .remember("prefers tea over coffee", "source")
            .unwrap();
        assert_eq!(
            manager
                .recall("what do I prefer to drink", 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn forget_removes_stored_fact() {
        let dir = tempfile::tempdir().unwrap();
        let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        let id = manager.remember("prefers tea", "source").unwrap();
        manager.forget(id).unwrap();
        assert!(manager.list_all().unwrap().is_empty());
    }

    #[test]
    fn facts_persist_after_manager_reopens_database() {
        let dir = tempfile::tempdir().unwrap();
        {
            let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
            manager
                .remember("prefers tea", "remember that I prefer tea")
                .unwrap();
        }
        let reopened = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        assert_eq!(reopened.list_all().unwrap()[0].text, "prefers tea");
    }

    #[test]
    fn list_all_returns_newest_first_for_same_millisecond() {
        let dir = tempfile::tempdir().unwrap();
        let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        manager.remember("first", "source").unwrap();
        manager.remember("second", "source").unwrap();
        manager.remember("third", "source").unwrap();
        let facts = manager.list_all().unwrap();
        assert_eq!(
            facts
                .iter()
                .map(|fact| fact.text.as_str())
                .collect::<Vec<_>>(),
            ["third", "second", "first"]
        );
    }

    #[test]
    fn recall_returns_only_the_matching_fact() {
        let dir = tempfile::tempdir().unwrap();
        let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        manager.remember("prefers tea", "source").unwrap();
        manager.remember("allergic to peanuts", "source").unwrap();
        let facts = manager.recall("tea", 10).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].text, "prefers tea");
    }

    #[test]
    fn recall_excludes_unrelated_facts() {
        let dir = tempfile::tempdir().unwrap();
        let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        manager
            .remember("prefers tea over coffee", "source")
            .unwrap();
        assert!(manager
            .recall("what's the capital of France", 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn recall_limit_keeps_newest_matching_facts() {
        let dir = tempfile::tempdir().unwrap();
        let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        for fact in ["tea one", "tea two", "tea three", "tea four", "tea five"] {
            manager.remember(fact, "source").unwrap();
        }
        let facts = manager.recall("tea", 2).unwrap();
        assert_eq!(
            facts
                .iter()
                .map(|fact| fact.text.as_str())
                .collect::<Vec<_>>(),
            ["tea five", "tea four"]
        );
    }

    #[test]
    fn recall_on_empty_database_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        assert!(manager.recall("anything", 10).unwrap().is_empty());
    }

    #[test]
    fn recall_ignores_stopword_only_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        manager.remember("it is", "source").unwrap();
        assert!(manager.recall("the is a", 10).unwrap().is_empty());
    }

    #[test]
    fn recall_matches_garage_code_query_without_stopwords() {
        let dir = tempfile::tempdir().unwrap();
        let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        manager.remember("garage code is 4521", "source").unwrap();
        assert_eq!(
            manager.recall("what's my garage code", 10).unwrap().len(),
            1
        );
    }

    #[test]
    fn recall_is_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        manager.remember("favorite TEA", "source").unwrap();
        assert_eq!(manager.recall("tea", 10).unwrap().len(), 1);
    }

    #[test]
    fn forgetting_missing_and_already_deleted_ids_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        manager.forget(99_999).unwrap();
        let id = manager.remember("prefers tea", "source").unwrap();
        manager.forget(id).unwrap();
        manager.forget(id).unwrap();
        assert!(manager.list_all().unwrap().is_empty());
    }

    #[test]
    fn concurrent_remember_calls_persist_both_facts() {
        use std::sync::Arc;
        use std::thread;

        let dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(MemoryManager::open(dir.path().join("memory.db")).unwrap());
        let first = Arc::clone(&manager);
        let second = Arc::clone(&manager);
        let a = thread::spawn(move || first.remember("first fact", "source one").unwrap());
        let b = thread::spawn(move || second.remember("second fact", "source two").unwrap());
        assert!(a.join().unwrap() > 0);
        assert!(b.join().unwrap() > 0);
        assert_eq!(manager.list_all().unwrap().len(), 2);
    }

    #[test]
    fn recall_over_ten_thousand_facts_is_under_fifty_milliseconds() {
        let dir = tempfile::tempdir().unwrap();
        let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        for index in 0..10_000 {
            manager
                .remember(&format!("fact number {index}"), "source")
                .unwrap();
        }
        let start = std::time::Instant::now();
        let matches = manager.recall("number 9999", 10).unwrap();
        assert!(!matches.is_empty());
        assert!(start.elapsed() < std::time::Duration::from_millis(50));
    }

    #[test]
    fn recall_zero_limit_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        manager.remember("tea", "source").unwrap();
        assert!(manager.recall("tea", 0).unwrap().is_empty());
    }

    #[test]
    fn recall_returns_fact_with_overlapping_word() {
        let dir = tempfile::tempdir().unwrap();
        let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        manager
            .remember("prefers tea over coffee", "source")
            .unwrap();
        assert_eq!(manager.recall("tea", 10).unwrap().len(), 1);
    }

    #[test]
    fn list_all_reads_stored_fact() {
        let dir = tempfile::tempdir().unwrap();
        let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        manager
            .remember("prefers tea", "remember that I prefer tea")
            .unwrap();
        let facts = manager.list_all().unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].text, "prefers tea");
        assert_eq!(facts[0].source_transcript, "remember that I prefer tea");
    }

    #[test]
    fn remember_returns_positive_id() {
        let dir = tempfile::tempdir().unwrap();
        let manager = MemoryManager::open(dir.path().join("memory.db")).unwrap();
        assert!(
            manager
                .remember("prefers tea", "remember that I prefer tea")
                .unwrap()
                > 0
        );
    }

    #[test]
    fn extracts_remember_i_trigger() {
        assert_eq!(
            MemoryManager::extract_fact("remember I prefer tea").as_deref(),
            Some("prefer tea")
        );
    }

    #[test]
    fn extracts_remember_my_trigger() {
        assert_eq!(
            MemoryManager::extract_fact("remember my address is Main Street").as_deref(),
            Some("address is Main Street")
        );
    }

    #[test]
    fn extracts_fact_from_remember_that_phrase() {
        assert_eq!(
            MemoryManager::extract_fact("remember that I prefer tea").as_deref(),
            Some("I prefer tea")
        );
    }

    #[test]
    fn extracts_mixed_case_trigger_and_preserves_fact_case() {
        assert_eq!(
            MemoryManager::extract_fact("Remember That I Prefer Tea").as_deref(),
            Some("I Prefer Tea")
        );
    }

    #[test]
    fn extracts_dont_forget_trigger() {
        assert_eq!(
            MemoryManager::extract_fact("don't forget that the garage code is 4521").as_deref(),
            Some("the garage code is 4521")
        );
    }

    #[test]
    fn extracts_note_that_trigger() {
        assert_eq!(
            MemoryManager::extract_fact("note that my dentist appointment is Tuesday").as_deref(),
            Some("my dentist appointment is Tuesday")
        );
    }

    #[test]
    fn ignores_transcript_without_trigger() {
        assert_eq!(
            MemoryManager::extract_fact("what's the weather today"),
            None
        );
    }

    #[test]
    fn ignores_empty_transcript() {
        assert_eq!(MemoryManager::extract_fact(""), None);
    }

    #[test]
    fn ignores_trigger_with_no_fact_text() {
        assert_eq!(MemoryManager::extract_fact("remember"), None);
    }

    #[test]
    fn trims_whitespace_around_extracted_fact() {
        assert_eq!(
            MemoryManager::extract_fact("   remember that   i like coffee  ").as_deref(),
            Some("i like coffee")
        );
    }

    #[test]
    fn extracts_trigger_after_leading_words() {
        assert_eq!(
            MemoryManager::extract_fact("please remember that i like coffee").as_deref(),
            Some("i like coffee")
        );
    }

    #[test]
    fn strips_only_the_first_trigger_prefix() {
        assert_eq!(
            MemoryManager::extract_fact("remember that remember that nested").as_deref(),
            Some("remember that nested")
        );
    }
}
