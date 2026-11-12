//! Where the database lives. By default it sits in the app data folder; the
//! user can point it at any folder (e.g. a OneDrive/Dropbox/Syncthing folder)
//! to sync tasks across devices. The choice is kept in `location.json` in the
//! app data folder, because the database itself moves.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::{
    db::{self, Db},
    error::{AppError, AppResult},
};

pub const DB_FILE: &str = "questops.db";
const LOCATION_FILE: &str = "location.json";

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocationFile {
    #[serde(default)]
    data_dir: Option<String>,
}

/// The folder in use, decided once at startup.
pub struct Storage {
    /// Folder that holds the database right now.
    pub dir: PathBuf,
    /// True when `dir` is a folder the user chose.
    pub custom: bool,
    /// A chosen folder that could not be reached at startup (local data used instead).
    pub unreachable: Option<String>,
}

impl Storage {
    pub fn db_path(&self) -> PathBuf {
        self.dir.join(DB_FILE)
    }
}

fn read_location(app_dir: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(app_dir.join(LOCATION_FILE)).ok()?;
    let loc: LocationFile = serde_json::from_str(&text).ok()?;
    loc.data_dir.filter(|d| !d.trim().is_empty()).map(PathBuf::from)
}

fn write_location(app_dir: &Path, dir: Option<&Path>) -> AppResult<()> {
    let path = app_dir.join(LOCATION_FILE);
    match dir {
        None => {
            if path.exists() {
                fs::remove_file(&path).map_err(io_err)?;
            }
        }
        Some(d) => {
            let loc = LocationFile { data_dir: Some(d.to_string_lossy().into_owned()) };
            let json = serde_json::to_string_pretty(&loc).map_err(|e| AppError::Msg(e.to_string()))?;
            fs::write(&path, json).map_err(io_err)?;
        }
    }
    Ok(())
}

fn io_err(e: std::io::Error) -> AppError {
    AppError::Msg(e.to_string())
}

