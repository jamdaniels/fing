use once_cell::sync::Lazy;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// Global database connection (SQLite with FTS5).
static DB: Lazy<Mutex<Option<Connection>>> = Lazy::new(|| Mutex::new(None));

/// Maximum allowed FTS5 query length
const MAX_FTS_QUERY_LENGTH: usize = 500;
const DEFAULT_PAGE_LIMIT: i64 = 25;
const MAX_PAGE_LIMIT: i64 = 200;
const MAX_PAGE_OFFSET: i64 = 10_000;

const STOP_WORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for", "of", "with", "by",
    "from", "is", "it", "that", "this", "be", "are", "was", "were", "been", "have", "has", "had",
    "do", "does", "did", "will", "would", "could", "should", "may", "might", "can", "just", "so",
    "like", "if", "then", "than", "when", "what", "which", "who", "how", "all", "each", "every",
    "both", "few", "more", "most", "other", "some", "such", "no", "not", "only", "same", "too",
    "very", "as", "into", "through", "during", "before", "after", "above", "below", "up", "down",
    "out", "off", "over", "under", "again", "further", "once", "here", "there", "where", "why",
    "any", "about", "because", "also", "get", "got", "going", "go", "know", "think", "want",
    "need", "make", "see", "look", "come", "back", "now", "way", "well", "even", "new", "take",
    "use", "your", "our", "their", "my", "its", "you", "we", "they", "he", "she", "him", "her",
    "his", "them", "i", "me", "us", "yeah", "yes", "okay", "ok", "um", "uh", "ah", "oh", "hmm",
    "actually", "really",
];

static STOP_WORD_SET: Lazy<HashSet<&'static str>> =
    Lazy::new(|| STOP_WORDS.iter().copied().collect());

/// Sanitize FTS5 search query to prevent injection
fn sanitize_fts5_query(query: &str) -> String {
    // Truncate to max length
    let truncated = if query.len() > MAX_FTS_QUERY_LENGTH {
        &query[..MAX_FTS_QUERY_LENGTH]
    } else {
        query
    };

    // Remove FTS5 special operators and escape quotes
    let mut result = String::with_capacity(truncated.len());
    let chars = truncated.chars();

    for c in chars {
        match c {
            // Skip FTS5 operators and special characters
            '*' | '^' | ':' | '(' | ')' | '{' | '}' | '[' | ']' => continue,
            // Escape double quotes
            '"' => result.push_str("\"\""),
            // Handle potential keywords - check if at word boundary
            _ => {
                result.push(c);
            }
        }
    }

    // Remove FTS5 keywords (AND, OR, NOT, NEAR) at word boundaries
    let result = result
        .split_whitespace()
        .filter(|word| {
            let upper = word.to_uppercase();
            !matches!(upper.as_str(), "AND" | "OR" | "NOT" | "NEAR")
        })
        .collect::<Vec<_>>()
        .join(" ");

    // Wrap in quotes for phrase search (safer)
    if result.trim().is_empty() {
        String::new()
    } else {
        format!("\"{result}\"")
    }
}

fn sanitize_pagination(limit: i64, offset: i64) -> (i64, i64) {
    let limit = if limit <= 0 {
        DEFAULT_PAGE_LIMIT
    } else {
        limit.min(MAX_PAGE_LIMIT)
    };
    let offset = offset.clamp(0, MAX_PAGE_OFFSET);
    (limit, offset)
}

fn extract_term_counts(text: &str) -> HashMap<String, i64> {
    let mut word_counts: HashMap<String, i64> = HashMap::new();

    for word in text.split_whitespace() {
        let clean: String = word
            .chars()
            .filter(|c| c.is_alphabetic())
            .collect::<String>()
            .to_lowercase();

        if clean.len() < 3 || STOP_WORD_SET.contains(clean.as_str()) {
            continue;
        }
        *word_counts.entry(clean).or_insert(0) += 1;
    }

    word_counts
}

fn insert_transcript_terms(
    conn: &Connection,
    transcript_id: i64,
    created_at: &str,
    term_counts: &HashMap<String, i64>,
) -> Result<(), rusqlite::Error> {
    if term_counts.is_empty() {
        return Ok(());
    }

    let mut insert_stmt = conn.prepare(
        "INSERT OR REPLACE INTO transcript_terms (transcript_id, created_at, word, count)
         VALUES (?1, ?2, ?3, ?4)",
    )?;

    for (word, count) in term_counts {
        insert_stmt.execute(params![transcript_id, created_at, word, count])?;
    }

    Ok(())
}

