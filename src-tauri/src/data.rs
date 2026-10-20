//! Task export/import (Quest Ops JSON) and the danger-zone history actions.

use std::collections::HashSet;

use rusqlite::{Connection, Transaction, params};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::{
    db::Db,
    error::{AppError, AppResult},
    priority, quests,
};

const FORMAT: &str = "quest-ops-tasks";
const VERSION: i64 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportStep {
    pub title: String,
    #[serde(default)]
    pub done_when: String,
    #[serde(default = "default_effort")]
    pub effort: i64,
    #[serde(default = "default_minutes")]
    pub est_minutes: i64,
    #[serde(default = "default_step_status")]
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportQuest {
    pub title: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default = "default_quest_status")]
    pub status: String,
    #[serde(default)]
    pub deadline: Option<String>,
    #[serde(default)]
    pub importance: Option<i64>,
    #[serde(default)]
    pub urgency_base: Option<i64>,
    #[serde(default)]
    pub rationale: String,
    #[serde(default)]
    pub steps: Vec<ExportStep>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFile {
    pub format: String,
    pub version: i64,
    #[serde(default)]
    pub exported_at: String,
    pub quests: Vec<ExportQuest>,
}

fn default_effort() -> i64 {
    2
}
fn default_minutes() -> i64 {
    15
}
fn default_step_status() -> String {
    "pending".into()
}
fn default_quest_status() -> String {
    "inbox".into()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferResult {
    pub quests: i64,
    pub steps: i64,
    /// Import only: operations skipped as duplicates or finished work.
    pub skipped: i64,
}

fn collect(conn: &Connection) -> AppResult<ExportFile> {
    let mut qs = conn.prepare(
        "SELECT q.id, q.title, q.notes, c.name, q.status, q.deadline, q.importance,
                q.urgency_base, q.ai_rationale
         FROM quest q LEFT JOIN category c ON c.id = q.category_id
         WHERE q.status IN ('inbox', 'active', 'completed')
         ORDER BY q.id",
    )?;
    let mut ss = conn.prepare(
        "SELECT title, done_when, effort, est_minutes, status FROM step
         WHERE quest_id = ?1 AND parent_step_id IS NULL ORDER BY order_index",
    )?;
    let rows = qs
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                ExportQuest {
                    title: r.get(1)?,
                    notes: r.get(2)?,
                    category: r.get(3)?,
                    status: r.get(4)?,
                    deadline: r.get(5)?,
                    importance: r.get(6)?,
                    urgency_base: r.get(7)?,
                    rationale: r.get(8)?,
                    steps: Vec::new(),
                },
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut quests_out = Vec::new();
    for (id, mut q) in rows {
        q.steps = ss
            .query_map([id], |r| {
                Ok(ExportStep {
                    title: r.get(0)?,
                    done_when: r.get(1)?,
                    effort: r.get(2)?,
                    est_minutes: r.get(3)?,
                    status: r.get::<_, String>(4)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        quests_out.push(q);
    }
    let exported_at: String = conn.query_row("SELECT datetime('now', 'localtime')", [], |r| r.get(0))?;
    Ok(ExportFile { format: FORMAT.into(), version: VERSION, exported_at, quests: quests_out })
}

#[tauri::command]
pub fn export_tasks(db: State<'_, Db>, path: String) -> AppResult<TransferResult> {
    let file = collect(&*quests::lock(&db)?)?;
    let steps = file.quests.iter().map(|q| q.steps.len() as i64).sum();
    let json = serde_json::to_string_pretty(&file)
        .map_err(|e| AppError::Msg(format!("could not encode export: {e}")))?;
    std::fs::write(&path, json).map_err(|e| AppError::Msg(format!("could not write {path}: {e}")))?;
    Ok(TransferResult { quests: file.quests.len() as i64, steps, skipped: 0 })
}

/// Adds the file's inbox and active operations. Finished operations and
/// operations whose title already exists among open ones are skipped.
pub fn import_into(conn: &mut Connection, file: ExportFile) -> AppResult<(TransferResult, Vec<i64>)> {
    if file.format != FORMAT {
        return Err(AppError::Msg("not a Quest Ops task export".into()));
    }
    if file.version > VERSION {
        return Err(AppError::Msg("this export comes from a newer Quest Ops version".into()));
    }
    let mut existing: HashSet<String> = conn
        .prepare("SELECT lower(title) FROM quest WHERE status IN ('inbox', 'active')")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;

    let tx = conn.transaction()?;
    let (mut quests_n, mut steps_n, mut skipped) = (0, 0, 0);
    let mut to_score = Vec::new();
    for q in file.quests {
        let title = q.title.trim();
        let status = if q.status == "active" && !q.steps.is_empty() { "active" } else { "inbox" };
        if title.is_empty() || q.status == "completed" || !existing.insert(title.to_lowercase()) {
            skipped += 1;
            continue;
        }
        let category_id: Option<i64> = match q.category.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => {
                tx.execute("INSERT OR IGNORE INTO category (name) VALUES (?1)", [name])?;
                Some(tx.query_row("SELECT id FROM category WHERE name = ?1", [name], |r| r.get(0))?)
            }
            _ => None,
        };
        let score = |v: Option<i64>| v.filter(|n| (1..=5).contains(n));
        tx.execute(
            "INSERT INTO quest (title, notes, category_id, status, deadline, importance,
                                urgency_base, ai_rationale)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                title,
                q.notes.trim(),
                category_id,
                status,
                q.deadline.as_deref().map(str::trim).filter(|d| !d.is_empty()),
                score(q.importance),
                score(q.urgency_base),
                q.rationale.trim(),
            ],
        )?;
        let quest_id = tx.last_insert_rowid();
        quests_n += 1;
        if status == "active" && score(q.importance).is_none() {
            to_score.push(quest_id);
        }
        for (i, s) in q.steps.iter().enumerate() {
            if s.title.trim().is_empty() {
                continue;
            }
            let effort = if [1, 2, 3, 5].contains(&s.effort) { s.effort } else { 2 };
            let step_status = if s.status == "done" { "done" } else { "pending" };
            tx.execute(
                "INSERT INTO step (quest_id, title, done_when, effort, est_minutes, order_index,
                                   status, completed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
                         CASE WHEN ?7 = 'done' THEN datetime('now') END)",
                params![
                    quest_id,
                    s.title.trim(),
                    s.done_when.trim(),
                    effort,
                    s.est_minutes.clamp(1, 240),
                    i as i64,
                    step_status
                ],
            )?;
            steps_n += 1;
        }
    }
    tx.commit()?;
    Ok((TransferResult { quests: quests_n, steps: steps_n, skipped }, to_score))
}

#[tauri::command]
pub fn import_tasks(app: AppHandle, db: State<'_, Db>, path: String) -> AppResult<TransferResult> {
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| AppError::Msg(format!("could not read {path}: {e}")))?;
    let file: ExportFile = serde_json::from_str(&raw)
        .map_err(|e| AppError::Msg(format!("not a valid Quest Ops export: {e}")))?;
    let (result, to_score) = {
        let mut conn = quests::lock(&db)?;
        import_into(&mut conn, file)?
    };
    priority::schedule(&app, to_score);
    Ok(result)
}

