//! XP, levels, streaks, estimate calibration, achievements and stats.
//! `xp_event` is the source of truth; `profile.total_xp` and `category.xp`
//! are running totals kept in step with it.

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::{Map, Value, json};
use tauri::State;

use crate::{
    db::Db,
    error::{AppError, AppResult},
    priority, quests,
};

/// XP for starting an objective for the first time (rewards getting going).
pub const START_XP: i64 = 2;
/// XP per effort point when an objective is completed.
const XP_PER_EFFORT: i64 = 10;
/// Bonus share for finishing inside the original estimate.
const ON_TIME_PCT: i64 = 25;
/// Bonus share for Strategic Ops (Q2) work.
const STRATEGIC_PCT: i64 = 50;
/// One in this many completions is a critical hit (double XP).
const CRITICAL_ODDS: u32 = 10;
/// Largest gap between active days that keeps a streak alive (one rest day).
const STREAK_MAX_GAP: i64 = 2;
/// Weight of the newest sample in the estimate calibration average.
const CALIBRATION_ALPHA: f64 = 0.3;
pub const GENERAL_CATEGORY: &str = "General";

/// Returns (level, xp into the level, xp needed for the next level).
pub fn level_for(total_xp: i64) -> (i64, i64, i64) {
    let (mut level, mut rest) = (0, total_xp.max(0));
    loop {
        let need = 100 + 25 * level;
        if rest < need {
            return (level, rest, need);
        }
        rest -= need;
        level += 1;
    }
}

/// Current and longest streak from a list of active day numbers (any order).
/// Days separated by at most one rest day count as consecutive.
pub fn streaks(mut days: Vec<i64>, today: i64) -> (i64, i64) {
    days.sort_unstable();
    days.dedup();
    let (mut longest, mut run) = (0, 0);
    let mut prev: Option<i64> = None;
    for &d in &days {
        run = match prev {
            Some(p) if d - p <= STREAK_MAX_GAP => run + 1,
            _ => 1,
        };
        longest = longest.max(run);
        prev = Some(d);
    }
    let current = match prev {
        Some(last) if today - last <= STREAK_MAX_GAP => run,
        _ => 0,
    };
    (current, longest)
}

fn roll_critical() -> bool {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(1);
    nanos.wrapping_mul(2654435761) % CRITICAL_ODDS == 0
}

/// Records an XP event and updates the running totals.
pub fn award(
    conn: &Connection,
    quest_id: i64,
    step_id: Option<i64>,
    amount: i64,
    reason: &str,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO xp_event (quest_id, step_id, amount, reason) VALUES (?1, ?2, ?3, ?4)",
        params![quest_id, step_id, amount, reason],
    )?;
    conn.execute("UPDATE profile SET total_xp = total_xp + ?1 WHERE id = 1", [amount])?;
    conn.execute(
        "UPDATE category SET xp = xp + ?1
         WHERE id = (SELECT category_id FROM quest WHERE id = ?2)",
        params![amount, quest_id],
    )?;
    Ok(())
}

fn total_xp(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row("SELECT total_xp FROM profile WHERE id = 1", [], |r| r.get(0))?)
}

