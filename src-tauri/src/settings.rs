use serde::{Deserialize, Serialize};
use tauri::State;

use crate::{
    db::Db,
    error::{AppError, AppResult},
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub commander_name: String,
    pub goals_text: String,
    pub model_name: String,
    pub small_model_name: String,
    pub api_base_url: String,
    pub max_pauses: i64,
    /// A deadline this many days away (or fewer) makes a task urgent.
    pub urgent_within_days: i64,
    /// Importance score (1 to 5) at which a task counts as important.
    pub importance_threshold: i64,
    /// Interface and AI output language: "en" or "zh".
    pub language: String,
    /// Colour theme: "dark", "light" or "system". Changed only through `set_theme`.
    #[serde(default)]
    pub theme: String,
}

pub fn load(conn: &rusqlite::Connection) -> AppResult<Settings> {
    Ok(conn.query_row(
        "SELECT commander_name, goals_text, model_name, small_model_name, api_base_url,
                max_pauses, urgent_within_days, importance_threshold, language, theme
         FROM profile WHERE id = 1",
        [],
        |r| {
            Ok(Settings {
                commander_name: r.get(0)?,
                goals_text: r.get(1)?,
                model_name: r.get(2)?,
                small_model_name: r.get(3)?,
                api_base_url: r.get(4)?,
                max_pauses: r.get(5)?,
                urgent_within_days: r.get(6)?,
                importance_threshold: r.get(7)?,
                language: r.get(8)?,
                theme: r.get(9)?,
            })
        },
    )?)
}

#[tauri::command]
pub fn get_settings(db: State<'_, Db>) -> AppResult<Settings> {
    let conn = db.0.lock().map_err(|_| AppError::Msg("database lock poisoned".into()))?;
    load(&conn)
}

#[tauri::command]
pub fn save_settings(db: State<'_, Db>, settings: Settings) -> AppResult<Settings> {
    if !(0..=10).contains(&settings.max_pauses) {
        return Err(AppError::Msg("max pauses must be between 0 and 10".into()));
    }
    if !(0..=60).contains(&settings.urgent_within_days) {
        return Err(AppError::Msg("urgent window must be 0 to 60 days".into()));
    }
    if !(2..=5).contains(&settings.importance_threshold) {
        return Err(AppError::Msg("importance threshold must be 2 to 5".into()));
    }
    if !["en", "zh"].contains(&settings.language.as_str()) {
        return Err(AppError::Msg("language must be en or zh".into()));
    }
    let conn = db.0.lock().map_err(|_| AppError::Msg("database lock poisoned".into()))?;
    conn.execute(
        "UPDATE profile SET commander_name = ?1, goals_text = ?2, model_name = ?3,
                small_model_name = ?4, api_base_url = ?5, max_pauses = ?6,
                urgent_within_days = ?7, importance_threshold = ?8, language = ?9
         WHERE id = 1",
        rusqlite::params![
            settings.commander_name.trim(),
            settings.goals_text.trim(),
            settings.model_name.trim(),
            settings.small_model_name.trim(),
            settings.api_base_url.trim().trim_end_matches('/'),
            settings.max_pauses,
            settings.urgent_within_days,
            settings.importance_threshold,
            settings.language,
        ],
    )?;
    load(&conn)
}

/// Switches language immediately, independent of the rest of the Config form.
#[tauri::command]
pub fn set_language(db: State<'_, Db>, language: String) -> AppResult<Settings> {
    if !["en", "zh"].contains(&language.as_str()) {
        return Err(AppError::Msg("language must be en or zh".into()));
    }
    let conn = db.0.lock().map_err(|_| AppError::Msg("database lock poisoned".into()))?;
    conn.execute("UPDATE profile SET language = ?1 WHERE id = 1", [language])?;
    load(&conn)
}

/// Switches the colour theme immediately.
#[tauri::command]
pub fn set_theme(db: State<'_, Db>, theme: String) -> AppResult<Settings> {
    if !["dark", "light", "system"].contains(&theme.as_str()) {
        return Err(AppError::Msg("theme must be dark, light or system".into()));
    }
    let conn = db.0.lock().map_err(|_| AppError::Msg("database lock poisoned".into()))?;
    conn.execute("UPDATE profile SET theme = ?1 WHERE id = 1", [theme])?;
    load(&conn)
}