fn backfill_recent_term_index(conn: &Connection) -> Result<u64, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.text, t.created_at
         FROM transcripts t
         WHERE t.created_at >= datetime('now', '-30 days')
           AND NOT EXISTS (
             SELECT 1 FROM transcript_terms tt WHERE tt.transcript_id = t.id
           )",
    )?;

    let missing_rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    for (id, text, created_at) in &missing_rows {
        let term_counts = extract_term_counts(text);
        insert_transcript_terms(conn, *id, created_at, &term_counts)?;
    }

    Ok(missing_rows.len() as u64)
}

/// Initialize the database, creating tables and FTS5 index if needed.
pub fn init_db() -> Result<(), String> {
    let path = crate::paths::db_path().ok_or_else(|| "App paths not initialized".to_string())?;

    // Ensure directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create database directory: {e}"))?;
    }

    let conn = Connection::open(&path).map_err(|e| format!("Failed to open database: {e}"))?;

    if let Err(e) = conn.execute("PRAGMA foreign_keys=ON", []) {
        tracing::warn!("Could not enable foreign key enforcement: {}", e);
    }

    // Enable WAL mode for better reliability (non-fatal if fails)
    if let Err(e) = conn.execute("PRAGMA journal_mode=WAL", []) {
        tracing::warn!("Could not enable WAL mode: {}", e);
    }

    // Create main transcripts table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS transcripts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            text TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            duration_ms INTEGER NOT NULL DEFAULT 0,
            app_context TEXT,
            word_count INTEGER NOT NULL DEFAULT 0
        )",
        [],
    )
    .map_err(|e| format!("Failed to create transcripts table: {e}"))?;

    // Create FTS5 virtual table for full-text search
    conn.execute(
        "CREATE VIRTUAL TABLE IF NOT EXISTS transcripts_fts USING fts5(
            text,
            content='transcripts',
            content_rowid='id'
        )",
        [],
    )
    .map_err(|e| format!("Failed to create FTS table: {e}"))?;

    // Create triggers to keep FTS in sync
    conn.execute_batch(
        "
        CREATE TRIGGER IF NOT EXISTS transcripts_ai AFTER INSERT ON transcripts BEGIN
            INSERT INTO transcripts_fts(rowid, text) VALUES (new.id, new.text);
        END;

        CREATE TRIGGER IF NOT EXISTS transcripts_ad AFTER DELETE ON transcripts BEGIN
            INSERT INTO transcripts_fts(transcripts_fts, rowid, text) VALUES ('delete', old.id, old.text);
        END;

        CREATE TRIGGER IF NOT EXISTS transcripts_au AFTER UPDATE ON transcripts BEGIN
            INSERT INTO transcripts_fts(transcripts_fts, rowid, text) VALUES ('delete', old.id, old.text);
            INSERT INTO transcripts_fts(rowid, text) VALUES (new.id, new.text);
        END;
        "
    )
    .map_err(|e| format!("Failed to create triggers: {e}"))?;

    // Create index for faster date queries
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_transcripts_created_at ON transcripts(created_at)",
        [],
    )
    .map_err(|e| format!("Failed to create index: {e}"))?;

    // Pre-tokenized terms for fast top-word stats queries.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS transcript_terms (
            transcript_id INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            word TEXT NOT NULL,
            count INTEGER NOT NULL,
            PRIMARY KEY (transcript_id, word),
            FOREIGN KEY (transcript_id) REFERENCES transcripts(id) ON DELETE CASCADE
        )",
        [],
    )
    .map_err(|e| format!("Failed to create transcript_terms table: {e}"))?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_transcript_terms_created_at
         ON transcript_terms(created_at)",
        [],
    )
    .map_err(|e| format!("Failed to create transcript_terms created_at index: {e}"))?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_transcript_terms_word
         ON transcript_terms(word)",
        [],
    )
    .map_err(|e| format!("Failed to create transcript_terms word index: {e}"))?;

    match backfill_recent_term_index(&conn) {
        Ok(0) => {}
        Ok(n) => tracing::info!("Backfilled term index for {n} transcripts"),
        Err(e) => tracing::warn!("Failed to backfill term index: {e}"),
    }

    let mut db = DB
        .lock()
        .map_err(|e| format!("Database lock poisoned: {e}"))?;
    *db = Some(conn);

    tracing::info!("Database initialized at {:?}", path);
    Ok(())
}