/// Picks the database folder at startup. A chosen folder that is missing
/// (sync drive not mounted, folder deleted) falls back to the local folder
/// and is reported, rather than silently creating an empty database there.
pub fn resolve(app_dir: &Path) -> Storage {
    match read_location(app_dir) {
        Some(dir) if dir.is_dir() => Storage { dir, custom: true, unreachable: None },
        Some(dir) => Storage {
            dir: app_dir.to_path_buf(),
            custom: false,
            unreachable: Some(dir.to_string_lossy().into_owned()),
        },
        None => Storage { dir: app_dir.to_path_buf(), custom: false, unreachable: None },
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageInfo {
    pub dir: String,
    pub default_dir: String,
    pub custom: bool,
    pub unreachable: Option<String>,
}

#[tauri::command]
pub fn get_storage(app: AppHandle, storage: State<'_, Storage>) -> AppResult<StorageInfo> {
    let default_dir = app.path().app_data_dir().map_err(|e| AppError::Msg(e.to_string()))?;
    Ok(StorageInfo {
        dir: storage.dir.to_string_lossy().into_owned(),
        default_dir: default_dir.to_string_lossy().into_owned(),
        custom: storage.custom,
        unreachable: storage.unreachable.clone(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirCheck {
    /// The folder is the one already in use.
    pub is_current: bool,
    /// The folder already holds a Quest Ops database.
    pub has_data: bool,
}

#[tauri::command]
pub fn inspect_storage_dir(dir: String, storage: State<'_, Storage>) -> AppResult<DirCheck> {
    let dir = PathBuf::from(dir);
    if !dir.is_dir() {
        return Err(AppError::Msg(format!("Folder not found: {}", dir.display())));
    }
    Ok(DirCheck { is_current: same_dir(&dir, &storage.dir), has_data: dir.join(DB_FILE).is_file() })
}

/// What to do with the data when switching folders.
#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum SwitchMode {
    /// Copy the current data into the new folder (any database there is kept as a backup file).
    Move,
    /// Use the database already in the new folder (e.g. synced from another device).
    UseExisting,
}

fn stamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Checks that a file is a Quest Ops database before switching to it.
fn validate_existing(path: &Path) -> AppResult<()> {
    if !path.is_file() {
        return Err(AppError::Msg("That folder has no Quest Ops data.".into()));
    }
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let ok: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name IN ('profile', 'quest')",
            [],
            |r| r.get(0),
        )
        .map_err(|_| AppError::Msg("The file in that folder is not a Quest Ops database.".into()))?;
    if ok != 2 {
        return Err(AppError::Msg("The file in that folder is not a Quest Ops database.".into()));
    }
    Ok(())
}

/// Switches the data folder, then restarts the app so everything reopens
/// from the new location. `dir = None` means back to the local app folder.
#[tauri::command]
pub fn set_storage_dir(
    app: AppHandle,
    db: State<'_, Db>,
    storage: State<'_, Storage>,
    dir: Option<String>,
    mode: SwitchMode,
) -> AppResult<()> {
    let app_dir = app.path().app_data_dir().map_err(|e| AppError::Msg(e.to_string()))?;
    let target_dir = dir.map(PathBuf::from).unwrap_or_else(|| app_dir.clone());
    if !target_dir.is_dir() {
        return Err(AppError::Msg(format!("Folder not found: {}", target_dir.display())));
    }
    if same_dir(&target_dir, &storage.dir) {
        return Err(AppError::Msg("That folder is already in use.".into()));
    }
    let target = target_dir.join(DB_FILE);

    match mode {
        SwitchMode::UseExisting => validate_existing(&target)?,
        SwitchMode::Move => {
            if target.exists() {
                let backup = target_dir.join(format!("questops-replaced-{}.db", stamp()));
                fs::rename(&target, &backup).map_err(io_err)?;
            }
            // VACUUM INTO writes one self-contained file (no -wal/-shm side files),
            // which is what a sync service should see.
            let conn = db.0.lock().map_err(|_| AppError::Msg("database lock poisoned".into()))?;
            conn.execute("VACUUM INTO ?1", [target.to_string_lossy().as_ref()])?;
        }
    }

    let is_default = same_dir(&target_dir, &app_dir);
    write_location(&app_dir, if is_default { None } else { Some(&target_dir) })?;
    app.restart();
}

/// Opens the database for this storage location. Synced folders use the
/// rollback journal instead of WAL so the database is always one file.
pub fn open_db(storage: &Storage) -> AppResult<Connection> {
    db::open(&storage.db_path(), !storage.custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("questops-storage-{name}-{}", stamp()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn location_round_trip_and_fallback() {
        let app = temp_dir("app");
        let sync = temp_dir("sync");

        let s = resolve(&app);
        assert!(!s.custom && s.unreachable.is_none() && s.dir == app);

        write_location(&app, Some(&sync)).unwrap();
        let s = resolve(&app);
        assert!(s.custom && s.dir == sync);

        fs::remove_dir_all(&sync).unwrap();
        let s = resolve(&app);
        assert!(!s.custom && s.dir == app);
        assert_eq!(s.unreachable.as_deref(), Some(sync.to_string_lossy().as_ref()));

        write_location(&app, None).unwrap();
        assert!(resolve(&app).unreachable.is_none());
        fs::remove_dir_all(&app).unwrap();
    }

    #[test]
    fn copied_database_is_single_file_and_valid() {
        let a = temp_dir("a");
        let b = temp_dir("b");
        let conn = db::open(&a.join(DB_FILE), true).unwrap();
        conn.execute("INSERT INTO quest (title) VALUES ('x')", []).unwrap();
        conn.execute("VACUUM INTO ?1", [b.join(DB_FILE).to_string_lossy().as_ref()]).unwrap();
        drop(conn);

        validate_existing(&b.join(DB_FILE)).unwrap();
        let copy = db::open(&b.join(DB_FILE), false).unwrap();
        let mode: String = copy.pragma_query_value(None, "journal_mode", |r| r.get(0)).unwrap();
        assert_eq!(mode.to_lowercase(), "delete");
        let n: i64 = copy.query_row("SELECT count(*) FROM quest", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        drop(copy);
        assert!(!b.join(format!("{DB_FILE}-wal")).exists());

        let junk = b.join("junk.db");
        Connection::open(&junk).unwrap().execute_batch("CREATE TABLE t (x)").unwrap();
        assert!(validate_existing(&junk).is_err());
        let _ = fs::remove_dir_all(&a);
        let _ = fs::remove_dir_all(&b);
    }
}
