//! Eisenhower matrix: AI importance/urgency scoring, deadline-driven urgency,
//! manual quadrant overrides, and a background scoring queue.

use std::{
    collections::{HashSet, VecDeque},
    sync::Mutex,
};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{
    db::Db,
    error::{AppError, AppResult},
    llm, quests, settings,
};

/// Urgency at or above this counts as urgent.
const URGENT: i64 = 3;
/// How many recent manual moves are shown to the AI as calibration examples.
const OVERRIDE_EXAMPLES: i64 = 20;
/// Parallel AI requests from the background scoring queue.
const SCORE_WORKERS: usize = 3;

/// The user's thresholds from Config.
#[derive(Debug, Clone, Copy)]
pub struct Rules {
    pub urgent_within_days: i64,
    pub importance_threshold: i64,
}

impl Rules {
    fn load(conn: &Connection) -> AppResult<Self> {
        let s = settings::load(conn)?;
        Ok(Rules {
            urgent_within_days: s.urgent_within_days,
            importance_threshold: s.importance_threshold,
        })
    }
}

/// Urgency implied by the deadline alone, scaled to the user's urgent window `w`:
/// 3 at the edge of the window, 5 in its final third, 2 up to twice the window.
pub fn deadline_urgency(days_left: i64, w: i64) -> i64 {
    let (d, w) = (days_left as f64, w.max(0) as f64);
    if d <= w / 3.0 {
        5
    } else if d <= w * 2.0 / 3.0 {
        4
    } else if d <= w {
        3
    } else if d <= w * 2.0 {
        2
    } else {
        1
    }
}

/// The AI's non-deadline urgency, raised by deadline pressure when there is a deadline.
pub fn effective_urgency(base: Option<i64>, days_left: Option<i64>, rules: Rules) -> Option<i64> {
    let from_deadline = days_left.map(|d| deadline_urgency(d, rules.urgent_within_days));
    match (base, from_deadline) {
        (Some(b), Some(d)) => Some(b.max(d)),
        (b, d) => b.or(d),
    }
}

