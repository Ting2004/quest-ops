mod coach;
mod data;
mod db;
mod error;
mod llm;
mod priority;
mod quests;
mod rewards;
mod secrets;
mod settings;
mod timer;
mod today;

use std::sync::Mutex;

use tauri::{
    App, Manager,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Tray icon with Show / Quit; its tooltip shows the running countdown.
fn build_tray(app: &App, language: &str) -> tauri::Result<()> {
    let (show_label, quit_label) = if language == "zh" { ("显示", "退出") } else { ("Show", "Quit") };
    let show = MenuItem::with_id(app, "show", show_label, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", quit_label, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let mut tray = TrayIconBuilder::with_id(timer::TRAY_ID)
        .tooltip("Quest Ops")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let conn = db::open(&dir.join("questops.db"))?;
            let language = settings::load(&conn).map(|s| s.language).unwrap_or_default();
            app.manage(db::Db(Mutex::new(conn)));
            app.manage(priority::ScoreQueue::default());

            build_tray(app, &language)?;
            timer::spawn_ticker(app.handle().clone());
            // Score anything deployed but never scored (new installs, past failures).
            let _ = priority::score_pending(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            settings::get_settings,
            settings::save_settings,
            settings::set_language,
            settings::set_theme,
            secrets::set_api_key,
            secrets::clear_api_key,
            secrets::has_api_key,
            llm::test_llm_connection,
            llm::breakdown_quest,
            quests::list_categories,
            quests::list_quests,
            quests::create_quest,
            quests::delete_quest,
            quests::get_steps,
            quests::deploy_quest,
            quests::get_quest_plan,
            quests::update_quest_plan,
            priority::list_matrix,
            priority::set_quadrant,
            priority::clear_quadrant_override,
            priority::score_quest,
            priority::score_pending,
            priority::rescore_all,
            timer::get_timer,
            timer::start_step,
            timer::pause_timer,
            timer::resume_timer,
            timer::extend_timer,
            timer::abort_step,
            timer::complete_step,
            timer::complete_step_untimed,
            coach::split_step,
            coach::unstick_step,
            coach::insert_step,
            today::get_today,
            today::clear_today_plan,
            today::plan_today,
            today::debrief_today,
            rewards::get_stats,
            data::export_tasks,
            data::import_tasks,
            data::clear_finished,
            data::reset_progress,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