// ------------------------------------------------------------- completion

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XpLine {
    pub reason: String,
    pub amount: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestDone {
    pub id: i64,
    pub title: String,
    pub bonus: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionResult {
    /// "on_time", "overtime" or "untimed".
    pub outcome: String,
    pub elapsed_seconds: Option<i64>,
    pub lines: Vec<XpLine>,
    pub total: i64,
    pub critical: bool,
    pub quest_completed: Option<QuestDone>,
    pub achievements: Vec<Unlocked>,
    pub level_before: i64,
    pub level_after: i64,
}

/// Timing of a timed run, when the objective was completed under the timer.
pub struct Timing {
    pub session_id: i64,
    pub elapsed_ms: i64,
    pub planned_seconds: i64,
    pub now_ms: i64,
}

/// Marks an objective done, awards XP, completes the quest when it was the
/// last objective, updates calibration and unlocks achievements.
pub fn complete(conn: &mut Connection, step_id: i64, timing: Option<Timing>) -> AppResult<CompletionResult> {
    let (quest_id, effort, est_minutes, status): (i64, i64, i64, String) = conn
        .query_row(
            "SELECT quest_id, effort, est_minutes, status FROM step WHERE id = ?1",
            [step_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?
        .ok_or_else(|| AppError::Msg("objective not found".into()))?;
    if status == "done" {
        return Err(AppError::Msg("objective already completed".into()));
    }

    let level_before = level_for(total_xp(conn)?).0;
    let quadrant = priority::current_quadrant(conn, quest_id)?;
    let elapsed_seconds = timing.as_ref().map(|t| (t.elapsed_ms / 1000).max(1));
    let outcome = match &timing {
        Some(t) if t.elapsed_ms <= t.planned_seconds * 1000 => "on_time",
        Some(_) => "overtime",
        None => "untimed",
    };

    let base = XP_PER_EFFORT * effort;
    let mut lines = vec![XpLine { reason: "objective".into(), amount: base }];
    if outcome == "on_time" {
        lines.push(XpLine { reason: "on_time".into(), amount: base * ON_TIME_PCT / 100 });
    }
    if quadrant == Some(2) {
        lines.push(XpLine { reason: "strategic".into(), amount: base * STRATEGIC_PCT / 100 });
    }
    let critical = roll_critical();
    if critical {
        let sum: i64 = lines.iter().map(|l| l.amount).sum();
        lines.push(XpLine { reason: "critical".into(), amount: sum });
    }

    let tx = conn.transaction()?;
    if let Some(t) = &timing {
        tx.execute(
            "UPDATE timer_session SET ended_at = datetime('now'), ended_ms = ?1, outcome = ?2,
                    paused_ms = paused_ms + COALESCE(?1 - paused_since_ms, 0),
                    paused_since_ms = NULL
             WHERE id = ?3",
            params![t.now_ms, outcome, t.session_id],
        )?;
    }
    tx.execute(
        "UPDATE step SET status = 'done', completed_at = datetime('now'), actual_seconds = ?1
         WHERE id = ?2",
        params![elapsed_seconds, step_id],
    )?;
    for l in &lines {
        award(&tx, quest_id, Some(step_id), l.amount, &l.reason)?;
    }

    let remaining: i64 = tx.query_row(
        "SELECT COUNT(*) FROM step WHERE quest_id = ?1 AND parent_step_id IS NULL
         AND status IN ('pending', 'active')",
        [quest_id],
        |r| r.get(0),
    )?;
    let quest_completed = if remaining == 0 {
        let (title, steps): (String, i64) = tx.query_row(
            "SELECT title, (SELECT COUNT(*) FROM step WHERE quest_id = q.id AND parent_step_id IS NULL)
             FROM quest q WHERE id = ?1",
            [quest_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut bonus = 20 + 5 * steps;
        award(&tx, quest_id, None, bonus, "operation")?;
        if quadrant == Some(2) {
            let extra = bonus * STRATEGIC_PCT / 100;
            award(&tx, quest_id, None, extra, "operation_strategic")?;
            bonus += extra;
        }
        if quadrant == Some(4) {
            // Zero-XP marker: quadrants drift with deadlines, so record it now.
            award(&tx, quest_id, None, 0, "operation_archive")?;
        }
        tx.execute(
            "UPDATE quest SET status = 'completed', completed_at = datetime('now') WHERE id = ?1",
            [quest_id],
        )?;
        Some(QuestDone { id: quest_id, title, bonus })
    } else {
        None
    };

    if let (Some(secs), Some(_)) = (elapsed_seconds, &timing) {
        update_calibration(&tx, quest_id, secs as f64 / (est_minutes * 60) as f64)?;
    }
    tx.commit()?;

    let achievements = check_achievements(conn)?;
    let total: i64 = lines.iter().map(|l| l.amount).sum::<i64>()
        + quest_completed.as_ref().map(|q| q.bonus).unwrap_or(0);
    Ok(CompletionResult {
        outcome: outcome.into(),
        elapsed_seconds,
        lines,
        total,
        critical,
        quest_completed,
        achievements,
        level_before,
        level_after: level_for(total_xp(conn)?).0,
    })
}

// ------------------------------------------------------------ calibration

fn category_key(conn: &Connection, quest_id: i64) -> AppResult<String> {
    let name: Option<String> = conn.query_row(
        "SELECT c.name FROM quest q LEFT JOIN category c ON c.id = q.category_id WHERE q.id = ?1",
        [quest_id],
        |r| r.get(0),
    )?;
    Ok(name.unwrap_or_else(|| GENERAL_CATEGORY.into()))
}

fn load_calibration(conn: &Connection) -> AppResult<Map<String, Value>> {
    let raw: String =
        conn.query_row("SELECT calibration_json FROM profile WHERE id = 1", [], |r| r.get(0))?;
    Ok(serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default())
}

fn update_calibration(conn: &Connection, quest_id: i64, ratio: f64) -> AppResult<()> {
    let key = category_key(conn, quest_id)?;
    let ratio = ratio.clamp(0.25, 4.0);
    let mut cal = load_calibration(conn)?;
    let (old, n) = cal
        .get(&key)
        .map(|v| (v["ratio"].as_f64().unwrap_or(ratio), v["samples"].as_i64().unwrap_or(0)))
        .unwrap_or((ratio, 0));
    let next = if n == 0 { ratio } else { old + CALIBRATION_ALPHA * (ratio - old) };
    cal.insert(key, json!({ "ratio": (next * 100.0).round() / 100.0, "samples": n + 1 }));
    conn.execute(
        "UPDATE profile SET calibration_json = ?1 WHERE id = 1",
        [Value::Object(cal).to_string()],
    )?;
    Ok(())
}

/// The person's actual/estimated time ratio for a quest's category, once
/// there are enough samples to trust it.
pub fn calibration_for(conn: &Connection, quest_id: i64) -> AppResult<Option<f64>> {
    let key = category_key(conn, quest_id)?;
    let cal = load_calibration(conn)?;
    Ok(cal.get(&key).and_then(|v| {
        (v["samples"].as_i64().unwrap_or(0) >= 3).then(|| v["ratio"].as_f64()).flatten()
    }))
}

// ---------------------------------------------------------- achievements

/// Tiered achievement families. Each tier is stored as `"{family}:{goal}"`.
/// Past the last listed goal, each further goal doubles the previous one.
const FAMILIES: &[(&str, &[i64])] = &[
    ("objectives", &[1, 10, 50, 100, 200, 500, 1000, 2000, 5000, 10000]),
    ("operations", &[1, 5, 10, 25, 50, 100, 200, 500, 1000]),
    ("strategic", &[1, 5, 10, 25, 50, 100, 250, 500]),
    ("on_time", &[1, 10, 25, 50, 100, 250, 500, 1000, 2500]),
    ("streak", &[3, 7, 14, 30, 60, 100, 180, 365]),
    ("level", &[5, 10, 20, 30, 40, 50, 75, 100]),
    ("focus_hours", &[1, 5, 10, 25, 50, 100, 250, 500, 1000]),
];

/// Goal of tier `i` (0-based).
pub fn tier_goal(tiers: &[i64], i: usize) -> i64 {
    match tiers.get(i) {
        Some(&g) => g,
        None => tiers.last().copied().unwrap_or(1) << (i + 1 - tiers.len()).min(40),
    }
}

/// How many tiers `value` has reached.
pub fn tiers_reached(tiers: &[i64], value: i64) -> usize {
    (0..).take_while(|&i| tier_goal(tiers, i) <= value).count()
}

fn family_value(conn: &Connection, family: &str) -> AppResult<i64> {
    Ok(match family {
        // Counted from the XP log so clearing finished work keeps the progress.
        "objectives" => count(conn, "SELECT COUNT(*) FROM xp_event WHERE reason = 'objective'")?,
        "operations" => count(conn, "SELECT COUNT(*) FROM xp_event WHERE reason = 'operation'")?,
        "strategic" => count(conn, "SELECT COUNT(*) FROM xp_event WHERE reason = 'operation_strategic'")?,
        "on_time" => count(conn, "SELECT COUNT(*) FROM xp_event WHERE reason = 'on_time'")?,
        "streak" => {
            let (days, today) = active_days(conn)?;
            streaks(days, today).1
        }
        "level" => level_for(total_xp(conn)?).0,
        "focus_hours" => count(
            conn,
            "SELECT ((SELECT archived_focus_ms FROM profile) + COALESCE(SUM(ended_ms - started_ms - paused_ms), 0)) / 3600000
             FROM timer_session WHERE outcome IN ('on_time', 'overtime')",
        )?,
        _ => 0,
    })
}

/// Special achievements, invisible until earned. Each is a query that is
/// true once the feat has happened at least once.
const HIDDEN_ACHIEVEMENTS: &[(&str, &str)] = &[
    (
        "night_owl",
        "SELECT 1 FROM step WHERE status = 'done'
           AND CAST(strftime('%H', completed_at, 'localtime') AS INTEGER) < 4",
    ),
    (
        "early_bird",
        "SELECT 1 FROM step WHERE status = 'done'
           AND CAST(strftime('%H', completed_at, 'localtime') AS INTEGER) BETWEEN 4 AND 6",
    ),
    (
        "photo_finish",
        "SELECT 1 FROM timer_session WHERE outcome = 'on_time'
           AND ended_ms - started_ms - paused_ms >= planned_seconds * 1000 - 30000",
    ),
    (
        "speedrun",
        "SELECT 1 FROM timer_session WHERE outcome = 'on_time' AND planned_seconds >= 600
           AND ended_ms - started_ms - paused_ms <= planned_seconds * 250",
    ),
    (
        "marathon",
        "SELECT 1 FROM timer_session WHERE outcome IN ('on_time', 'overtime')
           AND ended_ms - started_ms - paused_ms >= 3600000",
    ),
    (
        "comeback",
        "SELECT 1 FROM step s WHERE s.status = 'done'
           AND EXISTS (SELECT 1 FROM timer_session t WHERE t.step_id = s.id AND t.outcome = 'aborted')
           AND EXISTS (SELECT 1 FROM timer_session t WHERE t.step_id = s.id
                       AND t.outcome IN ('on_time', 'overtime'))",
    ),
    (
        "fortune",
        "SELECT 1 FROM xp_event WHERE reason = 'critical' GROUP BY reason HAVING COUNT(*) >= 3",
    ),
    (
        "flawless",
        "SELECT 1 FROM quest q WHERE q.status = 'completed'
           AND (SELECT COUNT(*) FROM step s WHERE s.quest_id = q.id AND s.parent_step_id IS NULL) >= 3
           AND NOT EXISTS (
               SELECT 1 FROM step s WHERE s.quest_id = q.id AND s.parent_step_id IS NULL
                 AND NOT EXISTS (SELECT 1 FROM timer_session t
                                 WHERE t.step_id = s.id AND t.outcome = 'on_time'))",
    ),
    (
        "just_in_time",
        "SELECT 1 FROM quest WHERE status = 'completed' AND deadline IS NOT NULL
           AND date(completed_at, 'localtime') = deadline",
    ),
    (
        "someday",
        "SELECT 1 FROM xp_event WHERE reason = 'operation_archive'",
    ),
    (
        "weekend_warrior",
        "SELECT 1 FROM (SELECT DISTINCT date(created_at, 'localtime') AS d
                        FROM xp_event WHERE reason = 'objective') sat
         WHERE strftime('%w', sat.d) = '6'
           AND EXISTS (SELECT 1 FROM xp_event sun WHERE sun.reason = 'objective'
                       AND date(sun.created_at, 'localtime') = date(sat.d, '+1 day'))",
    ),
];

fn count(conn: &Connection, sql: &str) -> AppResult<i64> {
    Ok(conn.query_row(sql, [], |r| r.get(0))?)
}

fn exists(conn: &Connection, sql: &str) -> AppResult<bool> {
    Ok(count(conn, &format!("SELECT EXISTS ({sql})"))? == 1)
}

fn active_days(conn: &Connection) -> AppResult<(Vec<i64>, i64)> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT CAST(julianday(date(created_at, 'localtime')) AS INTEGER)
         FROM xp_event WHERE reason = 'objective'",
    )?;
    let days = stmt.query_map([], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    let today = count(conn, "SELECT CAST(julianday(date('now', 'localtime')) AS INTEGER)")?;
    Ok((days, today))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unlocked {
    /// Family name for tiered achievements, otherwise the achievement id.
    pub id: String,
    /// A special achievement that was invisible until now.
    pub hidden: bool,
    /// The goal reached, for tiered achievements.
    pub goal: Option<i64>,
    /// The tier number (1-based), for tiered achievements.
    pub level: Option<i64>,
}

/// Unlocks every achievement whose condition now holds; returns the new ones.
fn check_achievements(conn: &Connection) -> AppResult<Vec<Unlocked>> {
    let insert = |id: &str| -> AppResult<bool> {
        Ok(conn.execute("INSERT OR IGNORE INTO achievement (id) VALUES (?1)", [id])? == 1)
    };
    let mut new = Vec::new();
    for (family, tiers) in FAMILIES {
        // Record every tier reached; announce only the highest new one.
        let mut highest = None;
        for i in 0..tiers_reached(tiers, family_value(conn, family)?) {
            if insert(&format!("{family}:{}", tier_goal(tiers, i)))? {
                highest = Some(i);
            }
        }
        if let Some(i) = highest {
            new.push(Unlocked {
                id: family.to_string(),
                hidden: false,
                goal: Some(tier_goal(tiers, i)),
                level: Some(i as i64 + 1),
            });
        }
    }
    for (id, sql) in HIDDEN_ACHIEVEMENTS {
        if exists(conn, sql)? && insert(id)? {
            new.push(Unlocked { id: id.to_string(), hidden: true, goal: None, level: None });
        }
    }
    Ok(new)
}

// ----------------------------------------------------------------- stats

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayXp {
    pub date: String,
    pub xp: i64,
    pub objectives: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Calibration {
    pub category: String,
    pub ratio: f64,
    pub samples: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TierProgress {
    pub family: String,
    pub value: i64,
    /// Tiers reached so far (0 before the first goal).
    pub level: i64,
    /// Goal of the tier reached last (0 before the first goal).
    pub prev_goal: i64,
    pub next_goal: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecialAchievement {
    pub id: String,
    pub unlocked_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub total_xp: i64,
    pub level: i64,
    pub level_xp: i64,
    pub level_need: i64,
    pub streak: i64,
    pub longest_streak: i64,
    pub today_xp: i64,
    pub today_objectives: i64,
    pub objectives_done: i64,
    pub operations_done: i64,
    pub timed_runs: i64,
    pub on_time_runs: i64,
    pub daily: Vec<DayXp>,
    pub calibration: Vec<Calibration>,
    pub tiers: Vec<TierProgress>,
    /// Earned special achievements only; unearned ones stay invisible.
    pub specials: Vec<SpecialAchievement>,
}

#[tauri::command]
pub fn get_stats(db: State<'_, Db>) -> AppResult<Stats> {
    let conn = quests::lock(&db)?;
    let total = total_xp(&conn)?;
    let (level, level_xp, level_need) = level_for(total);
    let (days, today) = active_days(&conn)?;
    let (streak, longest_streak) = streaks(days, today);

    let mut stmt = conn.prepare(
        "WITH RECURSIVE d(n) AS (SELECT 0 UNION ALL SELECT n + 1 FROM d WHERE n < 13)
         SELECT date('now', 'localtime', -n || ' days') AS day,
                (SELECT COALESCE(SUM(amount), 0) FROM xp_event
                  WHERE date(created_at, 'localtime') = date('now', 'localtime', -n || ' days')),
                (SELECT COUNT(*) FROM xp_event WHERE reason = 'objective'
                  AND date(created_at, 'localtime') = date('now', 'localtime', -n || ' days'))
         FROM d ORDER BY n DESC",
    )?;
    let daily: Vec<DayXp> = stmt
        .query_map([], |r| Ok(DayXp { date: r.get(0)?, xp: r.get(1)?, objectives: r.get(2)? }))?
        .collect::<Result<_, _>>()?;
    let (today_xp, today_objectives) =
        daily.last().map(|d| (d.xp, d.objectives)).unwrap_or((0, 0));

    let mut calibration: Vec<Calibration> = load_calibration(&conn)?
        .into_iter()
        .map(|(category, v)| Calibration {
            category,
            ratio: v["ratio"].as_f64().unwrap_or(1.0),
            samples: v["samples"].as_i64().unwrap_or(0),
        })
        .collect();
    calibration.sort_by(|a, b| b.samples.cmp(&a.samples));

    let tiers = FAMILIES
        .iter()
        .map(|(family, goals)| {
            let value = family_value(&conn, family)?;
            let level = tiers_reached(goals, value);
            Ok(TierProgress {
                family: family.to_string(),
                value,
                level: level as i64,
                prev_goal: if level == 0 { 0 } else { tier_goal(goals, level - 1) },
                next_goal: tier_goal(goals, level),
            })
        })
        .collect::<AppResult<Vec<_>>>()?;

    let mut stmt = conn.prepare("SELECT unlocked_at FROM achievement WHERE id = ?1")?;
    let mut specials = Vec::new();
    for (id, _) in HIDDEN_ACHIEVEMENTS {
        if let Some(unlocked_at) = stmt.query_row([id], |r| r.get(0)).optional()? {
            specials.push(SpecialAchievement { id: id.to_string(), unlocked_at });
        }
    }

    Ok(Stats {
        total_xp: total,
        level,
        level_xp,
        level_need,
        streak,
        longest_streak,
        today_xp,
        today_objectives,
        objectives_done: count(&conn, "SELECT COUNT(*) FROM xp_event WHERE reason = 'objective'")?,
        operations_done: count(&conn, "SELECT COUNT(*) FROM xp_event WHERE reason = 'operation'")?,
        timed_runs: count(
            &conn,
            "SELECT COUNT(*) FROM timer_session WHERE outcome IN ('on_time', 'overtime')",
        )?,
        on_time_runs: count(&conn, "SELECT COUNT(*) FROM timer_session WHERE outcome = 'on_time'")?,
        daily,
        calibration,
        tiers,
        specials,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrate_for_test(&mut conn);
        // Q2: importance 5, urgency 1, no deadline.
        conn.execute_batch(
            "INSERT INTO quest (id, title, status, importance, urgency_base, category_id)
                 VALUES (1, 'Op', 'active', 5, 1, 2);
             INSERT INTO step (id, quest_id, title, effort, est_minutes, order_index)
                 VALUES (1, 1, 'A', 2, 10, 0), (2, 1, 'B', 3, 10, 1);",
        )
        .unwrap();
        conn
    }

    /// Announced unlocks as "family:goal" for tiers, plain ids otherwise.
    fn unlocked(r: &CompletionResult) -> Vec<String> {
        r.achievements
            .iter()
            .map(|a| match a.goal {
                Some(g) => format!("{}:{g}", a.id),
                None => a.id.clone(),
            })
            .collect()
    }

    fn has(r: &CompletionResult, id: &str) -> bool {
        unlocked(r).iter().any(|u| u == id)
    }

    #[test]
    fn tier_goals_extend_by_doubling() {
        let goals = [1, 10, 50, 100];
        assert_eq!(tier_goal(&goals, 0), 1);
        assert_eq!(tier_goal(&goals, 3), 100);
        assert_eq!(tier_goal(&goals, 4), 200);
        assert_eq!(tier_goal(&goals, 5), 400);
        assert_eq!(tiers_reached(&goals, 0), 0);
        assert_eq!(tiers_reached(&goals, 9), 1);
        assert_eq!(tiers_reached(&goals, 10), 2);
        assert_eq!(tiers_reached(&goals, 250), 5);
    }

    #[test]
    fn old_milestones_carry_over_as_tiers() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::MIGRATIONS[..3].concat().as_str()).unwrap();
        conn.execute("INSERT INTO achievement (id) VALUES ('first_objective'), ('streak_3')", [])
            .unwrap();
        conn.pragma_update(None, "user_version", 3).unwrap();
        crate::db::migrate_for_test(&mut conn);
        let ids: Vec<String> = conn
            .prepare("SELECT id FROM achievement ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(ids, ["objectives:1", "streak:3"]);
    }

    /// A finished timer session of `elapsed_s` on a 10-minute estimate.
    fn timed(conn: &Connection, session_id: i64, step_id: i64, elapsed_s: i64) -> Timing {
        conn.execute(
            "INSERT INTO timer_session (id, step_id, started_at, started_ms, planned_seconds)
             VALUES (?1, ?2, datetime('now'), 0, 600)",
            params![session_id, step_id],
        )
        .unwrap();
        Timing { session_id, elapsed_ms: elapsed_s * 1000, planned_seconds: 600, now_ms: elapsed_s * 1000 }
    }

    #[test]
    fn hidden_achievements_unlock_from_play_patterns() {
        let mut conn = seeded();
        // 9:50 of 10:00: on time with 10 seconds to spare.
        let t = timed(&conn, 1, 1, 590);
        let a = complete(&mut conn, 1, Some(t)).unwrap();
        assert!(has(&a, "photo_finish"));
        assert!(a.achievements.iter().any(|x| x.id == "photo_finish" && x.hidden));
        assert!(!has(&a, "speedrun"));

        // Abort a run on step 2, then finish it: second wind.
        conn.execute(
            "INSERT INTO timer_session (id, step_id, started_at, started_ms, planned_seconds,
                                        ended_at, ended_ms, outcome)
             VALUES (2, 2, datetime('now'), 0, 600, datetime('now'), 1000, 'aborted')",
            [],
        )
        .unwrap();
        let t = timed(&conn, 3, 2, 300);
        let b = complete(&mut conn, 2, Some(t)).unwrap();
        assert!(has(&b, "comeback"));
        assert!(!has(&b, "someday"), "operation 1 was Q2");

        // An Archive (Q4) operation completed: someday is today.
        conn.execute_batch(
            "INSERT INTO quest (id, title, status, importance, urgency_base)
                 VALUES (2, 'Chore', 'active', 1, 1);
             INSERT INTO step (id, quest_id, title, effort, est_minutes, order_index)
                 VALUES (3, 2, 'C', 1, 10, 0);",
        )
        .unwrap();
        let c = complete(&mut conn, 3, None).unwrap();
        assert!(has(&c, "someday"));

        let stats_hidden: i64 = conn
            .query_row("SELECT COUNT(*) FROM achievement WHERE id IN
                        ('photo_finish', 'comeback', 'someday')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stats_hidden, 3);
    }

    fn reasons(r: &CompletionResult) -> Vec<&str> {
        r.lines.iter().map(|l| l.reason.as_str()).filter(|r| *r != "critical").collect()
    }

    #[test]
    fn completing_last_objective_finishes_operation() {
        let mut conn = seeded();
        let fast = Timing { session_id: 0, elapsed_ms: 5 * 60_000, planned_seconds: 600, now_ms: 0 };
        let a = complete(&mut conn, 1, Some(fast)).unwrap();
        assert_eq!(a.outcome, "on_time");
        assert_eq!(reasons(&a), ["objective", "on_time", "strategic"]);
        assert!(a.quest_completed.is_none());
        assert!(has(&a, "objectives:1"));

        let b = complete(&mut conn, 2, None).unwrap();
        assert_eq!(b.outcome, "untimed");
        assert_eq!(reasons(&b), ["objective", "strategic"]);
        let done = b.quest_completed.as_ref().unwrap();
        assert_eq!(done.bonus, 30 + 15); // 20 + 5 per objective, +50% strategic
        assert!(has(&b, "operations:1"));

        let status: String =
            conn.query_row("SELECT status FROM quest WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(status, "completed");
        let (log, total): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT SUM(amount) FROM xp_event), total_xp FROM profile",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(log, total, "running total matches the event log");
        assert!(complete(&mut conn, 2, None).is_err(), "cannot complete twice");
    }

    #[test]
    fn calibration_tracks_slow_runs() {
        let mut conn = seeded();
        let slow = Timing { session_id: 0, elapsed_ms: 20 * 60_000, planned_seconds: 600, now_ms: 0 };
        assert_eq!(complete(&mut conn, 1, Some(slow)).unwrap().outcome, "overtime");
        let cal = load_calibration(&conn).unwrap();
        assert_eq!(cal["Writing"]["ratio"], 2.0);
        assert_eq!(cal["Writing"]["samples"], 1);
        assert_eq!(calibration_for(&conn, 1).unwrap(), None, "needs 3 samples");
    }

    #[test]
    fn level_curve() {
        assert_eq!(level_for(0), (0, 0, 100));
        assert_eq!(level_for(99), (0, 99, 100));
        assert_eq!(level_for(100), (1, 0, 125));
        assert_eq!(level_for(225), (2, 0, 150));
    }

    #[test]
    fn streak_allows_one_rest_day() {
        assert_eq!(streaks(vec![], 100), (0, 0));
        assert_eq!(streaks(vec![100], 100), (1, 1));
        assert_eq!(streaks(vec![98, 99, 100], 100), (3, 3));
        // a rest day on 99 keeps the streak going
        assert_eq!(streaks(vec![97, 98, 100], 100), (3, 3));
        // two missed days break it
        assert_eq!(streaks(vec![90, 91, 92, 96, 97], 97), (2, 3));
        // last active day was two days ago: still alive today
        assert_eq!(streaks(vec![97, 98], 100), (2, 2));
        assert_eq!(streaks(vec![97, 98], 101), (0, 2));
    }
}
