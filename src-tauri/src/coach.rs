//! AI help while working: splitting an objective that is too big, and
//! finding a smaller entry point when stuck.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::State;

use crate::{
    db::Db,
    error::{AppError, AppResult},
    llm::{self, BreakdownStep},
    quests::{self, Step, StepDraft},
    settings::{self, Settings},
    timer,
};

struct StepContext {
    quest_id: i64,
    quest_title: String,
    quest_notes: String,
    title: String,
    done_when: String,
    est_minutes: i64,
    order_index: i64,
}

fn step_context(conn: &Connection, step_id: i64) -> AppResult<StepContext> {
    let (ctx, status) = conn
        .query_row(
            "SELECT s.quest_id, q.title, q.notes, s.title, s.done_when, s.est_minutes,
                    s.order_index, s.status
             FROM step s JOIN quest q ON q.id = s.quest_id WHERE s.id = ?1",
            [step_id],
            |r| {
                Ok((
                    StepContext {
                        quest_id: r.get(0)?,
                        quest_title: r.get(1)?,
                        quest_notes: r.get(2)?,
                        title: r.get(3)?,
                        done_when: r.get(4)?,
                        est_minutes: r.get(5)?,
                        order_index: r.get(6)?,
                    },
                    r.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::Msg("objective not found".into()))?;
    if status == "done" {
        return Err(AppError::Msg("objective already completed".into()));
    }
    Ok(ctx)
}

fn fast_model(s: &Settings) -> AppResult<&str> {
    if s.model_name.is_empty() {
        return Err(AppError::Msg("set a model name in Config first".into()));
    }
    Ok(if s.small_model_name.is_empty() { &s.model_name } else { &s.small_model_name })
}

fn describe(ctx: &StepContext) -> String {
    let mut out = format!("Operation: {}\n", ctx.quest_title);
    if !ctx.quest_notes.is_empty() {
        out += &format!("Operation notes: {}\n", ctx.quest_notes);
    }
    out += &format!(
        "Objective: {}\nDone when: {}\nEstimate: {} minutes\n",
        ctx.title, ctx.done_when, ctx.est_minutes
    );
    out
}

fn step_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["title", "doneWhen", "effort", "estMinutes"],
        "properties": {
            "title": { "type": "string" },
            "doneWhen": { "type": "string" },
            "effort": { "type": "integer", "enum": [1, 2, 3, 5] },
            "estMinutes": { "type": "integer" }
        }
    })
}

fn to_draft(b: &BreakdownStep) -> StepDraft {
    StepDraft {
        title: b.title.trim().to_string(),
        done_when: b.done_when.trim().to_string(),
        effort: llm::snap_effort(b.effort),
        est_minutes: b.est_minutes.clamp(1, 240),
    }
}

// ------------------------------------------------------------------ split

#[derive(Deserialize)]
struct SplitReply {
    steps: Vec<BreakdownStep>,
}

const SPLIT_SYSTEM: &str = "\
You split one objective that turned out too big into smaller objectives.
- Return 2 to 4 objectives that together finish the original objective.
- Each takes 5 to 15 minutes and starts with a concrete verb.
- Each has a doneWhen criterion that makes completion unambiguous.
- If the person already worked on it, cover only the remaining work.
- effort is 1 (trivial), 2 (light), 3 (focused) or 5 (demanding).";

/// Replaces an objective with 2 to 4 smaller ones in the same position. A run
/// in progress on it is ended without XP first.
#[tauri::command]
pub async fn split_step(
    db: State<'_, Db>,
    step_id: i64,
    worked_minutes: Option<i64>,
) -> AppResult<Vec<Step>> {
    let (s, ctx) = {
        let conn = quests::lock(&db)?;
        (settings::load(&conn)?, step_context(&conn, step_id)?)
    };
    let model = fast_model(&s)?;
    let mut user = describe(&ctx);
    if let Some(m) = worked_minutes.filter(|m| *m > 0) {
        user += &format!("Already worked on it for {m} minutes.\n");
    }
    let system = format!("{SPLIT_SYSTEM}\n{}", llm::language_rule(&s.language));
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["steps"],
        "properties": { "steps": { "type": "array", "items": step_schema() } }
    });
    let reply: SplitReply = llm::chat_json(&s, model, &system, &user, "split_objective", schema).await?;
    let drafts: Vec<StepDraft> = reply.steps.iter().map(to_draft).collect();
    if drafts.is_empty() {
        return Err(AppError::Msg("the AI returned no smaller objectives".into()));
    }
    for (i, d) in drafts.iter().enumerate() {
        quests::validate_draft(d, i + 1)?;
    }

    let mut conn = quests::lock(&db)?;
    if timer::active(&conn)?.is_some_and(|t| t.step_id == step_id) {
        timer::abort_active(&conn)?;
    }
    let tx = conn.transaction()?;
    let extra = drafts.len() as i64 - 1;
    tx.execute(
        "UPDATE step SET order_index = order_index + ?1 WHERE quest_id = ?2 AND order_index > ?3",
        params![extra, ctx.quest_id, ctx.order_index],
    )?;
    let first = &drafts[0];
    tx.execute(
        "UPDATE step SET title = ?1, done_when = ?2, effort = ?3, est_minutes = ?4,
                status = 'pending', pauses_used = 0
         WHERE id = ?5",
        params![first.title, first.done_when, first.effort, first.est_minutes, step_id],
    )?;
    for (i, d) in drafts.iter().enumerate().skip(1) {
        tx.execute(
            "INSERT INTO step (quest_id, title, done_when, effort, est_minutes, order_index)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![ctx.quest_id, d.title, d.done_when, d.effort, d.est_minutes, ctx.order_index + i as i64],
        )?;
    }
    tx.commit()?;
    quests::steps_for(&conn, ctx.quest_id)
}