// ------------------------------------------------------------ danger zone

/// What a danger-zone action removed, so the interface can report it.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearSummary {
    pub operations: i64,
    pub objectives: i64,
    pub runs: i64,
    /// Reset only: XP total before the reset.
    pub xp: i64,
    /// Reset only: achievements removed (tiers and specials).
    pub achievements: i64,
}

/// Deletes finished work: completed operations, completed objectives, ended
/// timer runs, day plans and debriefs. XP, level, streak and achievements stay
/// (they derive from the XP log, which is kept). Focus time is banked first.
fn clear_finished_tx(tx: &Transaction) -> AppResult<ClearSummary> {
    tx.execute(
        "UPDATE profile SET archived_focus_ms = archived_focus_ms + (
             SELECT COALESCE(SUM(ended_ms - started_ms - paused_ms), 0)
             FROM timer_session WHERE outcome IN ('on_time', 'overtime'))",
        [],
    )?;
    let objectives: i64 =
        tx.query_row("SELECT COUNT(*) FROM step WHERE status = 'done'", [], |r| r.get(0))?;
    let runs = tx.execute("DELETE FROM timer_session WHERE ended_at IS NOT NULL", [])? as i64;
    let operations = tx.execute("DELETE FROM quest WHERE status = 'completed'", [])? as i64;
    tx.execute("DELETE FROM step WHERE status = 'done'", [])?;
    tx.execute("DELETE FROM daily_plan", [])?;
    tx.execute("DELETE FROM daily_debrief", [])?;
    Ok(ClearSummary { operations, objectives, runs, ..Default::default() })
}

