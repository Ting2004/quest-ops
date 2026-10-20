//! Timed runs. The database is the source of truth (epoch-millisecond
//! timestamps), so a run survives the window closing or the app restarting.
//! A background ticker sends desktop notifications and updates the tray.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_notification::NotificationExt;

use crate::{
    db::Db,
    error::{AppError, AppResult},
    quests,
    rewards::{self, CompletionResult, Timing},
    settings,
};

pub const TRAY_ID: &str = "main";

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimerView {
    pub session_id: i64,
    pub step_id: i64,
    pub step_title: String,
    pub done_when: String,
    pub effort: i64,
    pub quest_id: i64,
    pub quest_title: String,
    /// The original estimate; finishing inside it earns the on-time bonus.
    pub planned_seconds: i64,
    /// Extra time granted through the overtime protocol.
    pub extended_seconds: i64,
    pub started_ms: i64,
    pub paused_ms: i64,
    pub paused_since_ms: Option<i64>,
    pub pauses_used: i64,
    pub max_pauses: i64,
}

impl TimerView {
    pub fn elapsed_ms(&self, now: i64) -> i64 {
        let paused_now = self.paused_since_ms.map(|p| now - p).unwrap_or(0);
        (now - self.started_ms - self.paused_ms - paused_now).max(0)
    }

    fn limit_ms(&self) -> i64 {
        (self.planned_seconds + self.extended_seconds) * 1000
    }
}

pub fn active(conn: &Connection) -> AppResult<Option<TimerView>> {
    Ok(conn
        .query_row(
            "SELECT t.id, t.step_id, s.title, s.done_when, s.effort, s.quest_id, q.title,
                    t.planned_seconds, t.extended_seconds, t.started_ms, t.paused_ms,
                    t.paused_since_ms, s.pauses_used, p.max_pauses
             FROM timer_session t
             JOIN step s ON s.id = t.step_id
             JOIN quest q ON q.id = s.quest_id
             JOIN profile p ON p.id = 1
             WHERE t.ended_at IS NULL
             ORDER BY t.id DESC LIMIT 1",
            [],
            |r| {
                Ok(TimerView {
                    session_id: r.get(0)?,
                    step_id: r.get(1)?,
                    step_title: r.get(2)?,
                    done_when: r.get(3)?,
                    effort: r.get(4)?,
                    quest_id: r.get(5)?,
                    quest_title: r.get(6)?,
                    planned_seconds: r.get(7)?,
                    extended_seconds: r.get(8)?,
                    started_ms: r.get(9)?,
                    paused_ms: r.get(10)?,
                    paused_since_ms: r.get(11)?,
                    pauses_used: r.get(12)?,
                    max_pauses: r.get(13)?,
                })
            },
        )
        .optional()?)
}

fn require_active(conn: &Connection) -> AppResult<TimerView> {
    active(conn)?.ok_or_else(|| AppError::Msg("no objective is running".into()))
}

