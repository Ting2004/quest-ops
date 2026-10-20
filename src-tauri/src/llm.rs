//! Chat client for OpenAI and OpenAI-compatible APIs (DeepSeek, custom).
//! OpenAI gets strict JSON-schema output; providers without it get JSON mode
//! with the schema spelled out in the prompt. Either way every reply
//! deserializes straight into a Rust type.

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

/// Models listed back to the interface after a link test.
const MAX_LISTED_MODELS: usize = 40;
/// Output budget for JSON-mode providers, which may truncate without one.
const JSON_MODE_MAX_TOKENS: u32 = 4096;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionCheck {
    pub ok: bool,
    pub model: String,
    pub message: String,
    /// Model IDs the key can use, so the person can pick an exact one.
    pub available: Vec<String>,
}

fn provider_name(provider: &str) -> &'static str {
    match provider {
        "deepseek" => "DeepSeek",
        "custom" => "API",
        _ => "OpenAI",
    }
}

/// Only OpenAI is relied on for strict `json_schema` output.
fn supports_json_schema(provider: &str) -> bool {
    provider == "openai"
}

fn load_settings(db: &Db) -> AppResult<Settings> {
    let conn = quests::lock(db)?;
    settings::load(&conn)
}

fn require_key(s: &Settings) -> AppResult<String> {
    secrets::get_api_key(&s.provider)?.ok_or_else(|| {
        AppError::Msg(format!("no {} API key saved. Open Config.", provider_name(&s.provider)))
    })
}

fn client() -> AppResult<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(Duration::from_secs(180)).build()?)
}

/// Lists models via `GET /models` (supported by OpenAI and DeepSeek). This
/// confirms the key works and the model name exists without spending tokens.
#[tauri::command]
pub async fn test_llm_connection(db: State<'_, Db>) -> AppResult<ConnectionCheck> {
    let s = load_settings(&db)?;
    let key = require_key(&s)?;
    let res = client()?.get(format!("{}/models", s.api_base_url)).bearer_auth(key).send().await?;
    let status = res.status();
    let name = provider_name(&s.provider);

    let fail = |message: String| ConnectionCheck {
        ok: false,
        model: s.model_name.clone(),
        message,
        available: Vec::new(),
    };
    match status.as_u16() {
        200 => {}
        401 => return Ok(fail(format!("{name} rejected the API key (401). Check the key."))),
        404 => return Ok(fail(format!("No model list at this address (404). Check the API base URL."))),
        429 => return Ok(fail("Rate limited or out of credit (429).".into())),
        code => return Ok(fail(format!("Unexpected response from {name} ({code})."))),
    }

    let payload: Value = res.json().await.unwrap_or(Value::Null);
    let mut available: Vec<String> = payload["data"]
        .as_array()
        .map(|a| a.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
        .unwrap_or_default();
    available.sort();
    available.truncate(MAX_LISTED_MODELS);

    let wanted: Vec<&str> =
        [&s.model_name, &s.small_model_name].into_iter().map(String::as_str).filter(|m| !m.is_empty()).collect();
    let missing: Vec<&str> =
        wanted.iter().copied().filter(|m| !available.iter().any(|a| a == m)).collect();

    let (ok, message) = if s.model_name.is_empty() {
        (false, format!("{name} key works. Pick a model below."))
    } else if available.is_empty() {
        (true, format!("{name} key works. The model list was empty, so the model name is unchecked."))
    } else if missing.is_empty() {
        (true, format!("Link established with {name}. Key and model verified."))
    } else {
        (false, format!("{name} key works, but \"{}\" is not available. Pick a model below.", missing.join("\", \"")))
    };
    Ok(ConnectionCheck { ok, model: s.model_name, message, available })
}

/// One request. Returns the reply text, or an error that is not worth retrying.
async fn request(s: &Settings, key: &str, body: &Value) -> AppResult<Option<String>> {
    let res = client()?
        .post(format!("{}/chat/completions", s.api_base_url))
        .bearer_auth(key)
        .json(body)
        .send()
        .await?;
    let status = res.status();
    let payload: Value = res.json().await?;

    if !status.is_success() {
        let detail = payload["error"]["message"].as_str().unwrap_or("no details");
        return Err(AppError::Msg(format!(
            "{} error ({}): {detail}",
            provider_name(&s.provider),
            status.as_u16()
        )));
    }

    let choice = &payload["choices"][0];
    if let Some(refusal) = choice["message"]["refusal"].as_str() {
        return Err(AppError::Msg(format!("model declined: {refusal}")));
    }
    if choice["finish_reason"] == "length" {
        return Err(AppError::Msg("model reply was cut off. Try a shorter task.".into()));
    }
    // DeepSeek's JSON mode can occasionally return empty content.
    Ok(choice["message"]["content"].as_str().map(str::trim).filter(|c| !c.is_empty()).map(String::from))
}

/// Strips a ```json fence some JSON-mode models wrap around their reply.
fn unfence(text: &str) -> &str {
    let t = text.trim();
    let t = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")).unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}

/// Sends one system + user exchange and parses the schema-shaped reply,
/// asking once more if the first reply is empty or does not fit the schema.
pub(crate) async fn chat_json<T: DeserializeOwned>(
    s: &Settings,
    model: &str,
    system: &str,
    user: &str,
    schema_name: &str,
    schema: Value,
) -> AppResult<T> {
    let key = require_key(s)?;
    let strict = supports_json_schema(&s.provider);
    let system = if strict {
        system.to_string()
    } else {
        format!(
            "{system}\n\nReply with a single JSON object and nothing else. It must match \
             this JSON schema exactly, with every listed field present:\n{schema}"
        )
    };
    let mut messages = vec![
        json!({ "role": "system", "content": system }),
        json!({ "role": "user", "content": user }),
    ];

    let mut last_error = String::new();
    for _attempt in 0..2 {
        let mut body = json!({ "model": model, "messages": messages });
        if strict {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": { "name": schema_name, "strict": true, "schema": schema },
            });
        } else {
            body["response_format"] = json!({ "type": "json_object" });
            body["max_tokens"] = json!(JSON_MODE_MAX_TOKENS);
        }

        match request(s, &key, &body).await? {
            None => {
                last_error = "model returned no content".into();
                messages.push(json!({ "role": "user", "content": "Your reply was empty. Reply with the JSON object." }));
            }
            Some(text) => match serde_json::from_str::<T>(unfence(&text)) {
                Ok(v) => return Ok(v),
                Err(e) => {
                    last_error = format!("could not parse model reply: {e}");
                    messages.push(json!({ "role": "assistant", "content": text }));
                    messages.push(json!({
                        "role": "user",
                        "content": format!("That JSON does not match the schema ({e}). Reply again with only the corrected JSON object."),
                    }));
                }
            },
        }
    }
    Err(AppError::Msg(last_error))
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
        step.effort = snap_effort(step.effort);
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

/// Nearest valid effort (1, 2, 3 or 5); JSON-mode models may return others.
pub(crate) fn snap_effort(effort: i64) -> i64 {
    match effort {
        ..=1 => 1,
        2 => 2,
        3 => 3,
        _ => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_mode_replies_are_cleaned_up() {
        assert_eq!(unfence("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(unfence("  {\"a\":1} "), "{\"a\":1}");
        assert_eq!(snap_effort(0), 1);
        assert_eq!(snap_effort(4), 5);
        assert_eq!(snap_effort(3), 3);
        assert!(supports_json_schema("openai"));
        assert!(!supports_json_schema("deepseek"));
    }
}