pub fn clear_finished_in(conn: &mut Connection) -> AppResult<ClearSummary> {
    let tx = conn.transaction()?;
    let summary = clear_finished_tx(&tx)?;
    tx.commit()?;
    Ok(summary)
}

/// Resets the whole app in one transaction: every task and objective
/// (including a running timer) and all progress. Settings, the API key and
/// categories are kept.
pub fn reset_progress_in(conn: &mut Connection) -> AppResult<ClearSummary> {
    let tx = conn.transaction()?;
    let count = |sql: &str| -> AppResult<i64> { Ok(tx.query_row(sql, [], |r| r.get(0))?) };
    let mut summary = ClearSummary {
        operations: count("SELECT COUNT(*) FROM quest")?,
        objectives: count("SELECT COUNT(*) FROM step")?,
        runs: count("SELECT COUNT(*) FROM timer_session")?,
        xp: count("SELECT total_xp FROM profile WHERE id = 1")?,
        achievements: 0,
    };
    // Quests cascade to steps, timer sessions, day plans and overrides.
    tx.execute_batch(
        "DELETE FROM quest;
         DELETE FROM timer_session;
         DELETE FROM daily_plan;
         DELETE FROM daily_debrief;
         DELETE FROM priority_override;
         DELETE FROM xp_event;",
    )?;
    summary.achievements = tx.execute("DELETE FROM achievement", [])? as i64;
    tx.execute("UPDATE category SET xp = 0", [])?;
    tx.execute(
        "UPDATE profile SET total_xp = 0, current_streak = 0, longest_streak = 0,
                            calibration_json = '{}', archived_focus_ms = 0",
        [],
    )?;
    tx.commit()?;
    Ok(summary)
}

#[tauri::command]
pub fn clear_finished(db: State<'_, Db>) -> AppResult<ClearSummary> {
    clear_finished_in(&mut *quests::lock(&db)?)
}

