use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use once_cell::sync::Lazy;

static DB: Lazy<Mutex<Option<Connection>>> = Lazy::new(|| Mutex::new(None));

fn get_db_path() -> PathBuf {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fing");
    data_dir.join("fing.db")
}

pub fn init_db() -> Result<(), String> {
    let path = get_db_path();

    // Ensure directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create database directory: {}", e))?;
    }

    let conn = Connection::open(&path)
        .map_err(|e| format!("Failed to open database: {}", e))?;

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
    .map_err(|e| format!("Failed to create transcripts table: {}", e))?;

    // Create FTS5 virtual table for full-text search
    conn.execute(
        "CREATE VIRTUAL TABLE IF NOT EXISTS transcripts_fts USING fts5(
            text,
            content='transcripts',
            content_rowid='id'
        )",
        [],
    )
    .map_err(|e| format!("Failed to create FTS table: {}", e))?;

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
    .map_err(|e| format!("Failed to create triggers: {}", e))?;

    // Create index for faster date queries
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_transcripts_created_at ON transcripts(created_at)",
        [],
    )
    .map_err(|e| format!("Failed to create index: {}", e))?;

    let mut db = DB.lock().unwrap();
    *db = Some(conn);

    tracing::info!("Database initialized at {:?}", path);
    Ok(())
}

fn with_db<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce(&Connection) -> Result<T, rusqlite::Error>,
{
    let db = DB.lock().unwrap();
    let conn = db.as_ref().ok_or("Database not initialized")?;
    f(conn).map_err(|e| e.to_string())
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTranscript {
    pub text: String,
    pub duration_ms: i64,
    pub app_context: Option<String>,
}

pub fn save_transcript(transcript: &NewTranscript) -> Result<Transcript, String> {
    let word_count = transcript.text.split_whitespace().count() as i64;

    with_db(|conn| {
        conn.execute(
            "INSERT INTO transcripts (text, duration_ms, app_context, word_count) VALUES (?1, ?2, ?3, ?4)",
            params![transcript.text, transcript.duration_ms, transcript.app_context, word_count],
        )?;

        let id = conn.last_insert_rowid();

        let mut stmt = conn.prepare("SELECT id, text, created_at, duration_ms, app_context, word_count FROM transcripts WHERE id = ?1")?;
        stmt.query_row([id], |row| {
            Ok(Transcript {
                id: row.get(0)?,
                text: row.get(1)?,
                created_at: row.get(2)?,
                duration_ms: row.get(3)?,
                app_context: row.get(4)?,
                word_count: row.get(5)?,
            })
        })
    })
}

pub fn get_recent_transcripts(limit: i64, offset: i64) -> Result<Vec<Transcript>, String> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, text, created_at, duration_ms, app_context, word_count
             FROM transcripts
             ORDER BY created_at DESC
             LIMIT ?1 OFFSET ?2"
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

pub fn search_transcripts(query: &str, limit: i64, offset: i64) -> Result<Vec<Transcript>, String> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT t.id, t.text, t.created_at, t.duration_ms, t.app_context, t.word_count
             FROM transcripts t
             JOIN transcripts_fts fts ON t.id = fts.rowid
             WHERE transcripts_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2 OFFSET ?3"
        )?;

        let rows = stmt.query_map(params![query, limit, offset], |row| {
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

pub fn delete_transcript(id: i64) -> Result<(), String> {
    with_db(|conn| {
        conn.execute("DELETE FROM transcripts WHERE id = ?1", [id])?;
        Ok(())
    })
}

pub fn delete_all_transcripts() -> Result<(), String> {
    with_db(|conn| {
        conn.execute("DELETE FROM transcripts", [])?;
        Ok(())
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbStats {
    pub total_transcriptions: i64,
    pub total_words: i64,
    pub transcriptions_today: i64,
    pub words_today: i64,
}

pub fn get_db_stats() -> Result<DbStats, String> {
    with_db(|conn| {
        let total_transcriptions: i64 = conn
            .query_row("SELECT COUNT(*) FROM transcripts", [], |row| row.get(0))?;

        let total_words: i64 = conn
            .query_row("SELECT COALESCE(SUM(word_count), 0) FROM transcripts", [], |row| row.get(0))?;

        let transcriptions_today: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transcripts WHERE date(created_at) = date('now')",
                [],
                |row| row.get(0),
            )?;

        let words_today: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(word_count), 0) FROM transcripts WHERE date(created_at) = date('now')",
                [],
                |row| row.get(0),
            )?;

        Ok(DbStats {
            total_transcriptions,
            total_words,
            transcriptions_today,
            words_today,
        })
    })
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
