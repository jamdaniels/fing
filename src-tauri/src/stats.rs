use crate::db;
use serde::{Deserialize, Serialize};

/// A word and its usage count.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WordCount {
    pub word: String,
    pub count: i64,
}

/// Usage statistics for the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub total_transcriptions: i64,
    pub total_words: i64,
    pub transcriptions_today: i64,
    pub words_today: i64,
    pub average_words_per_transcription: f64,
    pub average_wpm: f64,
    pub top_words: Vec<WordCount>,
}

/// Compute usage statistics from the database.
pub fn compute_stats() -> Result<Stats, String> {
    let db_stats = db::get_db_stats()?;

    let average = if db_stats.total_transcriptions > 0 {
        db_stats.total_words as f64 / db_stats.total_transcriptions as f64
    } else {
        0.0
    };

    let top_words = db_stats
        .top_words
        .into_iter()
        .map(|(word, count)| WordCount { word, count })
        .collect();

    Ok(Stats {
        total_transcriptions: db_stats.total_transcriptions,
        total_words: db_stats.total_words,
        transcriptions_today: db_stats.transcriptions_today,
        words_today: db_stats.words_today,
        average_words_per_transcription: average,
        average_wpm: db_stats.average_wpm,
        top_words,
    })
}

#[tauri::command]
pub fn get_stats() -> Result<Stats, String> {
    compute_stats()
}
