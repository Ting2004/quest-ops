//! Update checks against every mirror (Gitee and GitHub) at once, installing
//! whichever offers the newest version. The plugin's own check stops at the
//! first mirror that answers, so a mirror that is merely behind (say, still
//! uploading a release) would hide a newer version on the other one.

use std::{sync::Mutex, time::Duration};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Url};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::error::{AppError, AppResult};

/// Per-mirror timeout, so an unreachable mirror (e.g. GitHub from mainland
/// China) costs at most this long.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// The update found by the last check, kept for `install_update`.
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<Update>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    pub notes: Option<String>,
    /// Host of the mirror the update will be downloaded from.
    pub source: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    received: u64,
    total: Option<u64>,
}

/// Mirrors from `plugins.updater.endpoints` in tauri.conf.json, in preference order.
fn endpoints(app: &AppHandle) -> Vec<Url> {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|u| u["endpoints"].as_array())
        .map(|list| list.iter().filter_map(|e| e.as_str()?.parse().ok()).collect())
        .unwrap_or_default()
}

/// Picks the newest version; on a tie the earlier (preferred) mirror wins.
pub fn pick_newest<T>(found: Vec<(String, T)>) -> Option<T> {
    let mut best: Option<(semver::Version, T)> = None;
    for (version, item) in found {
        let Ok(v) = semver::Version::parse(version.trim_start_matches('v')) else { continue };
        if best.as_ref().is_none_or(|(b, _)| v > *b) {
            best = Some((v, item));
        }
    }
    best.map(|(_, item)| item)
}

async fn check_one(app: AppHandle, url: Url) -> Result<Option<Update>, String> {
    let updater = app
        .updater_builder()
        .endpoints(vec![url])
        .map_err(|e| e.to_string())?
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    updater.check().await.map_err(|e| e.to_string())
}

/// Checks every mirror in parallel and returns the newest update, if any.
/// Fails only when no mirror could be reached at all.
#[tauri::command]
pub async fn check_update(app: AppHandle) -> AppResult<Option<UpdateInfo>> {
    let urls = endpoints(&app);
    if urls.is_empty() {
        return Err(AppError::Msg("no update mirrors configured".into()));
    }
    let tasks: Vec<_> = urls
        .iter()
        .map(|u| tauri::async_runtime::spawn(check_one(app.clone(), u.clone())))
        .collect();

    let mut found = Vec::new();
    let mut errors = Vec::new();
    let mut reached = 0;
    for (url, task) in urls.iter().zip(tasks) {
        let host = url.host_str().unwrap_or("?").to_string();
        match task.await {
            Ok(Ok(update)) => {
                reached += 1;
                if let Some(u) = update {
                    found.push((u.version.clone(), (host, u)));
                }
            }
            Ok(Err(e)) => errors.push(format!("{host}: {e}")),
            Err(e) => errors.push(format!("{host}: {e}")),
        }
    }
    if reached == 0 {
        return Err(AppError::Msg(format!("no update mirror reachable ({})", errors.join("; "))));
    }

    let newest = pick_newest(found);
    let info = newest.as_ref().map(|(host, u)| UpdateInfo {
        version: u.version.clone(),
        current_version: u.current_version.clone(),
        notes: u.body.clone(),
        source: host.clone(),
    });
    *app.state::<PendingUpdate>().0.lock().unwrap_or_else(|e| e.into_inner()) = newest.map(|(_, u)| u);
    Ok(info)
}

/// Downloads and installs the update found by the last check, reporting
/// progress as `update-progress` events, then restarts the app.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> AppResult<()> {
    let update = app
        .state::<PendingUpdate>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .ok_or_else(|| AppError::Msg("no update to install; check again".into()))?;

    let mut received: u64 = 0;
    let progress_app = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                received += chunk as u64;
                let _ = progress_app.emit("update-progress", Progress { received, total });
            },
            || {},
        )
        .await
        .map_err(|e| AppError::Msg(format!("update failed: {e}")))?;
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(found: &[(&str, &'static str)]) -> Option<&'static str> {
        pick_newest(found.iter().map(|(v, m)| (v.to_string(), *m)).collect())
    }

    #[test]
    fn newest_version_wins_and_ties_keep_the_preferred_mirror() {
        // Gitee still on 0.1.4 while GitHub already has 0.1.5.
        assert_eq!(pick(&[("0.1.4", "gitee"), ("0.1.5", "github")]), Some("github"));
        assert_eq!(pick(&[("0.1.5", "gitee"), ("0.1.5", "github")]), Some("gitee"));
        assert_eq!(pick(&[("0.1.10", "gitee"), ("0.1.9", "github")]), Some("gitee"));
        assert_eq!(pick(&[("v0.2.0", "gitee")]), Some("gitee"));
        assert_eq!(pick(&[("not-a-version", "gitee")]), None);
        assert_eq!(pick(&[]), None);
    }
}
