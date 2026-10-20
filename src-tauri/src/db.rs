use std::{path::Path, sync::Mutex};

use rusqlite::Connection;

use crate::error::AppResult;

/// Shared database handle stored in Tauri managed state.
pub struct Db(pub Mutex<Connection>);

/// Ordered schema migrations. Index + 1 is the resulting `user_version`.
/// Never edit a released migration; append a new one instead.
pub(crate) const MIGRATIONS: &[&str] = &[
    // v1: initial schema
    r#"
    CREATE TABLE profile (
        id                INTEGER PRIMARY KEY CHECK (id = 1),
        commander_name    TEXT    NOT NULL DEFAULT 'Commander',
        total_xp          INTEGER NOT NULL DEFAULT 0,
        current_streak    INTEGER NOT NULL DEFAULT 0,
        longest_streak    INTEGER NOT NULL DEFAULT 0,
        goals_text        TEXT    NOT NULL DEFAULT '',
        model_name        TEXT    NOT NULL DEFAULT '',
        small_model_name  TEXT    NOT NULL DEFAULT '',
        api_base_url      TEXT    NOT NULL DEFAULT 'https://api.openai.com/v1',
        max_pauses        INTEGER NOT NULL DEFAULT 2,
        calibration_json  TEXT    NOT NULL DEFAULT '{}'
    );
    INSERT INTO profile (id) VALUES (1);

    CREATE TABLE category (
        id    INTEGER PRIMARY KEY,
        name  TEXT    NOT NULL UNIQUE,
        xp    INTEGER NOT NULL DEFAULT 0
    );
    INSERT INTO category (name) VALUES ('Research'), ('Writing'), ('Admin'), ('Health');

    CREATE TABLE quest (
        id               INTEGER PRIMARY KEY,
        title            TEXT    NOT NULL,
        notes            TEXT    NOT NULL DEFAULT '',
        category_id      INTEGER REFERENCES category(id) ON DELETE SET NULL,
        status           TEXT    NOT NULL DEFAULT 'inbox'
                         CHECK (status IN ('inbox', 'active', 'completed', 'archived')),
        importance       INTEGER CHECK (importance BETWEEN 1 AND 5),
        urgency_base     INTEGER CHECK (urgency_base BETWEEN 1 AND 5),
        deadline         TEXT,
        ai_rationale     TEXT    NOT NULL DEFAULT '',
        user_overridden  INTEGER NOT NULL DEFAULT 0,
        created_at       TEXT    NOT NULL DEFAULT (datetime('now')),
        completed_at     TEXT
    );

    CREATE TABLE step (
        id              INTEGER PRIMARY KEY,
        quest_id        INTEGER NOT NULL REFERENCES quest(id) ON DELETE CASCADE,
        parent_step_id  INTEGER REFERENCES step(id) ON DELETE CASCADE,
        title           TEXT    NOT NULL,
        done_when       TEXT    NOT NULL DEFAULT '',
        effort          INTEGER NOT NULL DEFAULT 1 CHECK (effort IN (1, 2, 3, 5)),
        est_minutes     INTEGER NOT NULL DEFAULT 15,
        order_index     INTEGER NOT NULL DEFAULT 0,
        status          TEXT    NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending', 'active', 'done', 'aborted')),
        actual_seconds  INTEGER,
        pauses_used     INTEGER NOT NULL DEFAULT 0,
        completed_at    TEXT
    );
    CREATE INDEX idx_step_quest ON step(quest_id, order_index);

    CREATE TABLE timer_session (
        id               INTEGER PRIMARY KEY,
        step_id          INTEGER NOT NULL REFERENCES step(id) ON DELETE CASCADE,
        started_at       TEXT    NOT NULL,
        planned_seconds  INTEGER NOT NULL,
        paused_ms        INTEGER NOT NULL DEFAULT 0,
        ended_at         TEXT,
        outcome          TEXT CHECK (outcome IN ('on_time', 'overtime', 'aborted'))
    );

    CREATE TABLE priority_override (
        id             INTEGER PRIMARY KEY,
        quest_id       INTEGER NOT NULL REFERENCES quest(id) ON DELETE CASCADE,
        ai_quadrant    INTEGER NOT NULL CHECK (ai_quadrant BETWEEN 1 AND 4),
        user_quadrant  INTEGER NOT NULL CHECK (user_quadrant BETWEEN 1 AND 4),
        created_at     TEXT    NOT NULL DEFAULT (datetime('now'))
    );

    CREATE TABLE xp_event (
        id          INTEGER PRIMARY KEY,
        quest_id    INTEGER REFERENCES quest(id) ON DELETE SET NULL,
        step_id     INTEGER REFERENCES step(id) ON DELETE SET NULL,
        amount      INTEGER NOT NULL,
        reason      TEXT    NOT NULL,
        created_at  TEXT    NOT NULL DEFAULT (datetime('now'))
    );
    "#,
    // v2: manual quadrant placement and scoring timestamp
    r#"
    ALTER TABLE quest ADD COLUMN quadrant_override INTEGER
        CHECK (quadrant_override BETWEEN 1 AND 4);
    ALTER TABLE quest ADD COLUMN scored_at TEXT;
    "#,
    // v3: user-defined priority rules and interface language
    r#"
    ALTER TABLE profile ADD COLUMN urgent_within_days INTEGER NOT NULL DEFAULT 3;
    ALTER TABLE profile ADD COLUMN importance_threshold INTEGER NOT NULL DEFAULT 4;
    ALTER TABLE profile ADD COLUMN language TEXT NOT NULL DEFAULT 'en'
        CHECK (language IN ('en', 'zh'));

    -- timers: epoch milliseconds keep pause/elapsed arithmetic exact
    ALTER TABLE timer_session ADD COLUMN started_ms INTEGER;
    ALTER TABLE timer_session ADD COLUMN paused_since_ms INTEGER;
    ALTER TABLE timer_session ADD COLUMN ended_ms INTEGER;
    ALTER TABLE timer_session ADD COLUMN extended_seconds INTEGER NOT NULL DEFAULT 0;
    CREATE INDEX idx_timer_open ON timer_session(ended_at);
    CREATE INDEX idx_xp_created ON xp_event(created_at);

    CREATE TABLE achievement (
        id           TEXT PRIMARY KEY,
        unlocked_at  TEXT NOT NULL DEFAULT (datetime('now'))
    );

    CREATE TABLE daily_plan (
        plan_date    TEXT    NOT NULL,
        step_id      INTEGER NOT NULL REFERENCES step(id) ON DELETE CASCADE,
        order_index  INTEGER NOT NULL,
        note         TEXT    NOT NULL DEFAULT '',
        PRIMARY KEY (plan_date, step_id)
    );

    CREATE TABLE daily_debrief (
        plan_date   TEXT PRIMARY KEY,
        text        TEXT NOT NULL,
        created_at  TEXT NOT NULL DEFAULT (datetime('now'))
    );
    "#,
    // v4: one-off milestones become tiers of achievement families
    r#"
    UPDATE achievement SET id = CASE id
        WHEN 'first_objective' THEN 'objectives:1'
        WHEN 'objectives_10'   THEN 'objectives:10'
        WHEN 'objectives_50'   THEN 'objectives:50'
        WHEN 'objectives_100'  THEN 'objectives:100'
        WHEN 'first_operation' THEN 'operations:1'
        WHEN 'operations_10'   THEN 'operations:10'
        WHEN 'strategic_5'     THEN 'strategic:5'
        WHEN 'on_time_10'      THEN 'on_time:10'
        WHEN 'streak_3'        THEN 'streak:3'
        WHEN 'streak_7'        THEN 'streak:7'
        WHEN 'streak_30'       THEN 'streak:30'
        WHEN 'level_5'         THEN 'level:5'
        WHEN 'level_10'        THEN 'level:10'
        ELSE id END;
    "#,
    // v5: focus time banked when finished timer runs are cleared
    r#"
    ALTER TABLE profile ADD COLUMN archived_focus_ms INTEGER NOT NULL DEFAULT 0;
    "#,
    // v6: colour theme
    r#"
    ALTER TABLE profile ADD COLUMN theme TEXT NOT NULL DEFAULT 'dark'
        CHECK (theme IN ('dark', 'light', 'system'));
    "#,
    // v7: LLM provider (OpenAI-compatible APIs such as DeepSeek)
    r#"
    ALTER TABLE profile ADD COLUMN provider TEXT NOT NULL DEFAULT 'openai'
        CHECK (provider IN ('openai', 'deepseek', 'custom'));
    "#,
];

pub fn open(path: &Path) -> AppResult<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    migrate(&mut conn)?;
    Ok(conn)
}

fn migrate(conn: &mut Connection) -> AppResult<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i as i64 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_and_are_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v, MIGRATIONS.len() as i64);
        let cats: i64 = conn.query_row("SELECT COUNT(*) FROM category", [], |r| r.get(0)).unwrap();
        assert_eq!(cats, 4);
    }
}

#[cfg(test)]
pub fn migrate_for_test(conn: &mut Connection) {
    migrate(conn).unwrap();
}
