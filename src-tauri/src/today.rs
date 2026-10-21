//! The daily operations board: what to work on today, what got done, and an
//! end-of-day debrief.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::State;

use crate::{
    db::Db,
    error::{AppError, AppResult},
    llm,
    priority::{self, MatrixQuest},
    quests, settings,
};

/// Upcoming objectives per operation offered to the AI planner.
const CANDIDATES_PER_QUEST: usize = 3;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodayStep {
    pub step_id: i64,
    pub title: String,
    pub done_when: String,
    pub est_minutes: i64,
    pub effort: i64,
    pub status: String,
    pub quest_id: i64,
    pub quest_title: String,
    pub quadrant: Option<i64>,
    pub days_left: Option<i64>,
    pub note: String,
    pub xp: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Today {
    pub date: String,
    /// The AI plan for today; empty when none was made.
    pub planned: Vec<TodayStep>,
    /// The next objective of every active operation, most pressing first.
    pub suggestions: Vec<TodayStep>,
    pub completed: Vec<TodayStep>,
    pub debrief: Option<String>,
}

fn today_date(conn: &Connection) -> AppResult<String> {
    Ok(conn.query_row("SELECT date('now', 'localtime')", [], |r| r.get(0))?)
}

fn matrix_by_id(conn: &Connection) -> AppResult<HashMap<i64, MatrixQuest>> {
    Ok(priority::active_matrix(conn)?.into_iter().map(|q| (q.id, q)).collect())
}

const STEP_COLS: &str = "s.id, s.title, s.done_when, s.est_minutes, s.effort, s.status,
                         q.id, q.title";

fn map_step(r: &rusqlite::Row, note_col: Option<usize>) -> rusqlite::Result<TodayStep> {
    Ok(TodayStep {
        step_id: r.get(0)?,
        title: r.get(1)?,
        done_when: r.get(2)?,
        est_minutes: r.get(3)?,
        effort: r.get(4)?,
        status: r.get(5)?,
        quest_id: r.get(6)?,
        quest_title: r.get(7)?,
        quadrant: None,
        days_left: None,
        note: match note_col {
            Some(i) => r.get(i)?,
            None => String::new(),
        },
        xp: 0,
    })
}

fn with_priority(mut s: TodayStep, m: &HashMap<i64, MatrixQuest>) -> TodayStep {
    if let Some(q) = m.get(&s.quest_id) {
        s.quadrant = q.quadrant;
        s.days_left = q.days_left;
    }
    s
}

/// Open objectives of active operations in order, grouped by operation.
fn open_steps(conn: &Connection, m: &HashMap<i64, MatrixQuest>) -> AppResult<Vec<Vec<TodayStep>>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {STEP_COLS} FROM step s JOIN quest q ON q.id = s.quest_id
         WHERE q.status = 'active' AND s.parent_step_id IS NULL
           AND s.status IN ('pending', 'active')
         ORDER BY q.id, s.order_index"
    ))?;
    let rows = stmt.query_map([], |r| map_step(r, None))?.collect::<Result<Vec<_>, _>>()?;
    let mut groups: Vec<Vec<TodayStep>> = Vec::new();
    for s in rows {
        let s = with_priority(s, m);
        match groups.last_mut() {
            Some(g) if g[0].quest_id == s.quest_id => g.push(s),
            _ => groups.push(vec![s]),
        }
    }
    groups.sort_by_key(|g| (g[0].quadrant.unwrap_or(5), g[0].days_left.unwrap_or(i64::MAX)));
    Ok(groups)
}

