use serde::{Deserialize, Serialize};
use crate::db;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub total_transcriptions: i64,
    pub total_words: i64,
    pub transcriptions_today: i64,
    pub words_today: i64,
    pub average_words_per_transcription: f64,
}

pub fn compute_stats() -> Result<Stats, String> {
    let db_stats = db::get_db_stats()?;

    let average = if db_stats.total_transcriptions > 0 {
        db_stats.total_words as f64 / db_stats.total_transcriptions as f64
    } else {
        0.0
    };

    Ok(Stats {
        total_transcriptions: db_stats.total_transcriptions,
        total_words: db_stats.total_words,
        transcriptions_today: db_stats.transcriptions_today,
        words_today: db_stats.words_today,
        average_words_per_transcription: average,
    })
}

#[tauri::command]
pub fn get_stats() -> Result<Stats, String> {
    compute_stats()
}
