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

/// Whether a listed model can do what the app needs: chat completions that
/// return text. Speech, transcription, audio, realtime, embedding, image,
/// video, moderation and search models are left out; for OpenAI so are Codex
/// and "-pro" models, which are not served by Chat Completions.
pub fn is_chat_model(provider: &str, id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    const NOT_TEXT: &[&str] = &[
        "tts", "whisper", "transcri", "audio", "realtime", "speech", "translate", "diarize",
        "embed", "rerank", "moderation", "dall-e", "image", "sora", "video", "live-", "search",
        "deep-research", "computer-use", "-instruct", "babbage", "davinci",
    ];
    if NOT_TEXT.iter().any(|w| id.contains(w)) {
        return false;
    }
    if provider == "openai" && (id.contains("codex") || id.ends_with("-pro") || id.contains("-pro-")) {
        return false;
    }
    true
}

#[derive(Deserialize)]
struct Probe {
    #[allow(dead_code)]
    ok: bool,
}

/// One tiny request in the app's real reply format, so a model that is listed
/// but cannot actually serve the app fails here rather than mid-task.
async fn verify_model(s: &Settings, model: &str) -> AppResult<()> {
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["ok"],
        "properties": { "ok": { "type": "boolean" } }
    });
    let _: Probe = chat_json(s, model, "This is a connection check.", "Reply with ok set to true.", "probe", schema).await?;
    Ok(())
}

