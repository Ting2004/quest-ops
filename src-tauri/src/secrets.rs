//! API key storage in the OS credential store (Windows Credential Manager),
//! one key per provider. The key never leaves the Rust side; the frontend
//! only learns whether one is set.

use keyring::Entry;

use crate::error::{AppError, AppResult};

const SERVICE: &str = "com.motivation.questops";

pub const PROVIDERS: &[&str] = &["openai", "deepseek", "custom"];

fn entry(provider: &str) -> AppResult<Entry> {
    if !PROVIDERS.contains(&provider) {
        return Err(AppError::Msg(format!("unknown provider \"{provider}\"")));
    }
    // "openai_api_key" predates multi-provider support; keep it so saved keys still work.
    Ok(Entry::new(SERVICE, &format!("{provider}_api_key"))?)
}

pub fn get_api_key(provider: &str) -> AppResult<Option<String>> {
    match entry(provider)?.get_password() {
        Ok(k) => Ok(Some(k)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[tauri::command]
pub fn set_api_key(provider: String, key: String) -> AppResult<()> {
    let key = key.trim();
    if key.is_empty() {
        return clear_api_key(provider);
    }
    entry(&provider)?.set_password(key)?;
    Ok(())
}

#[tauri::command]
pub fn clear_api_key(provider: String) -> AppResult<()> {
    match entry(&provider)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[tauri::command]
pub fn has_api_key(provider: String) -> AppResult<bool> {
    Ok(get_api_key(&provider)?.is_some())
}
