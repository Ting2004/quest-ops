//! API key storage in the OS credential store (Windows Credential Manager).
//! The key never leaves the Rust side; the frontend only learns whether one is set.

use keyring::Entry;

use crate::error::AppResult;

const SERVICE: &str = "com.motivation.questops";
const ACCOUNT: &str = "openai_api_key";

fn entry() -> AppResult<Entry> {
    Ok(Entry::new(SERVICE, ACCOUNT)?)
}

pub fn get_api_key() -> AppResult<Option<String>> {
    match entry()?.get_password() {
        Ok(k) => Ok(Some(k)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[tauri::command]
pub fn set_api_key(key: String) -> AppResult<()> {
    let key = key.trim();
    if key.is_empty() {
        return clear_api_key();
    }
    entry()?.set_password(key)?;
    Ok(())
}

#[tauri::command]
pub fn clear_api_key() -> AppResult<()> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[tauri::command]
pub fn has_api_key() -> AppResult<bool> {
    Ok(get_api_key()?.is_some())
}