/// Checks the link: the key (via `GET /models`), that the chosen models are
/// listed, and that each one answers a real request in the app's format.
#[tauri::command]
pub async fn test_llm_connection(db: State<'_, Db>) -> AppResult<ConnectionCheck> {
    let s = load_settings(&db)?;
    let key = require_key(&s)?;
    let name = provider_name(&s.provider);
    if s.provider != "openai" && s.api_base_url.contains("api.openai.com") {
        return Ok(ConnectionCheck {
            ok: false,
            model: s.model_name.clone(),
            message: "This is OpenAI's address. Choose the OpenAI provider so requests use OpenAI's format.".into(),
            available: Vec::new(),
        });
    }
    let res = client()?.get(format!("{}/models", s.api_base_url)).bearer_auth(key).send().await?;
    let status = res.status();

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
    let listed = available.clone();
    available.retain(|m| is_chat_model(&s.provider, m));
    // Newest-looking names first (higher versions sort later alphabetically).
    available.sort();
    available.reverse();
    available.truncate(MAX_LISTED_MODELS);

    let mut wanted: Vec<&str> =
        [&s.model_name, &s.small_model_name].into_iter().map(String::as_str).filter(|m| !m.is_empty()).collect();
    wanted.dedup();
    let missing: Vec<&str> =
        wanted.iter().copied().filter(|m| !listed.iter().any(|a| a == m)).collect();

    if s.model_name.is_empty() {
        return Ok(ConnectionCheck { ok: false, model: s.model_name, message: format!("{name} key works. Pick a model below."), available });
    }
    if !listed.is_empty() && !missing.is_empty() {
        let message = format!("{name} key works, but \"{}\" is not available. Pick a model below.", missing.join("\", \""));
        return Ok(ConnectionCheck { ok: false, model: s.model_name, message, available });
    }
    if let Some(bad) = wanted.iter().find(|m| !is_chat_model(&s.provider, m)) {
        let message = format!("\"{bad}\" is not a text model the app can use (speech, image or similar). Pick another.");
        return Ok(ConnectionCheck { ok: false, model: s.model_name, message, available });
    }

    // A real request per model in the app's reply format.
    for m in &wanted {
        if let Err(e) = verify_model(&s, m).await {
            let message = format!("\"{m}\" is listed but failed a test request: {e}");
            return Ok(ConnectionCheck { ok: false, model: s.model_name, message, available });
        }
    }
    let message = format!("Link established with {name}. {} verified with a test request.", wanted.join(" and "));
    Ok(ConnectionCheck { ok: true, model: s.model_name, message, available })
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
            // DeepSeek's docs ask for an explicit budget. Other OpenAI-compatible
            // servers differ (newer OpenAI models reject max_tokens outright),
            // so they get none.
            if s.provider == "deepseek" {
                body["max_tokens"] = json!(JSON_MODE_MAX_TOKENS);
            }
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
- Prefer making reasonable assumptions and planning a first working session; \
early objectives can gather what is missing (e.g. \"List the sources you already \
have\"). Only if not even a sensible first objective is possible, return 1 to 3 \
clarifyingQuestions and an empty steps list. Otherwise return an empty \
clarifyingQuestions list.
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

/// Live checks against the real API with the saved key. Never run by default:
///   QUESTOPS_PROBE_MODEL=gpt-5 cargo test live_ -- --ignored --nocapture
#[cfg(test)]
pub(crate) mod live {
    use super::*;

    pub(crate) fn probe_settings(model: &str) -> Settings {
        Settings {
            commander_name: String::new(),
            goals_text: String::new(),
            model_name: model.into(),
            small_model_name: String::new(),
            api_base_url: std::env::var("QUESTOPS_PROBE_BASE").unwrap_or_else(|_| "https://api.openai.com/v1".into()),
            max_pauses: 2,
            urgent_within_days: 3,
            importance_threshold: 4,
            language: "en".into(),
            theme: "dark".into(),
            provider: std::env::var("QUESTOPS_PROBE_PROVIDER").unwrap_or_else(|_| "openai".into()),
        }
    }

    #[test]
    #[ignore]
    fn live_list_models() {
        let s = probe_settings("");
        let key = require_key(&s).expect("no saved OpenAI key");
        let ids: Vec<String> = tauri::async_runtime::block_on(async {
            let v: Value = client().unwrap().get(format!("{}/models", s.api_base_url))
                .bearer_auth(key).send().await.unwrap().json().await.unwrap();
            v["data"].as_array().unwrap().iter().filter_map(|m| m["id"].as_str().map(String::from)).collect()
        });
        let mut ids = ids;
        ids.sort();
        println!("{} models: {}", ids.len(), ids.join(", "));
    }

    #[test]
    #[ignore]
    fn live_breakdown() {
        let model = std::env::var("QUESTOPS_PROBE_MODEL").unwrap_or_else(|_| "gpt-5".into());
        let s = probe_settings(&model);
        let lang = std::env::var("QUESTOPS_PROBE_LANG").unwrap_or_else(|_| "en".into());
        let system = format!("{BREAKDOWN_SYSTEM}\n{}", language_rule(&lang));
        let user = "Today: 2026-10-21\nTask: Write the literature review for chapter 2 of my thesis\n";
        let r: AppResult<Breakdown> = tauri::async_runtime::block_on(chat_json(
            &s, &model, &system, user, "quest_breakdown", breakdown_schema(),
        ));
        match r {
            Ok(b) => println!(
                "OK {model}: {} objectives, first: {:?}, title: {:?}, questions: {:?}",
                b.steps.len(),
                b.steps.first().map(|x| &x.title),
                b.quest_title,
                b.clarifying_questions
            ),
            Err(e) => println!("FAILED {model}: {e}"),
        }
    }
}

#[cfg(test)]
mod live_json_mode {
    use super::*;

    #[test]
    #[ignore]
    fn live_openai_json_mode_max_tokens() {
        let s = live::probe_settings("gpt-5");
        let key = require_key(&s).unwrap();
        for with_limit in [true, false] {
            let mut body = json!({
                "model": "gpt-5",
                "messages": [{ "role": "user", "content": "Reply with a JSON object {\"ok\": true}." }],
                "response_format": { "type": "json_object" },
            });
            if with_limit { body["max_tokens"] = json!(JSON_MODE_MAX_TOKENS); }
            let r = tauri::async_runtime::block_on(request(&s, &key, &body));
            println!("json mode, max_tokens={with_limit}: {:?}", r.map(|c| c.unwrap_or_default()).map_err(|e| e.to_string()));
        }
    }
}

#[cfg(test)]
mod model_filter_tests {
    use super::is_chat_model;

    #[test]
    fn only_text_chat_models_are_offered() {
        for id in ["gpt-5", "gpt-5-mini", "gpt-6-luna", "gpt-5.6-luna", "gpt-4.1", "o3", "o4-mini", "chat-latest"] {
            assert!(is_chat_model("openai", id), "{id} should be offered");
        }
        for id in [
            "tts-1", "whisper-1", "gpt-4o-mini-tts", "gpt-4o-transcribe", "gpt-audio", "gpt-realtime-2",
            "text-embedding-3-small", "gpt-image-2", "chatgpt-image-latest", "sora-2", "omni-moderation-latest",
            "gpt-5-codex", "gpt-5-pro", "gpt-5.4-pro-2026-03-05", "o1-pro", "gpt-4o-search-preview",
            "o4-mini-deep-research", "gpt-3.5-turbo-instruct", "davinci-002", "gpt-live-1", "gpt-realtime-translate",
        ] {
            assert!(!is_chat_model("openai", id), "{id} should be hidden");
        }
        // DeepSeek's "-pro" models are ordinary chat models.
        assert!(is_chat_model("deepseek", "deepseek-v4-pro"));
        assert!(is_chat_model("deepseek", "deepseek-flash"));
    }
}
