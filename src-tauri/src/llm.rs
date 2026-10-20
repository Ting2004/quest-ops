//! OpenAI client. Uses Chat Completions with strict JSON-schema output so every
//! response deserializes straight into a Rust type.

use std::time::Duration;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tauri::State;

use crate::{
    db::Db,
    error::{AppError, AppResult},
    quests, rewards, secrets,
    settings::{self, Settings},
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionCheck {
    pub ok: bool,
    pub model: String,
    pub message: String,
}

fn load_settings(db: &Db) -> AppResult<Settings> {
    let conn = quests::lock(db)?;
    settings::load(&conn)
}

fn require_key() -> AppResult<String> {
    secrets::get_api_key()?.ok_or_else(|| AppError::Msg("no API key saved. Open Config.".into()))
}

fn client() -> AppResult<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(Duration::from_secs(180)).build()?)
}

/// Looks up the configured model via `GET /models/{id}`. This confirms the key
/// works and the model name exists without spending any tokens.
#[tauri::command]
pub async fn test_llm_connection(db: State<'_, Db>) -> AppResult<ConnectionCheck> {
    let s = load_settings(&db)?;
    if s.model_name.is_empty() {
        return Err(AppError::Msg("set a model name first".into()));
    }
    let key = require_key()?;

    let url = format!("{}/models/{}", s.api_base_url, s.model_name);
    let res = client()?.get(url).bearer_auth(key).send().await?;
    let status = res.status();

    let message = match status.as_u16() {
        200 => "Link established. Key and model verified.".to_string(),
        401 => "API key rejected (401). Check the key.".to_string(),
        404 => format!("Model \"{}\" not found (404). Check the exact model ID.", s.model_name),
        429 => "Rate limited or out of credit (429).".to_string(),
        code => format!("Unexpected response ({code})."),
    };
    Ok(ConnectionCheck { ok: status.is_success(), model: s.model_name, message })
}

/// Sends one system + user exchange and parses the schema-constrained reply.
pub(crate) async fn chat_json<T: DeserializeOwned>(
    s: &Settings,
    model: &str,
    system: &str,
    user: &str,
    schema_name: &str,
    schema: Value,
) -> AppResult<T> {
    let key = require_key()?;
    let body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": { "name": schema_name, "strict": true, "schema": schema },
        },
    });

    let res = client()?
        .post(format!("{}/chat/completions", s.api_base_url))
        .bearer_auth(key)
        .json(&body)
        .send()
        .await?;
    let status = res.status();
    let payload: Value = res.json().await?;

    if !status.is_success() {
        let detail = payload["error"]["message"].as_str().unwrap_or("no details");
        return Err(AppError::Msg(format!("OpenAI error ({}): {detail}", status.as_u16())));
    }

    let choice = &payload["choices"][0];
    if let Some(refusal) = choice["message"]["refusal"].as_str() {
        return Err(AppError::Msg(format!("model declined: {refusal}")));
    }
    if choice["finish_reason"] == "length" {
        return Err(AppError::Msg("model reply was cut off. Try a shorter task.".into()));
    }
    let content = choice["message"]["content"]
        .as_str()
        .ok_or_else(|| AppError::Msg("model returned no content".into()))?;
    serde_json::from_str(content)
        .map_err(|e| AppError::Msg(format!("could not parse model reply: {e}")))
}