/// 1 = important + urgent, 2 = important, 3 = urgent, 4 = neither.
pub fn quadrant(importance: i64, urgency: i64, rules: Rules) -> i64 {
    match (importance >= rules.importance_threshold, urgency >= URGENT) {
        (true, true) => 1,
        (true, false) => 2,
        (false, true) => 3,
        (false, false) => 4,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixQuest {
    pub id: i64,
    pub title: String,
    pub notes: String,
    pub category_name: Option<String>,
    pub deadline: Option<String>,
    pub days_left: Option<i64>,
    pub importance: Option<i64>,
    pub urgency: Option<i64>,
    pub rationale: String,
    /// Where the AI scores put it; `None` until scored.
    pub ai_quadrant: Option<i64>,
    /// Where it is shown: the manual placement if any, else `ai_quadrant`.
    pub quadrant: Option<i64>,
    pub overridden: bool,
    pub step_count: i64,
    pub steps_done: i64,
    pub minutes_left: i64,
}

const MATRIX_SELECT: &str = "
    SELECT q.id, q.title, q.notes, c.name, q.deadline,
           CAST(julianday(q.deadline) - julianday(date('now', 'localtime')) AS INTEGER),
           q.importance, q.urgency_base, q.ai_rationale, q.quadrant_override,
           (SELECT COUNT(*) FROM step s WHERE s.quest_id = q.id AND s.parent_step_id IS NULL),
           (SELECT COUNT(*) FROM step s WHERE s.quest_id = q.id AND s.parent_step_id IS NULL
                AND s.status = 'done'),
           (SELECT COALESCE(SUM(s.est_minutes), 0) FROM step s WHERE s.quest_id = q.id
                AND s.parent_step_id IS NULL AND s.status IN ('pending', 'active'))
    FROM quest q
    LEFT JOIN category c ON c.id = q.category_id";

fn map_matrix(r: &rusqlite::Row, rules: Rules) -> rusqlite::Result<MatrixQuest> {
    let days_left: Option<i64> = r.get(5)?;
    let importance: Option<i64> = r.get(6)?;
    let urgency = effective_urgency(r.get(7)?, days_left, rules);
    let override_q: Option<i64> = r.get(9)?;
    let ai_quadrant = importance.zip(urgency).map(|(i, u)| quadrant(i, u, rules));
    Ok(MatrixQuest {
        id: r.get(0)?,
        title: r.get(1)?,
        notes: r.get(2)?,
        category_name: r.get(3)?,
        deadline: r.get(4)?,
        days_left,
        importance,
        urgency,
        rationale: r.get(8)?,
        ai_quadrant,
        quadrant: override_q.or(ai_quadrant),
        overridden: override_q.is_some(),
        step_count: r.get(10)?,
        steps_done: r.get(11)?,
        minutes_left: r.get(12)?,
    })
}

fn load_matrix_quest(conn: &Connection, id: i64) -> AppResult<MatrixQuest> {
    let rules = Rules::load(conn)?;
    Ok(conn.query_row(&format!("{MATRIX_SELECT} WHERE q.id = ?1"), [id], |r| {
        map_matrix(r, rules)
    })?)
}

/// Quadrant a quest is shown in right now (manual placement wins).
pub fn current_quadrant(conn: &Connection, quest_id: i64) -> AppResult<Option<i64>> {
    Ok(load_matrix_quest(conn, quest_id)?.quadrant)
}

#[tauri::command]
pub fn list_matrix(db: State<'_, Db>) -> AppResult<Vec<MatrixQuest>> {
    active_matrix(&*quests::lock(&db)?)
}

/// Every active quest with its derived priority fields.
pub fn active_matrix(conn: &Connection) -> AppResult<Vec<MatrixQuest>> {
    let rules = Rules::load(&conn)?;
    let mut stmt = conn.prepare(&format!(
        "{MATRIX_SELECT} WHERE q.status = 'active'
         ORDER BY q.deadline IS NULL, q.deadline, q.importance DESC, q.id"
    ))?;
    let rows = stmt.query_map([], |r| map_matrix(r, rules))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Manually places a quest. Moving it back to the AI's quadrant clears the override.
#[tauri::command]
pub fn set_quadrant(db: State<'_, Db>, quest_id: i64, quadrant: i64) -> AppResult<MatrixQuest> {
    if !(1..=4).contains(&quadrant) {
        return Err(AppError::Msg("quadrant must be 1 to 4".into()));
    }
    let mut conn = quests::lock(&db)?;
    let current = load_matrix_quest(&conn, quest_id)?;
    let tx = conn.transaction()?;
    if current.ai_quadrant == Some(quadrant) {
        tx.execute(
            "UPDATE quest SET quadrant_override = NULL, user_overridden = 0 WHERE id = ?1",
            [quest_id],
        )?;
    } else {
        tx.execute(
            "UPDATE quest SET quadrant_override = ?1, user_overridden = 1 WHERE id = ?2",
            params![quadrant, quest_id],
        )?;
        if let Some(ai) = current.ai_quadrant {
            tx.execute(
                "INSERT INTO priority_override (quest_id, ai_quadrant, user_quadrant)
                 VALUES (?1, ?2, ?3)",
                params![quest_id, ai, quadrant],
            )?;
        }
    }
    tx.commit()?;
    load_matrix_quest(&conn, quest_id)
}

#[tauri::command]
pub fn clear_quadrant_override(db: State<'_, Db>, quest_id: i64) -> AppResult<MatrixQuest> {
    let conn = quests::lock(&db)?;
    conn.execute(
        "UPDATE quest SET quadrant_override = NULL, user_overridden = 0 WHERE id = ?1",
        [quest_id],
    )?;
    load_matrix_quest(&conn, quest_id)
}

// ------------------------------------------------------------------ scoring

#[derive(Debug, Deserialize)]
struct Score {
    importance: i64,
    urgency: i64,
    rationale: String,
}

const SCORE_SYSTEM: &str = "\
You help a person prioritise tasks on an Eisenhower matrix. Score one task.
- importance (1 to 5): how much finishing it advances the person's stated goals \
or prevents real harm. 1 = trivial chore, 2 = useful, 3 = worthwhile, \
4 = clearly advances a stated goal, 5 = central to a stated goal. Most everyday \
tasks are 2 or 3. Across a typical list, only about a third of tasks deserve 4 or 5.
- urgency (1 to 5): time pressure OTHER THAN the deadline. The app adds deadline \
pressure itself, so do not count the deadline. Use 1 when nothing besides the \
deadline creates pressure, which is the normal case. Use 3 or more only for real \
external pressure: someone is blocked waiting on it, an opportunity closes soon, \
or harm grows every day it waits.
- rationale: one sentence (max 140 characters) naming the goal or pressure that \
drove the scores.
- If examples of the person's manual corrections are given, adjust your \
judgement toward their preferences.";

fn score_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["importance", "urgency", "rationale"],
        "properties": {
            "importance": { "type": "integer", "enum": [1, 2, 3, 4, 5] },
            "urgency": { "type": "integer", "enum": [1, 2, 3, 4, 5] },
            "rationale": { "type": "string" }
        }
    })
}