#[tauri::command]
pub fn reset_progress(db: State<'_, Db>) -> AppResult<ClearSummary> {
    reset_progress_in(&mut *quests::lock(&db)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrate_for_test(&mut conn);
        conn.execute_batch(
            "INSERT INTO quest (id, title, status, category_id, deadline, importance, urgency_base)
                 VALUES (1, 'Write chapter', 'active', 2, '2026-12-01', 4, 1),
                        (2, 'Old thing', 'completed', NULL, NULL, NULL, NULL),
                        (3, 'Someday idea', 'inbox', NULL, NULL, NULL, NULL);
             INSERT INTO step (id, quest_id, title, effort, est_minutes, order_index, status)
                 VALUES (1, 1, 'Outline', 2, 15, 0, 'done'),
                        (2, 1, 'Draft intro', 3, 20, 1, 'pending'),
                        (3, 2, 'Done long ago', 1, 10, 0, 'done');
             INSERT INTO timer_session (step_id, started_at, started_ms, planned_seconds,
                                        ended_at, ended_ms, outcome)
                 VALUES (1, datetime('now'), 0, 900, datetime('now'), 7200000, 'on_time');
             INSERT INTO xp_event (quest_id, step_id, amount, reason)
                 VALUES (1, 1, 20, 'objective'), (2, 3, 10, 'objective'), (2, NULL, 25, 'operation');
             UPDATE profile SET total_xp = 55;",
        )
        .unwrap();
        conn
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn export_then_import_round_trips_open_tasks() {
        let src = seeded();
        let file = collect(&src).unwrap();
        assert_eq!(file.quests.len(), 3);
        let json = serde_json::to_string(&file).unwrap();

        let mut dst = Connection::open_in_memory().unwrap();
        crate::db::migrate_for_test(&mut dst);
        let (r, to_score) = import_into(&mut dst, serde_json::from_str(&json).unwrap()).unwrap();
        assert_eq!((r.quests, r.steps, r.skipped), (2, 2, 1), "completed op skipped");
        assert!(to_score.is_empty(), "scores carried over");
        assert_eq!(
            count(&dst, "SELECT COUNT(*) FROM quest WHERE title = 'Write chapter'
                         AND status = 'active' AND deadline = '2026-12-01' AND importance = 4
                         AND category_id = (SELECT id FROM category WHERE name = 'Writing')"),
            1
        );
        assert_eq!(count(&dst, "SELECT COUNT(*) FROM step WHERE status = 'done'"), 1);

        // Importing the same file again adds nothing.
        let (again, _) = import_into(&mut dst, serde_json::from_str(&json).unwrap()).unwrap();
        assert_eq!((again.quests, again.skipped), (0, 3));
    }

    #[test]
    fn import_rejects_other_files() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrate_for_test(&mut conn);
        let bad = ExportFile { format: "other".into(), version: 1, exported_at: String::new(), quests: vec![] };
        assert!(import_into(&mut conn, bad).is_err());
    }

    #[test]
    fn reset_wipes_tasks_and_progress_but_keeps_settings() {
        let mut conn = seeded();
        conn.execute("INSERT INTO achievement (id) VALUES ('objectives:1')", []).unwrap();
        // A run in progress is wiped too.
        conn.execute(
            "INSERT INTO timer_session (step_id, started_at, started_ms, planned_seconds)
             VALUES (2, datetime('now'), 0, 1200)",
            [],
        )
        .unwrap();
        conn.execute("UPDATE profile SET goals_text = 'Finish thesis', model_name = 'm'", [])
            .unwrap();
        let s = reset_progress_in(&mut conn).unwrap();
        assert_eq!((s.operations, s.objectives, s.runs, s.xp, s.achievements), (3, 3, 2, 55, 1));
        for table in ["quest", "step", "timer_session", "xp_event", "achievement", "daily_plan"] {
            assert_eq!(count(&conn, &format!("SELECT COUNT(*) FROM {table}")), 0, "{table}");
        }
        assert_eq!(count(&conn, "SELECT total_xp FROM profile"), 0);
        assert_eq!(count(&conn, "SELECT archived_focus_ms FROM profile"), 0);
        let goals: String =
            conn.query_row("SELECT goals_text FROM profile", [], |r| r.get(0)).unwrap();
        assert_eq!(goals, "Finish thesis", "settings survive a reset");
    }

    #[test]
    fn clear_finished_keeps_xp_and_open_tasks() {
        let mut conn = seeded();
        let s = clear_finished_in(&mut conn).unwrap();
        assert_eq!((s.operations, s.objectives, s.runs, s.xp), (1, 2, 1, 0));
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM quest"), 2);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM step"), 1);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM timer_session"), 0);
        assert_eq!(count(&conn, "SELECT total_xp FROM profile"), 55);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM xp_event"), 3);
        assert_eq!(count(&conn, "SELECT archived_focus_ms FROM profile"), 7200000);
    }
}