// ---------------------------------------------------------------- breakdown

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakdownStep {
    pub title: String,
    pub done_when: String,
    pub effort: i64,
    pub est_minutes: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Breakdown {
    pub quest_title: String,
    pub clarifying_questions: Vec<String>,
    pub steps: Vec<BreakdownStep>,
}

#[derive(Debug, Deserialize)]
pub struct Clarification {
    pub question: String,
    pub answer: String,
}

const BREAKDOWN_SYSTEM: &str = "\
You turn a large, vague task into a short sequence of concrete objectives that a \
person can start immediately. Rules:
- Return 2 to 6 objectives in the order they should be done.
- Each objective is a meaningful 10 to 20 minute chunk of work. Do not split \
work into tiny actions (opening a file, creating a document, finding a link); \
fold those into the objective they belong to.
- Each title starts with a concrete verb (draft, outline, review, email, read).
- Each objective has a doneWhen criterion that makes completion unambiguous.
- Make the first objective the easiest entry point into the work.
- effort is 1 (trivial), 2 (light), 3 (focused) or 5 (demanding).
- estMinutes is a realistic estimate for this person, between 10 and 20.
- If the task covers more work than 6 objectives can hold, cover the first \
working session and make the last objective \"Plan the next session\".
- If the task is too vague to plan sensibly, return 1 to 3 clarifyingQuestions \
and an empty steps list. Otherwise return an empty clarifyingQuestions list.
- questTitle is a short, specific name for the whole task (max 60 characters).";

fn breakdown_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["questTitle", "clarifyingQuestions", "steps"],
        "properties": {
            "questTitle": { "type": "string" },
            "clarifyingQuestions": { "type": "array", "items": { "type": "string" } },
            "steps": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["title", "doneWhen", "effort", "estMinutes"],
                    "properties": {
                        "title": { "type": "string" },
                        "doneWhen": { "type": "string" },
                        "effort": { "type": "integer", "enum": [1, 2, 3, 5] },
                        "estMinutes": { "type": "integer" }
                    }
                }
            }
        }
    })
}

#[tauri::command]
pub async fn breakdown_quest(
    db: State<'_, Db>,
    quest_id: i64,
    answers: Vec<Clarification>,
) -> AppResult<Breakdown> {
    let (s, quest, calibration) = {
        let conn = quests::lock(&db)?;
        (
            settings::load(&conn)?,
            quests::load_quest(&conn, quest_id)?,
            rewards::calibration_for(&conn, quest_id)?,
        )
    };
    if s.model_name.is_empty() {
        return Err(AppError::Msg("set a model name in Config first".into()));
    }

    let mut user = format!("Today: {}\nTask: {}\n", today(), quest.title);
    if !quest.notes.is_empty() {
        user += &format!("Notes: {}\n", quest.notes);
    }
    if let Some(c) = &quest.category_name {
        user += &format!("Category: {c}\n");
    }
    if let Some(d) = &quest.deadline {
        user += &format!("Deadline: {d}\n");
    }
    if !s.goals_text.is_empty() {
        user += &format!("\nThe person's goals, for context:\n{}\n", s.goals_text);
    }
    if let Some(r) = calibration.filter(|r| (r - 1.0).abs() >= 0.15) {
        user += &format!(
            "\nMeasured from past timed runs, this person takes about {r:.1}x as long as \
             they estimate for this kind of work. Make estMinutes the real time it will \
             take them.\n"
        );
    }
    let answered: Vec<_> = answers.iter().filter(|a| !a.answer.trim().is_empty()).collect();
    if !answered.is_empty() {
        user += "\nAnswers to your earlier questions (do not ask again, produce steps):\n";
        for a in answered {
            user += &format!("Q: {}\nA: {}\n", a.question, a.answer.trim());
        }
    }

    let system = format!("{BREAKDOWN_SYSTEM}\n{}", language_rule(&s.language));
    let mut b: Breakdown =
        chat_json(&s, &s.model_name, &system, &user, "quest_breakdown", breakdown_schema()).await?;
    for step in &mut b.steps {
        step.est_minutes = step.est_minutes.clamp(1, 240);
    }
    Ok(b)
}

/// System-prompt line fixing the language of every text field the model writes.
pub(crate) fn language_rule(language: &str) -> &'static str {
    match language {
        "zh" => "- Write every text field (titles, criteria, questions, rationale) in \
                 Simplified Chinese, whatever language the task is written in.",
        _ => "- Write every text field in English, whatever language the task is written in.",
    }
}

/// Local date as YYYY-MM-DD, via SQLite so no date crate is needed.
pub(crate) fn today() -> String {
    rusqlite::Connection::open_in_memory()
        .and_then(|c| c.query_row("SELECT date('now', 'localtime')", [], |r| r.get(0)))
        .unwrap_or_default()
}