fn load_today(conn: &Connection) -> AppResult<Today> {
    let date = today_date(conn)?;
    let m = matrix_by_id(conn)?;

    let mut stmt = conn.prepare(&format!(
        "SELECT {STEP_COLS}, p.note FROM daily_plan p
         JOIN step s ON s.id = p.step_id JOIN quest q ON q.id = s.quest_id
         WHERE p.plan_date = ?1 ORDER BY p.order_index"
    ))?;
    let planned = stmt
        .query_map([&date], |r| map_step(r, Some(8)))?
        .map(|s| s.map(|s| with_priority(s, &m)))
        .collect::<Result<Vec<_>, _>>()?;

    let suggestions = open_steps(conn, &m)?.into_iter().map(|g| g[0].clone()).collect();

    let mut stmt = conn.prepare(&format!(
        "SELECT {STEP_COLS},
                (SELECT COALESCE(SUM(amount), 0) FROM xp_event x WHERE x.step_id = s.id)
         FROM step s JOIN quest q ON q.id = s.quest_id
         WHERE s.status = 'done' AND date(s.completed_at, 'localtime') = ?1
         ORDER BY s.completed_at DESC"
    ))?;
    let completed = stmt
        .query_map([&date], |r| {
            let mut s = map_step(r, None)?;
            s.xp = r.get(8)?;
            Ok(s)
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let debrief = conn
        .query_row("SELECT text FROM daily_debrief WHERE plan_date = ?1", [&date], |r| r.get(0))
        .optional()?;
    Ok(Today { date, planned, suggestions, completed, debrief })
}

#[tauri::command]
pub fn get_today(db: State<'_, Db>) -> AppResult<Today> {
    load_today(&*quests::lock(&db)?)
}

#[tauri::command]
pub fn clear_today_plan(db: State<'_, Db>) -> AppResult<Today> {
    let conn = quests::lock(&db)?;
    conn.execute("DELETE FROM daily_plan WHERE plan_date = date('now', 'localtime')", [])?;
    load_today(&conn)
}

// ------------------------------------------------------------- AI planner

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pick {
    step_id: i64,
    note: String,
}

#[derive(Deserialize)]
struct PlanReply {
    picks: Vec<Pick>,
}

const PLAN_SYSTEM: &str = "\
You plan one working day. From the candidate objectives, choose an ordered \
list that fits the time available.
- Respect each operation's order: never pick an objective without also picking \
the earlier open objectives of the same operation.
- Prefer Q1 (important + urgent) first, then Q2 (important), then Q3, then Q4. \
Within a quadrant prefer nearer deadlines.
- Leave about 15% of the time free as a buffer.
- Start with something quick and winnable when possible.
- note: a short reason for each pick (max 80 characters).
- Use only stepId values from the candidate list.";

#[tauri::command]
pub async fn plan_today(db: State<'_, Db>, minutes: i64) -> AppResult<Today> {
    if !(10..=960).contains(&minutes) {
        return Err(AppError::Msg("available time must be 10 to 960 minutes".into()));
    }
    let (s, user, valid) = {
        let conn = quests::lock(&db)?;
        let s = settings::load(&conn)?;
        let m = matrix_by_id(&conn)?;
        let groups = open_steps(&conn, &m)?;
        if groups.is_empty() {
            return Err(AppError::Msg("no open objectives to plan".into()));
        }
        let mut user = format!("Time available today: {minutes} minutes\n\nCandidates:\n");
        let mut valid = HashSet::new();
        for g in &groups {
            let q = &g[0];
            let quad = q.quadrant.map(|n| format!("Q{n}")).unwrap_or_else(|| "unscored".into());
            let due = q.days_left.map(|d| format!(", due in {d} days")).unwrap_or_default();
            user += &format!("Operation \"{}\" ({quad}{due}):\n", q.quest_title);
            for st in g.iter().take(CANDIDATES_PER_QUEST) {
                valid.insert(st.step_id);
                user += &format!("  stepId {}: {} ({} min)\n", st.step_id, st.title, st.est_minutes);
            }
        }
        if !s.goals_text.is_empty() {
            user += &format!("\nThe person's goals:\n{}\n", s.goals_text);
        }
        (s, user, valid)
    };
    if s.model_name.is_empty() {
        return Err(AppError::Msg("set a model name in Config first".into()));
    }
    let system = format!("{PLAN_SYSTEM}\n{}", llm::language_rule(&s.language));
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["picks"],
        "properties": {
            "picks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["stepId", "note"],
                    "properties": {
                        "stepId": { "type": "integer" },
                        "note": { "type": "string" }
                    }
                }
            }
        }
    });
    let reply: PlanReply =
        llm::chat_json(&s, &s.model_name, &system, &user, "daily_plan", schema).await?;

    let mut conn = quests::lock(&db)?;
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM daily_plan WHERE plan_date = date('now', 'localtime')", [])?;
    let mut seen = HashSet::new();
    for (i, p) in reply.picks.iter().enumerate() {
        if valid.contains(&p.step_id) && seen.insert(p.step_id) {
            tx.execute(
                "INSERT INTO daily_plan (plan_date, step_id, order_index, note)
                 VALUES (date('now', 'localtime'), ?1, ?2, ?3)",
                params![p.step_id, i as i64, p.note.trim()],
            )?;
        }
    }
    tx.commit()?;
    load_today(&conn)
}