#[tauri::command]
pub fn get_timer(db: State<'_, Db>) -> AppResult<Option<TimerView>> {
    active(&*quests::lock(&db)?)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartResult {
    pub timer: TimerView,
    /// XP for starting, awarded only the first time an objective is started.
    pub start_xp: i64,
}

#[tauri::command]
pub fn start_step(db: State<'_, Db>, step_id: i64) -> AppResult<StartResult> {
    let mut conn = quests::lock(&db)?;
    if let Some(t) = active(&conn)? {
        return Err(AppError::Msg(format!("\"{}\" is already running", t.step_title)));
    }
    let (quest_id, est_minutes, status, quest_status): (i64, i64, String, String) = conn
        .query_row(
            "SELECT s.quest_id, s.est_minutes, s.status, q.status
             FROM step s JOIN quest q ON q.id = s.quest_id WHERE s.id = ?1",
            [step_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?
        .ok_or_else(|| AppError::Msg("objective not found".into()))?;
    if status == "done" {
        return Err(AppError::Msg("objective already completed".into()));
    }
    if quest_status != "active" {
        return Err(AppError::Msg("deploy the operation before starting it".into()));
    }
    let first_start: bool = conn.query_row(
        "SELECT NOT EXISTS (SELECT 1 FROM timer_session WHERE step_id = ?1)",
        [step_id],
        |r| r.get(0),
    )?;

    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO timer_session (step_id, started_at, started_ms, planned_seconds)
         VALUES (?1, datetime('now'), ?2, ?3)",
        params![step_id, now_ms(), est_minutes * 60],
    )?;
    tx.execute("UPDATE step SET status = 'active' WHERE id = ?1", [step_id])?;
    let start_xp = if first_start { rewards::START_XP } else { 0 };
    if start_xp > 0 {
        rewards::award(&tx, quest_id, Some(step_id), start_xp, "start")?;
    }
    tx.commit()?;
    Ok(StartResult { timer: require_active(&conn)?, start_xp })
}

#[tauri::command]
pub fn pause_timer(db: State<'_, Db>) -> AppResult<TimerView> {
    let conn = quests::lock(&db)?;
    let t = require_active(&conn)?;
    if t.paused_since_ms.is_some() {
        return Ok(t);
    }
    if t.pauses_used >= t.max_pauses {
        return Err(AppError::Msg("no pauses left for this objective".into()));
    }
    conn.execute(
        "UPDATE timer_session SET paused_since_ms = ?1 WHERE id = ?2",
        params![now_ms(), t.session_id],
    )?;
    conn.execute("UPDATE step SET pauses_used = pauses_used + 1 WHERE id = ?1", [t.step_id])?;
    require_active(&conn)
}

#[tauri::command]
pub fn resume_timer(db: State<'_, Db>) -> AppResult<TimerView> {
    let conn = quests::lock(&db)?;
    let t = require_active(&conn)?;
    if let Some(since) = t.paused_since_ms {
        conn.execute(
            "UPDATE timer_session SET paused_ms = paused_ms + ?1, paused_since_ms = NULL
             WHERE id = ?2",
            params![now_ms() - since, t.session_id],
        )?;
    }
    require_active(&conn)
}

#[tauri::command]
pub fn extend_timer(db: State<'_, Db>, minutes: i64) -> AppResult<TimerView> {
    if !(1..=60).contains(&minutes) {
        return Err(AppError::Msg("extension must be 1 to 60 minutes".into()));
    }
    let conn = quests::lock(&db)?;
    let t = require_active(&conn)?;
    conn.execute(
        "UPDATE timer_session SET extended_seconds = extended_seconds + ?1 WHERE id = ?2",
        params![minutes * 60, t.session_id],
    )?;
    require_active(&conn)
}

/// Ends the running session without XP and returns the objective to the queue.
pub fn abort_active(conn: &Connection) -> AppResult<Option<i64>> {
    let Some(t) = active(conn)? else { return Ok(None) };
    let now = now_ms();
    conn.execute(
        "UPDATE timer_session SET ended_at = datetime('now'), ended_ms = ?1, outcome = 'aborted',
                paused_ms = paused_ms + COALESCE(?1 - paused_since_ms, 0), paused_since_ms = NULL
         WHERE id = ?2",
        params![now, t.session_id],
    )?;
    conn.execute("UPDATE step SET status = 'pending' WHERE id = ?1", [t.step_id])?;
    Ok(Some(t.step_id))
}

#[tauri::command]
pub fn abort_step(db: State<'_, Db>) -> AppResult<()> {
    abort_active(&*quests::lock(&db)?)?;
    Ok(())
}

#[tauri::command]
pub fn complete_step(db: State<'_, Db>) -> AppResult<CompletionResult> {
    let mut conn = quests::lock(&db)?;
    let t = require_active(&conn)?;
    let now = now_ms();
    let timing = Timing {
        session_id: t.session_id,
        elapsed_ms: t.elapsed_ms(now),
        planned_seconds: t.planned_seconds,
        now_ms: now,
    };
    rewards::complete(&mut conn, t.step_id, Some(timing))
}

/// Marks an objective done without a timed run (no on-time bonus or calibration).
#[tauri::command]
pub fn complete_step_untimed(db: State<'_, Db>, step_id: i64) -> AppResult<CompletionResult> {
    let mut conn = quests::lock(&db)?;
    if active(&conn)?.is_some_and(|t| t.step_id == step_id) {
        return Err(AppError::Msg("this objective is running; complete it from the timer".into()));
    }
    rewards::complete(&mut conn, step_id, None)
}

// ------------------------------------------------------------------ ticker

fn fmt_clock(ms: i64) -> String {
    let s = ms.abs() / 1000;
    format!("{}{:02}:{:02}", if ms < 0 { "+" } else { "" }, s / 60, s % 60)
}

/// Polls the running session once a second: keeps the tray tooltip current
/// and fires notifications at 50%, 80% and 100% of the time limit.
pub fn spawn_ticker(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // (session id, thresholds already handled as a bitmask)
        let mut seen: (i64, u8) = (0, 0);
        let mut last_tip = String::new();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let state = {
                let db = app.state::<Db>();
                let Ok(conn) = quests::lock(&db) else { continue };
                let lang = settings::load(&conn).map(|s| s.language).unwrap_or_default();
                active(&conn).ok().flatten().map(|t| (t, lang))
            };

            let tip = match &state {
                Some((t, _)) => {
                    let left = t.limit_ms() - t.elapsed_ms(now_ms());
                    let paused = if t.paused_since_ms.is_some() { " ‖" } else { "" };
                    format!("Quest Ops · {}{paused} · {}", fmt_clock(left), t.step_title)
                }
                None => "Quest Ops".to_string(),
            };
            if tip != last_tip {
                if let Some(tray) = app.tray_by_id(TRAY_ID) {
                    let _ = tray.set_tooltip(Some(&tip));
                }
                last_tip = tip;
            }

            let Some((t, lang)) = state else { continue };
            let frac = t.elapsed_ms(now_ms()) as f64 / t.limit_ms().max(1) as f64;
            let thresholds: [(u8, f64); 3] = [(1, 0.5), (2, 0.8), (4, 1.0)];
            if seen.0 != t.session_id {
                // A run found already in progress (e.g. after restart): skip passed marks.
                let passed = thresholds.iter().filter(|(_, f)| frac >= *f).fold(0, |m, (b, _)| m | b);
                seen = (t.session_id, passed);
                continue;
            }
            for (bit, mark) in thresholds {
                if frac >= mark && seen.1 & bit == 0 {
                    seen.1 |= bit;
                    notify(&app, &t, bit, &lang);
                }
            }
        }
    });
}

fn notify(app: &AppHandle, t: &TimerView, bit: u8, lang: &str) {
    let left_min = ((t.limit_ms() - t.elapsed_ms(now_ms())) / 60_000).max(0);
    let (title, body) = match (lang, bit) {
        ("zh", 1) => ("已过半程".to_string(), t.step_title.clone()),
        ("zh", 2) => (format!("剩余 {left_min} 分钟"), t.step_title.clone()),
        ("zh", _) => ("时间到".to_string(), format!("{} · 已进入超时", t.step_title)),
        (_, 1) => ("Halfway there".to_string(), t.step_title.clone()),
        (_, 2) => (format!("{left_min} min left"), t.step_title.clone()),
        (_, _) => ("Time's up".to_string(), format!("{} · overtime running", t.step_title)),
    };
    let _ = app.notification().builder().title(title).body(body).show();
    if bit == 4 {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.request_user_attention(Some(tauri::UserAttentionType::Informational));
        }
    }
}