/// Delete transcripts older than 30 days.
pub fn prune_old_transcripts() -> Result<u64, String> {
    with_db(|conn| {
        let deleted = conn.execute(
            "DELETE FROM transcripts WHERE created_at < datetime('now', '-30 days')",
            [],
        )?;
        Ok(deleted as u64)
    })
}

fn with_db<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce(&Connection) -> Result<T, rusqlite::Error>,
{
    let db = DB
        .lock()
        .map_err(|e| format!("Database lock poisoned: {e}"))?;
    let conn = db.as_ref().ok_or("Database not initialized")?;
    f(conn).map_err(|e| e.to_string())
}

/// A saved transcription record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub id: i64,
    pub text: String,
    pub created_at: String,
    pub duration_ms: i64,
    pub app_context: Option<String>,
    pub word_count: i64,
}

/// Data for creating a new transcription record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTranscript {
    pub text: String,
    pub duration_ms: i64,
    pub app_context: Option<String>,
}

/// Save a new transcription to the database.
pub fn save_transcript(transcript: &NewTranscript) -> Result<Transcript, String> {
    let word_count = transcript.text.split_whitespace().count() as i64;

    with_db(|conn| {
        conn.execute(
            "INSERT INTO transcripts (text, duration_ms, app_context, word_count) VALUES (?1, ?2, ?3, ?4)",
            params![transcript.text, transcript.duration_ms, transcript.app_context, word_count],
        )?;

        let id = conn.last_insert_rowid();

        let mut stmt = conn.prepare(
            "SELECT id, text, created_at, duration_ms, app_context, word_count
             FROM transcripts WHERE id = ?1",
        )?;
        let transcript = stmt.query_row([id], |row| {
            Ok(Transcript {
                id: row.get(0)?,
                text: row.get(1)?,
                created_at: row.get(2)?,
                duration_ms: row.get(3)?,
                app_context: row.get(4)?,
                word_count: row.get(5)?,
            })
        })?;

        let term_counts = extract_term_counts(&transcript.text);
        if let Err(e) =
            insert_transcript_terms(conn, transcript.id, &transcript.created_at, &term_counts)
        {
            tracing::warn!("Failed to index transcript terms for stats: {}", e);
        }

        Ok(transcript)
    })
}