fn score_prompt(conn: &Connection, quest_id: i64, goals: &str, rules: Rules) -> AppResult<String> {
    let q = quests::load_quest(conn, quest_id)?;
    let mut user = format!("Today: {}\nTask: {}\n", llm::today(), q.title);
    if !q.notes.is_empty() {
        user += &format!("Notes: {}\n", q.notes);
    }
    if let Some(c) = &q.category_name {
        user += &format!("Category: {c}\n");
    }

    let mut stmt = conn.prepare(
        "SELECT title FROM step WHERE quest_id = ?1 AND parent_step_id IS NULL
         ORDER BY order_index",
    )?;
    let steps: Vec<String> =
        stmt.query_map([quest_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    if !steps.is_empty() {
        user += &format!("Planned objectives: {}\n", steps.join("; "));
    }

    user += &format!(
        "\nThe person's goals:\n{}\n",
        if goals.is_empty() { "(not provided)" } else { goals }
    );
    user += &format!(
        "\nThe person treats importance {} or higher as important.\n",
        rules.importance_threshold
    );

    let mut stmt = conn.prepare(
        "SELECT q.title, o.ai_quadrant, o.user_quadrant
         FROM priority_override o JOIN quest q ON q.id = o.quest_id
         ORDER BY o.id DESC LIMIT ?1",
    )?;
    let examples: Vec<String> = stmt
        .query_map([OVERRIDE_EXAMPLES], |r| {
            Ok(format!(
                "- \"{}\": AI placed Q{}, person moved it to Q{}",
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?
            ))
        })?
        .collect::<Result<_, _>>()?;
    if !examples.is_empty() {
        user += &format!(
            "\nThe person's recent manual corrections (Q1 important+urgent, Q2 important, \
             Q3 urgent, Q4 neither):\n{}\n",
            examples.join("\n")
        );
    }
    Ok(user)
}

/// Asks the AI for importance and urgency and stores them. Clears any manual placement.
async fn score_one(app: &AppHandle, quest_id: i64) -> AppResult<MatrixQuest> {
    let db = app.state::<Db>();
    let (s, prompt) = {
        let conn = quests::lock(&db)?;
        let s = settings::load(&conn)?;
        let prompt = score_prompt(&conn, quest_id, &s.goals_text, Rules::load(&conn)?)?;
        (s, prompt)
    };
    if s.model_name.is_empty() {
        return Err(AppError::Msg("set a model name in Config first".into()));
    }
    let model = if s.small_model_name.is_empty() { &s.model_name } else { &s.small_model_name };
    let system = format!("{SCORE_SYSTEM}\n{}", llm::language_rule(&s.language));

    let score: Score =
        llm::chat_json(&s, model, &system, &prompt, "priority_score", score_schema()).await?;

    let conn = quests::lock(&db)?;
    conn.execute(
        "UPDATE quest SET importance = ?1, urgency_base = ?2, ai_rationale = ?3,
                quadrant_override = NULL, user_overridden = 0, scored_at = datetime('now')
         WHERE id = ?4",
        params![
            score.importance.clamp(1, 5),
            score.urgency.clamp(1, 5),
            score.rationale.trim(),
            quest_id
        ],
    )?;
    load_matrix_quest(&conn, quest_id)
}

/// Re-scores one quest and waits for the result (the drawer's Re-score button).
#[tauri::command]
pub async fn score_quest(app: AppHandle, quest_id: i64) -> AppResult<MatrixQuest> {
    score_one(&app, quest_id).await
}

// ------------------------------------------------------------ scoring queue

/// Quests waiting for or undergoing background scoring.
#[derive(Default)]
pub struct ScoreQueue(Mutex<QueueState>);

#[derive(Default)]
struct QueueState {
    waiting: VecDeque<i64>,
    /// Everything waiting or in flight, so a quest is never queued twice.
    queued: HashSet<i64>,
    workers: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScoreFailed {
    quest_id: i64,
    message: String,
}

/// Queues quests for scoring. Results arrive as `quest-scored` /
/// `quest-score-failed` events. Returns every quest id currently queued.
pub fn schedule(app: &AppHandle, ids: impl IntoIterator<Item = i64>) -> Vec<i64> {
    let queue = app.state::<ScoreQueue>();
    let mut st = queue.0.lock().unwrap_or_else(|e| e.into_inner());
    for id in ids {
        if st.queued.insert(id) {
            st.waiting.push_back(id);
        }
    }
    while st.workers < SCORE_WORKERS && st.workers < st.waiting.len() {
        st.workers += 1;
        let app = app.clone();
        tauri::async_runtime::spawn(async move { worker(app).await });
    }
    st.queued.iter().copied().collect()
}

async fn worker(app: AppHandle) {
    loop {
        let id = {
            let queue = app.state::<ScoreQueue>();
            let mut st = queue.0.lock().unwrap_or_else(|e| e.into_inner());
            match st.waiting.pop_front() {
                Some(id) => id,
                None => {
                    st.workers -= 1;
                    return;
                }
            }
        };
        let result = score_one(&app, id).await;
        app.state::<ScoreQueue>().0.lock().unwrap_or_else(|e| e.into_inner()).queued.remove(&id);
        let _ = match result {
            Ok(q) => app.emit("quest-scored", q),
            Err(e) => app.emit("quest-score-failed", ScoreFailed { quest_id: id, message: e.to_string() }),
        };
    }
}

fn active_ids(app: &AppHandle, filter: &str) -> AppResult<Vec<i64>> {
    let db = app.state::<Db>();
    let conn = quests::lock(&db)?;
    let mut stmt = conn.prepare(&format!(
        "SELECT id FROM quest WHERE status = 'active' AND quadrant_override IS NULL AND {filter}"
    ))?;
    let ids = stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(ids)
}

/// Queues every active quest that has never been scored. Runs at startup and
/// whenever the matrix opens, which also retries earlier failures.
#[tauri::command]
pub fn score_pending(app: AppHandle) -> AppResult<Vec<i64>> {
    let ids = active_ids(&app, "importance IS NULL")?;
    Ok(schedule(&app, ids))
}

/// Re-scores every active quest that is not placed manually.
#[tauri::command]
pub fn rescore_all(app: AppHandle) -> AppResult<Vec<i64>> {
    let ids = active_ids(&app, "1 = 1")?;
    Ok(schedule(&app, ids))
}

#[cfg(test)]
mod tests {
    use super::*;

    const R3: Rules = Rules { urgent_within_days: 3, importance_threshold: 4 };
    const R10: Rules = Rules { urgent_within_days: 10, importance_threshold: 3 };

    #[test]
    fn deadline_scale_follows_user_window() {
        // window 3: urgent (>=3) from D-3, critical at D-1
        assert_eq!(deadline_urgency(4, 3), 2);
        assert_eq!(deadline_urgency(3, 3), 3);
        assert_eq!(deadline_urgency(2, 3), 4);
        assert_eq!(deadline_urgency(1, 3), 5);
        assert_eq!(deadline_urgency(7, 3), 1);
        // window 10: urgent from D-10
        assert_eq!(deadline_urgency(10, 10), 3);
        assert_eq!(deadline_urgency(11, 10), 2);
        assert_eq!(deadline_urgency(3, 10), 5);
        // window 0: only due today or overdue is urgent
        assert_eq!(deadline_urgency(0, 0), 5);
        assert_eq!(deadline_urgency(-2, 0), 5);
        assert_eq!(deadline_urgency(1, 0), 1);
    }

    #[test]
    fn effective_urgency_takes_the_higher_signal() {
        assert_eq!(effective_urgency(Some(1), None, R3), Some(1));
        assert_eq!(effective_urgency(Some(1), Some(30), R3), Some(1));
        assert_eq!(effective_urgency(Some(1), Some(2), R3), Some(4));
        assert_eq!(effective_urgency(Some(4), Some(30), R3), Some(4));
        assert_eq!(effective_urgency(None, Some(5), R10), Some(4));
        assert_eq!(effective_urgency(None, None, R3), None);
    }

    #[test]
    fn quadrant_uses_importance_threshold() {
        assert_eq!(quadrant(4, 3, R3), 1);
        assert_eq!(quadrant(3, 3, R3), 3);
        assert_eq!(quadrant(3, 3, R10), 1);
        assert_eq!(quadrant(5, 1, R3), 2);
        assert_eq!(quadrant(2, 1, R3), 4);
    }
}