#[derive(Deserialize)]
struct DebriefReply {
    debrief: String,
}

const DEBRIEF_SYSTEM: &str = "\
Write a short end-of-day debrief for a person, from what they completed today.
- Two or three sentences. Warm and specific: name what they finished and why \
it matters for their goals. Never guilt or pressure.
- If little was completed, acknowledge the effort and suggest one small, \
concrete first move for tomorrow.";

#[tauri::command]
pub async fn debrief_today(db: State<'_, Db>) -> AppResult<Today> {
    let (s, user) = {
        let conn = quests::lock(&db)?;
        let s = settings::load(&conn)?;
        let t = load_today(&conn)?;
        let mut user = format!("Completed today ({}):\n", t.completed.len());
        for c in &t.completed {
            user += &format!("- {} (operation: {}, {} XP)\n", c.title, c.quest_title, c.xp);
        }
        if t.completed.is_empty() {
            user += "(nothing yet)\n";
        }
        if !s.goals_text.is_empty() {
            user += &format!("\nThe person's goals:\n{}\n", s.goals_text);
        }
        (s, user)
    };
    let model = if s.small_model_name.is_empty() { &s.model_name } else { &s.small_model_name };
    if model.is_empty() {
        return Err(AppError::Msg("set a model name in Config first".into()));
    }
    let system = format!("{DEBRIEF_SYSTEM}\n{}", llm::language_rule(&s.language));
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["debrief"],
        "properties": { "debrief": { "type": "string" } }
    });
    let reply: DebriefReply = llm::chat_json(&s, model, &system, &user, "debrief", schema).await?;

    let conn = quests::lock(&db)?;
    conn.execute(
        "INSERT INTO daily_debrief (plan_date, text) VALUES (date('now', 'localtime'), ?1)
         ON CONFLICT(plan_date) DO UPDATE SET text = excluded.text, created_at = datetime('now')",
        [reply.debrief.trim()],
    )?;
    load_today(&conn)
}

#[cfg(test)]
mod live {
    use super::*;

    #[test]
    #[ignore]
    fn live_plan_and_debrief() {
        let model = std::env::var("QUESTOPS_PROBE_MODEL").unwrap_or_else(|_| "gpt-5".into());
        let s = crate::llm::live::probe_settings(&model);
        let user = "Time available today: 90 minutes\n\nCandidates:\nOperation \"撰写第二章\" (Q2, due in 9 days):\n  stepId 11: 列出 10 篇核心文献 (15 min)\n  stepId 12: 起草 2.1 节 (20 min)\nOperation \"报销差旅\" (Q3, due in 1 days):\n  stepId 21: 填写报销单 (10 min)\n";
        let schema = json!({ "type": "object", "additionalProperties": false, "required": ["picks"],
            "properties": { "picks": { "type": "array", "items": { "type": "object", "additionalProperties": false,
                "required": ["stepId", "note"], "properties": { "stepId": { "type": "integer" }, "note": { "type": "string" } } } } } });
        let r: AppResult<PlanReply> = tauri::async_runtime::block_on(llm::chat_json(
            &s, &model, &format!("{PLAN_SYSTEM}\n{}", llm::language_rule("zh")), user, "daily_plan", schema));
        match r { Ok(x) => println!("OK plan {model}: {:?}", x.picks.iter().map(|p| p.step_id).collect::<Vec<_>>()), Err(e) => println!("FAILED plan {model}: {e}") }
        let schema = json!({ "type": "object", "additionalProperties": false, "required": ["debrief"],
            "properties": { "debrief": { "type": "string" } } });
        let r: AppResult<DebriefReply> = tauri::async_runtime::block_on(llm::chat_json(
            &s, &model, &format!("{DEBRIEF_SYSTEM}\n{}", llm::language_rule("zh")), "Completed today (1):\n- 列出 10 篇核心文献 (operation: 撰写第二章, 26 XP)\n", "debrief", schema));
        match r { Ok(x) => println!("OK debrief {model}: {} chars", x.debrief.chars().count()), Err(e) => println!("FAILED debrief {model}: {e}") }
    }
}