/// Get recent transcripts ordered by date descending.
pub fn get_recent_transcripts(limit: i64, offset: i64) -> Result<Vec<Transcript>, String> {
    let (limit, offset) = sanitize_pagination(limit, offset);

    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, text, created_at, duration_ms, app_context, word_count
             FROM transcripts
             ORDER BY created_at DESC
             LIMIT ?1 OFFSET ?2",
        )?;

        let rows = stmt.query_map(params![limit, offset], |row| {
            Ok(Transcript {
                id: row.get(0)?,
                text: row.get(1)?,
                created_at: row.get(2)?,
                duration_ms: row.get(3)?,
                app_context: row.get(4)?,
                word_count: row.get(5)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
    })
}

/// Full-text search transcripts using FTS5.
pub fn search_transcripts(query: &str, limit: i64, offset: i64) -> Result<Vec<Transcript>, String> {
    let (limit, offset) = sanitize_pagination(limit, offset);

    // Sanitize the FTS5 query
    let sanitized_query = sanitize_fts5_query(query);
    if sanitized_query.is_empty() {
        return Ok(Vec::new());
    }

    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT t.id, t.text, t.created_at, t.duration_ms, t.app_context, t.word_count
             FROM transcripts t
             JOIN transcripts_fts fts ON t.id = fts.rowid
             WHERE transcripts_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2 OFFSET ?3",
        )?;

        let rows = stmt.query_map(params![sanitized_query, limit, offset], |row| {
            Ok(Transcript {
                id: row.get(0)?,
                text: row.get(1)?,
                created_at: row.get(2)?,
                duration_ms: row.get(3)?,
                app_context: row.get(4)?,
                word_count: row.get(5)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
    })
}

/// Delete a single transcript by ID.
pub fn delete_transcript(id: i64) -> Result<(), String> {
    with_db(|conn| {
        conn.execute("DELETE FROM transcripts WHERE id = ?1", [id])?;
        Ok(())
    })
}

/// Delete all transcripts (clear history).
pub fn delete_all_transcripts() -> Result<(), String> {
    with_db(|conn| {
        conn.execute("DELETE FROM transcripts", [])?;
        Ok(())
    })
}

/// Aggregate statistics from the transcript database.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbStats {
    pub total_transcriptions: i64,
    pub total_words: i64,
    pub transcriptions_today: i64,
    pub words_today: i64,
    pub average_wpm: f64,
    pub top_words: Vec<(String, i64)>,
}

pub fn get_db_stats() -> Result<DbStats, String> {
    with_db(|conn| {
        let total_transcriptions: i64 = conn.query_row(
            "SELECT COUNT(*) FROM transcripts WHERE created_at >= datetime('now', '-30 days')",
            [],
            |row| row.get(0),
        )?;

        let total_words: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(word_count), 0) FROM transcripts WHERE created_at >= datetime('now', '-30 days')",
                [],
                |row| row.get(0),
            )?;

        let transcriptions_today: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transcripts WHERE date(created_at, 'localtime') = date('now', 'localtime')",
                [],
                |row| row.get(0),
            )?;

        let words_today: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(word_count), 0) FROM transcripts WHERE date(created_at, 'localtime') = date('now', 'localtime')",
                [],
                |row| row.get(0),
            )?;

        // Average WPM: total words / total duration in minutes (last 30 days)
        let average_wpm: f64 = conn
            .query_row(
                "SELECT COALESCE(SUM(word_count), 0), COALESCE(SUM(duration_ms), 0) FROM transcripts WHERE created_at >= datetime('now', '-30 days')",
                [],
                |row| {
                    let words: i64 = row.get(0)?;
                    let duration_ms: i64 = row.get(1)?;
                    if duration_ms > 0 {
                        Ok(words as f64 / (duration_ms as f64 / 60000.0))
                    } else {
                        Ok(0.0)
                    }
                },
            )?;

        // Top words from last 30 days
        let top_words = get_top_words(conn, 5)?;

        Ok(DbStats {
            total_transcriptions,
            total_words,
            transcriptions_today,
            words_today,
            average_wpm,
            top_words,
        })
    })
}

fn get_top_words(
    conn: &rusqlite::Connection,
    limit: usize,
) -> Result<Vec<(String, i64)>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT word, SUM(count) AS total_count
         FROM transcript_terms
         WHERE created_at >= datetime('now', '-30 days')
         GROUP BY word
         ORDER BY total_count DESC, word ASC
         LIMIT ?1",
    )?;

    let rows = stmt.query_map([limit as i64], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect::<Result<Vec<_>, _>>()
}

// Tauri commands
#[tauri::command]
pub fn db_save_transcript(transcript: NewTranscript) -> Result<Transcript, String> {
    save_transcript(&transcript)
}

#[tauri::command]
pub fn db_get_recent(limit: i64, offset: i64) -> Result<Vec<Transcript>, String> {
    get_recent_transcripts(limit, offset)
}

#[tauri::command]
pub fn db_search(query: String, limit: i64, offset: i64) -> Result<Vec<Transcript>, String> {
    search_transcripts(&query, limit, offset)
}

#[tauri::command]
pub fn db_delete(id: i64) -> Result<(), String> {
    delete_transcript(id)
}

#[tauri::command]
pub fn db_delete_all() -> Result<(), String> {
    delete_all_transcripts()
}

#[cfg(test)]
mod tests {
    use super::{extract_term_counts, sanitize_pagination};

    #[test]
    fn sanitize_pagination_clamps_limit_and_offset() {
        assert_eq!(sanitize_pagination(25, 0), (25, 0));
        assert_eq!(sanitize_pagination(0, -10), (25, 0));
        assert_eq!(sanitize_pagination(-5, 5), (25, 5));
        assert_eq!(sanitize_pagination(10_000, 50_000), (200, 10_000));
    }

    #[test]
    fn extract_term_counts_filters_short_words_and_stop_words() {
        let counts = extract_term_counts("The quick brown fox and THE fox run! run, it.");

        assert_eq!(counts.get("quick"), Some(&1));
        assert_eq!(counts.get("brown"), Some(&1));
        assert_eq!(counts.get("fox"), Some(&2));
        assert_eq!(counts.get("run"), Some(&2));
        assert_eq!(counts.get("the"), None);
        assert_eq!(counts.get("and"), None);
        assert_eq!(counts.get("it"), None);
    }
}
