use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::{
    db::Db,
    error::{AppError, AppResult},
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quest {
    pub id: i64,
    pub title: String,
    pub notes: String,
    pub category_id: Option<i64>,
    pub category_name: Option<String>,
    pub status: String,
    pub deadline: Option<String>,
    pub created_at: String,
    pub step_count: i64,
    pub total_minutes: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub id: i64,
    pub title: String,
    pub done_when: String,
    pub effort: i64,
    pub est_minutes: i64,
    pub order_index: i64,
    pub status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewQuest {
    pub title: String,
    pub notes: String,
    pub category_id: Option<i64>,
    pub deadline: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepDraft {
    pub title: String,
    pub done_when: String,
    pub effort: i64,
    pub est_minutes: i64,
}

pub fn lock(db: &Db) -> AppResult<std::sync::MutexGuard<'_, Connection>> {
    db.0.lock().map_err(|_| AppError::Msg("database lock poisoned".into()))
}

const QUEST_SELECT: &str = "
    SELECT q.id, q.title, q.notes, q.category_id, c.name, q.status, q.deadline, q.created_at,
           COUNT(s.id), COALESCE(SUM(s.est_minutes), 0)
    FROM quest q
    LEFT JOIN category c ON c.id = q.category_id
    LEFT JOIN step s ON s.quest_id = q.id AND s.parent_step_id IS NULL";

fn map_quest(r: &rusqlite::Row) -> rusqlite::Result<Quest> {
    Ok(Quest {
        id: r.get(0)?,
        title: r.get(1)?,
        notes: r.get(2)?,
        category_id: r.get(3)?,
        category_name: r.get(4)?,
        status: r.get(5)?,
        deadline: r.get(6)?,
        created_at: r.get(7)?,
        step_count: r.get(8)?,
        total_minutes: r.get(9)?,
    })
}

pub fn load_quest(conn: &Connection, id: i64) -> AppResult<Quest> {
    conn.query_row(&format!("{QUEST_SELECT} WHERE q.id = ?1 GROUP BY q.id"), [id], map_quest)
        .optional()?
        .ok_or_else(|| AppError::Msg(format!("operation {id} not found")))
}

fn clean_deadline(d: Option<String>) -> Option<String> {
    d.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[tauri::command]
pub fn list_categories(db: State<'_, Db>) -> AppResult<Vec<Category>> {
    let conn = lock(&db)?;
    let mut stmt = conn.prepare("SELECT id, name FROM category ORDER BY id")?;
    let rows = stmt.query_map([], |r| Ok(Category { id: r.get(0)?, name: r.get(1)? }))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[tauri::command]
pub fn list_quests(db: State<'_, Db>, status: String) -> AppResult<Vec<Quest>> {
    let conn = lock(&db)?;
    let mut stmt = conn.prepare(&format!(
        "{QUEST_SELECT} WHERE q.status = ?1 GROUP BY q.id ORDER BY q.created_at DESC, q.id DESC"
    ))?;
    let rows = stmt.query_map([status], map_quest)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[tauri::command]
pub fn create_quest(db: State<'_, Db>, quest: NewQuest) -> AppResult<Quest> {
    let title = quest.title.trim();
    if title.is_empty() {
        return Err(AppError::Msg("task title is required".into()));
    }
    let conn = lock(&db)?;
    conn.execute(
        "INSERT INTO quest (title, notes, category_id, deadline) VALUES (?1, ?2, ?3, ?4)",
        params![title, quest.notes.trim(), quest.category_id, clean_deadline(quest.deadline)],
    )?;
    load_quest(&conn, conn.last_insert_rowid())
}

#[tauri::command]
pub fn delete_quest(db: State<'_, Db>, id: i64) -> AppResult<()> {
    lock(&db)?.execute("DELETE FROM quest WHERE id = ?1", [id])?;
    Ok(())
}

#[tauri::command]
pub fn get_steps(db: State<'_, Db>, quest_id: i64) -> AppResult<Vec<Step>> {
    steps_for(&*lock(&db)?, quest_id)
}

/// Rejects a draft objective with a message naming its position.
pub fn validate_draft(s: &StepDraft, position: usize) -> AppResult<()> {
    if s.title.trim().is_empty() {
        return Err(AppError::Msg(format!("objective {position} has no title")));
    }
    if ![1, 2, 3, 5].contains(&s.effort) {
        return Err(AppError::Msg(format!("objective {position} has invalid effort")));
    }
    if !(1..=240).contains(&s.est_minutes) {
        return Err(AppError::Msg(format!(
            "objective {position} estimate must be 1 to 240 minutes"
        )));
    }
    Ok(())
}

pub fn steps_for(conn: &Connection, quest_id: i64) -> AppResult<Vec<Step>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, done_when, effort, est_minutes, order_index, status
         FROM step WHERE quest_id = ?1 AND parent_step_id IS NULL ORDER BY order_index",
    )?;
    let rows = stmt.query_map([quest_id], |r| {
        Ok(Step {
            id: r.get(0)?,
            title: r.get(1)?,
            done_when: r.get(2)?,
            effort: r.get(3)?,
            est_minutes: r.get(4)?,
            order_index: r.get(5)?,
            status: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Replaces a quest's pending steps with the reviewed list, marks it active,
/// and queues it for priority scoring.
#[tauri::command]
pub fn deploy_quest(
    app: tauri::AppHandle,
    db: State<'_, Db>,
    quest_id: i64,
    title: String,
    steps: Vec<StepDraft>,
) -> AppResult<Quest> {
    let title = title.trim();
    if title.is_empty() {
        return Err(AppError::Msg("operation title is required".into()));
    }
    if steps.is_empty() {
        return Err(AppError::Msg("add at least one objective".into()));
    }
    for (i, s) in steps.iter().enumerate() {
        validate_draft(s, i + 1)?;
    }

    let mut conn = lock(&db)?;
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM step WHERE quest_id = ?1 AND status = 'pending'", [quest_id])?;
    let offset: i64 = tx.query_row(
        "SELECT COALESCE(MAX(order_index) + 1, 0) FROM step WHERE quest_id = ?1",
        [quest_id],
        |r| r.get(0),
    )?;
    {
        let mut ins = tx.prepare(
            "INSERT INTO step (quest_id, title, done_when, effort, est_minutes, order_index)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for (i, s) in steps.iter().enumerate() {
            ins.execute(params![
                quest_id,
                s.title.trim(),
                s.done_when.trim(),
                s.effort,
                s.est_minutes,
                offset + i as i64
            ])?;
        }
    }
    tx.execute(
        "UPDATE quest SET title = ?1, status = 'active' WHERE id = ?2",
        params![title, quest_id],
    )?;
    tx.commit()?;
    let quest = load_quest(&conn, quest_id)?;
    drop(conn);
    crate::priority::schedule(&app, [quest_id]);
    Ok(quest)
}

// ------------------------------------------------------------ re-planning

/// An objective as edited on the planning screen; `id` is set for existing ones.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepEdit {
    pub id: Option<i64>,
    pub title: String,
    pub done_when: String,
    pub effort: i64,
    pub est_minutes: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestPlan {
    pub quest: Quest,
    pub importance: Option<i64>,
    /// All objectives in order, finished ones included (shown read-only).
    pub steps: Vec<Step>,
}

#[tauri::command]
pub fn get_quest_plan(db: State<'_, Db>, quest_id: i64) -> AppResult<QuestPlan> {
    let conn = lock(&db)?;
    let importance =
        conn.query_row("SELECT importance FROM quest WHERE id = ?1", [quest_id], |r| r.get(0))?;
    Ok(QuestPlan { quest: load_quest(&conn, quest_id)?, importance, steps: steps_for(&conn, quest_id)? })
}

/// Operation-level fields edited on the planning screen.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDetails {
    pub title: String,
    #[serde(default)]
    pub notes: String,
    pub category_id: Option<i64>,
    /// YYYY-MM-DD, or empty/None for no deadline.
    pub deadline: Option<String>,
    pub importance: Option<i64>,
}

fn valid_date(d: &str) -> bool {
    let b = d.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Rewrites a deployed operation: its details (name, notes, category,
/// deadline, importance) and its open objectives. Edited objectives are
/// updated, new ones added, removed ones deleted, and all re-ordered after the
/// finished objectives, which are left untouched. Setting `importance` records
/// the person's own rating, which background AI scoring then leaves alone.
pub fn update_plan_in(
    conn: &mut Connection,
    quest_id: i64,
    details: &PlanDetails,
    steps: &[StepEdit],
) -> AppResult<()> {
    let title = details.title.trim();
    let importance = details.importance;
    let deadline = clean_deadline(details.deadline.clone());
    if title.is_empty() {
        return Err(AppError::Msg("operation title is required".into()));
    }
    if steps.is_empty() {
        return Err(AppError::Msg("add at least one objective".into()));
    }
    if importance.is_some_and(|i| !(1..=5).contains(&i)) {
        return Err(AppError::Msg("importance must be 1 to 5".into()));
    }
    if deadline.as_deref().is_some_and(|d| !valid_date(d)) {
        return Err(AppError::Msg("deadline must be a date (YYYY-MM-DD)".into()));
    }
    for (i, s) in steps.iter().enumerate() {
        let draft = StepDraft {
            title: s.title.clone(),
            done_when: s.done_when.clone(),
            effort: s.effort,
            est_minutes: s.est_minutes,
        };
        validate_draft(&draft, i + 1)?;
    }

    let tx = conn.transaction()?;
    let open: Vec<(i64, String)> = tx
        .prepare(
            "SELECT id, status FROM step WHERE quest_id = ?1 AND parent_step_id IS NULL
             AND status IN ('pending', 'active')",
        )?
        .query_map([quest_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let kept: Vec<i64> = steps.iter().filter_map(|s| s.id).collect();
    if let Some(bad) = kept.iter().find(|id| !open.iter().any(|(o, _)| o == *id)) {
        return Err(AppError::Msg(format!("objective {bad} is not an open objective of this operation")));
    }
    for (id, status) in &open {
        if !kept.contains(id) {
            if status == "active" {
                return Err(AppError::Msg("stop the running objective before removing it".into()));
            }
            tx.execute("DELETE FROM step WHERE id = ?1", [id])?;
        }
    }

    let base: i64 = tx.query_row(
        "SELECT COALESCE(MAX(order_index) + 1, 0) FROM step
         WHERE quest_id = ?1 AND parent_step_id IS NULL AND status = 'done'",
        [quest_id],
        |r| r.get(0),
    )?;
    for (i, s) in steps.iter().enumerate() {
        let order = base + i as i64;
        match s.id {
            Some(id) => tx.execute(
                "UPDATE step SET title = ?1, done_when = ?2, effort = ?3, est_minutes = ?4,
                        order_index = ?5
                 WHERE id = ?6",
                params![s.title.trim(), s.done_when.trim(), s.effort, s.est_minutes, order, id],
            )?,
            None => tx.execute(
                "INSERT INTO step (quest_id, title, done_when, effort, est_minutes, order_index)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![quest_id, s.title.trim(), s.done_when.trim(), s.effort, s.est_minutes, order],
            )?,
        };
    }

    tx.execute(
        "UPDATE quest SET title = ?1, notes = ?2, category_id = ?3, deadline = ?4 WHERE id = ?5",
        params![title, details.notes.trim(), details.category_id, deadline, quest_id],
    )?;
    if let Some(i) = importance {
        // A manual rating counts as scored; urgency defaults to "no pressure"
        // beyond the deadline if the AI never scored it.
        tx.execute(
            "UPDATE quest SET importance = ?1, urgency_base = COALESCE(urgency_base, 1),
                    scored_at = COALESCE(scored_at, datetime('now'))
             WHERE id = ?2",
            params![i, quest_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[tauri::command]
pub fn update_quest_plan(
    db: State<'_, Db>,
    quest_id: i64,
    details: PlanDetails,
    steps: Vec<StepEdit>,
) -> AppResult<Quest> {
    let mut conn = lock(&db)?;
    update_plan_in(&mut conn, quest_id, &details, &steps)?;
    load_quest(&conn, quest_id)
}

/// Sends a deployed operation back to the inbox, keeping its objectives so it
/// can be re-planned and redeployed. A run in progress on it is ended without
/// XP, and its objectives leave today's plan.
pub fn move_to_inbox_in(conn: &mut Connection, quest_id: i64) -> AppResult<()> {
    let status: String = conn
        .query_row("SELECT status FROM quest WHERE id = ?1", [quest_id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| AppError::Msg(format!("operation {quest_id} not found")))?;
    if status != "active" {
        return Err(AppError::Msg("only deployed operations can move back to the inbox".into()));
    }
    if crate::timer::active(conn)?.is_some_and(|t| t.quest_id == quest_id) {
        crate::timer::abort_active(conn)?;
    }
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM daily_plan WHERE step_id IN (SELECT id FROM step WHERE quest_id = ?1)",
        [quest_id],
    )?;
    tx.execute("UPDATE step SET status = 'pending' WHERE quest_id = ?1 AND status = 'active'", [quest_id])?;
    tx.execute("UPDATE quest SET status = 'inbox' WHERE id = ?1", [quest_id])?;
    tx.commit()?;
    Ok(())
}

#[tauri::command]
pub fn move_to_inbox(db: State<'_, Db>, quest_id: i64) -> AppResult<Quest> {
    let mut conn = lock(&db)?;
    move_to_inbox_in(&mut conn, quest_id)?;
    load_quest(&conn, quest_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrate_for_test(&mut conn);
        conn.execute_batch(
            "INSERT INTO quest (id, title, status) VALUES (1, 'Op', 'active');
             INSERT INTO step (id, quest_id, title, est_minutes, order_index, status) VALUES
                 (1, 1, 'Finished', 10, 0, 'done'),
                 (2, 1, 'Keep me', 10, 1, 'pending'),
                 (3, 1, 'Drop me', 10, 2, 'pending'),
                 (4, 1, 'Running', 10, 3, 'active');",
        )
        .unwrap();
        conn
    }

    fn details(title: &str) -> PlanDetails {
        PlanDetails { title: title.into(), notes: String::new(), category_id: None, deadline: None, importance: None }
    }

    fn edit(id: Option<i64>, title: &str, est: i64) -> StepEdit {
        StepEdit { id, title: title.into(), done_when: String::new(), effort: 2, est_minutes: est }
    }

    fn titles(conn: &Connection) -> Vec<(String, String, i64)> {
        conn.prepare("SELECT title, status, est_minutes FROM step ORDER BY order_index")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn replanning_edits_adds_reorders_and_keeps_finished_work() {
        let mut conn = seeded();
        let steps =
            [edit(Some(4), "Running", 10), edit(None, "Brand new", 12), edit(Some(2), "Kept, renamed", 25)];
        let edited = PlanDetails {
            title: "Op v2".into(),
            notes: "new notes".into(),
            category_id: Some(3),
            deadline: Some("2026-12-24".into()),
            importance: Some(5),
        };
        update_plan_in(&mut conn, 1, &edited, &steps).unwrap();
        assert_eq!(
            titles(&conn),
            [
                ("Finished".into(), "done".into(), 10),
                ("Running".into(), "active".into(), 10),
                ("Brand new".into(), "pending".into(), 12),
                ("Kept, renamed".into(), "pending".into(), 25),
            ]
        );
        let (title, importance, urgency): (String, i64, i64) = conn
            .query_row("SELECT title, importance, urgency_base FROM quest WHERE id = 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!((title.as_str(), importance, urgency), ("Op v2", 5, 1));
        let (notes, deadline, cat): (String, String, i64) = conn
            .query_row("SELECT notes, deadline, category_id FROM quest WHERE id = 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!((notes.as_str(), deadline.as_str(), cat), ("new notes", "2026-12-24", 3));
        let bad = PlanDetails { deadline: Some("next week".into()), ..details("Op") };
        assert!(update_plan_in(&mut conn, 1, &bad, &steps).is_err(), "deadline must be a date");
    }

    #[test]
    fn replanning_refuses_to_drop_the_running_objective() {
        let mut conn = seeded();
        let err = update_plan_in(&mut conn, 1, &details("Op"), &[edit(Some(2), "Keep me", 10)]);
        assert!(err.is_err());
        assert_eq!(titles(&conn).len(), 4, "nothing changed");
    }
}

#[cfg(test)]
mod inbox_tests {
    use super::*;

    #[test]
    fn moving_back_to_inbox_keeps_objectives_and_stops_its_run() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrate_for_test(&mut conn);
        conn.execute_batch(
            "INSERT INTO quest (id, title, status) VALUES (1, 'Op', 'active');
             INSERT INTO step (id, quest_id, title, est_minutes, order_index, status) VALUES
                 (1, 1, 'Done', 10, 0, 'done'), (2, 1, 'Running', 10, 1, 'active'),
                 (3, 1, 'Next', 10, 2, 'pending');
             INSERT INTO timer_session (step_id, started_at, started_ms, planned_seconds)
                 VALUES (2, datetime('now'), 0, 600);
             INSERT INTO daily_plan (plan_date, step_id, order_index) VALUES ('2026-10-21', 3, 0);",
        )
        .unwrap();
        move_to_inbox_in(&mut conn, 1).unwrap();
        let q = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(q("SELECT COUNT(*) FROM quest WHERE status = 'inbox'"), 1);
        assert_eq!(q("SELECT COUNT(*) FROM step"), 3, "objectives kept");
        assert_eq!(q("SELECT COUNT(*) FROM step WHERE status = 'done'"), 1);
        assert_eq!(q("SELECT COUNT(*) FROM step WHERE status = 'active'"), 0);
        assert_eq!(q("SELECT COUNT(*) FROM timer_session WHERE ended_at IS NULL"), 0, "run stopped");
        assert_eq!(q("SELECT COUNT(*) FROM daily_plan"), 0);
        assert!(move_to_inbox_in(&mut conn, 1).is_err(), "already in the inbox");
    }
}