// ---------------------------------------------------------------- unstick

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnstickReply {
    pub advice: String,
    pub step: BreakdownStep,
}

const UNSTICK_SYSTEM: &str = "\
A person is stuck on an objective. Help them restart.
- advice: two or three short, kind, practical sentences. Name the likely \
blocker and one way around it. No lecturing.
- step: one tiny objective that gets them moving again, 3 to 10 minutes, \
starting with a concrete verb, with a doneWhen criterion. It should come \
before the stuck objective and make it easier.
- effort is 1 (trivial) or 2 (light).";

#[tauri::command]
pub async fn unstick_step(db: State<'_, Db>, step_id: i64, reason: String) -> AppResult<UnstickReply> {
    let (s, ctx) = {
        let conn = quests::lock(&db)?;
        (settings::load(&conn)?, step_context(&conn, step_id)?)
    };
    let model = fast_model(&s)?;
    let mut user = describe(&ctx);
    let reason = reason.trim();
    user += &format!(
        "What the person says is in the way: {}\n",
        if reason.is_empty() { "(not stated)" } else { reason }
    );
    let system = format!("{UNSTICK_SYSTEM}\n{}", llm::language_rule(&s.language));
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["advice", "step"],
        "properties": { "advice": { "type": "string" }, "step": step_schema() }
    });
    let mut reply: UnstickReply =
        llm::chat_json(&s, model, &system, &user, "unstick", schema).await?;
    reply.step.est_minutes = reply.step.est_minutes.clamp(1, 30);
    Ok(reply)
}

/// Inserts an objective before `before_step_id`, or at the end when `None`.
#[tauri::command]
pub fn insert_step(
    db: State<'_, Db>,
    quest_id: i64,
    before_step_id: Option<i64>,
    draft: StepDraft,
) -> AppResult<Step> {
    quests::validate_draft(&draft, 1)?;
    let mut conn = quests::lock(&db)?;
    let tx = conn.transaction()?;
    let index: i64 = match before_step_id {
        Some(id) => {
            let idx: i64 = tx.query_row(
                "SELECT order_index FROM step WHERE id = ?1 AND quest_id = ?2",
                params![id, quest_id],
                |r| r.get(0),
            )?;
            tx.execute(
                "UPDATE step SET order_index = order_index + 1
                 WHERE quest_id = ?1 AND order_index >= ?2",
                params![quest_id, idx],
            )?;
            idx
        }
        None => tx.query_row(
            "SELECT COALESCE(MAX(order_index) + 1, 0) FROM step WHERE quest_id = ?1",
            [quest_id],
            |r| r.get(0),
        )?,
    };
    tx.execute(
        "INSERT INTO step (quest_id, title, done_when, effort, est_minutes, order_index)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![quest_id, draft.title.trim(), draft.done_when.trim(), draft.effort, draft.est_minutes, index],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    quests::steps_for(&conn, quest_id)?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| AppError::Msg("inserted objective not found".into()))
}

#[cfg(test)]
mod live {
    use super::*;

    #[test]
    #[ignore]
    fn live_split_and_unstick() {
        let model = std::env::var("QUESTOPS_PROBE_MODEL").unwrap_or_else(|_| "gpt-5".into());
        let s = llm::live::probe_settings(&model);
        let user = "Operation: 撰写第二章\nObjective: 起草 2.1 节\nDone when: 2.1 节有 500 字草稿\nEstimate: 20 minutes\n";
        let split_schema = json!({ "type": "object", "additionalProperties": false, "required": ["steps"],
            "properties": { "steps": { "type": "array", "items": step_schema() } } });
        let r: AppResult<SplitReply> = tauri::async_runtime::block_on(llm::chat_json(
            &s, &model, &format!("{SPLIT_SYSTEM}\n{}", llm::language_rule("zh")), user, "split_objective", split_schema));
        match r { Ok(x) => println!("OK split {model}: {} steps", x.steps.len()), Err(e) => println!("FAILED split {model}: {e}") }
        let unstick_schema = json!({ "type": "object", "additionalProperties": false, "required": ["advice", "step"],
            "properties": { "advice": { "type": "string" }, "step": step_schema() } });
        let r: AppResult<UnstickReply> = tauri::async_runtime::block_on(llm::chat_json(
            &s, &model, &format!("{UNSTICK_SYSTEM}\n{}", llm::language_rule("zh")), &format!("{user}What the person says is in the way: 不知道从哪开始\n"), "unstick", unstick_schema));
        match r { Ok(x) => println!("OK unstick {model}: {:?}", x.step.title), Err(e) => println!("FAILED unstick {model}: {e}") }
    }
}
