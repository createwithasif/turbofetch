mod bridge;
mod engine;
mod torrent;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use engine::{DownloadInfo, Manager, Settings};
use torrent::Torrents;
use tauri::{AppHandle, Emitter, Manager as _, State};

type Downloads<'a> = State<'a, Arc<Manager>>;
type TorrentState<'a> = State<'a, Arc<Torrents>>;

/// Routes a link to the torrent engine or the HTTP engine.
pub(crate) async fn add_any(
    manager: &Arc<Manager>,
    torrents: &Torrents,
    url: String,
    dir: PathBuf,
    headers: Vec<(String, String)>,
    name_hint: Option<String>,
) -> Result<String, String> {
    if torrent::is_torrent(&url) {
        torrents.add(&url, dir).await
    } else {
        manager.add(url, dir, headers, name_hint).await
    }
}

fn all_downloads(manager: &Manager, torrents: &Torrents, interval: Option<Duration>) -> Vec<DownloadInfo> {
    let mut list = match interval {
        Some(i) => manager.tick(i),
        None => manager.list(),
    };
    list.extend(torrents.list());
    list
}

/// Explicit folder, else the one from Options, else the system Downloads folder.
pub(crate) fn download_dir(
    app: &AppHandle,
    manager: &Manager,
    dir: Option<String>,
) -> Result<PathBuf, String> {
    if let Some(d) = dir.filter(|d| !d.trim().is_empty()) {
        return Ok(PathBuf::from(d.trim()));
    }
    if let Some(d) = manager.settings().download_dir {
        return Ok(d);
    }
    app.path().download_dir().map_err(|e| e.to_string())
}

#[tauri::command]
async fn add_download(
    app: AppHandle,
    downloads: Downloads<'_>,
    torrents: TorrentState<'_>,
    url: String,
    dir: Option<String>,
) -> Result<String, String> {
    let dir = download_dir(&app, &downloads, dir)?;
    add_any(&downloads, &torrents, url.trim().to_string(), dir, Vec::new(), None).await
}

#[tauri::command]
fn list_downloads(downloads: Downloads<'_>, torrents: TorrentState<'_>) -> Vec<DownloadInfo> {
    all_downloads(&downloads, &torrents, None)
}

#[tauri::command]
async fn pause_download(downloads: Downloads<'_>, torrents: TorrentState<'_>, id: String) -> Result<(), String> {
    if torrent::owns(&id) {
        return torrents.pause(&id).await;
    }
    downloads.pause(&id)
}

#[tauri::command]
async fn resume_download(downloads: Downloads<'_>, torrents: TorrentState<'_>, id: String) -> Result<(), String> {
    if torrent::owns(&id) {
        return torrents.resume(&id).await;
    }
    downloads.resume(&id)
}

#[tauri::command]
async fn remove_download(
    downloads: Downloads<'_>,
    torrents: TorrentState<'_>,
    id: String,
    delete_file: bool,
) -> Result<(), String> {
    if torrent::owns(&id) {
        return torrents.remove(&id, delete_file).await;
    }
    downloads.remove(&id, delete_file)
}

#[tauri::command]
fn get_settings(app: AppHandle, downloads: Downloads<'_>) -> Result<Settings, String> {
    let mut settings = downloads.settings();
    if settings.download_dir.is_none() {
        // Show the effective folder in Options rather than an empty box.
        settings.download_dir = Some(download_dir(&app, &downloads, None)?);
    }
    Ok(settings)
}

#[tauri::command]
fn set_settings(downloads: Downloads<'_>, settings: Settings) -> Result<(), String> {
    downloads.set_settings(settings)
}

/// Folder holding the unpacked browser extension, for "Load unpacked".
#[tauri::command]
fn extension_dir(app: AppHandle) -> Result<PathBuf, String> {
    let dir = if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../extension")
    } else {
        app.path().resource_dir().map_err(|e| e.to_string())?.join("extension")
    };
    dir.canonicalize().map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // A second launch (e.g. from the Dock) just brings back the running copy.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![HIDDEN_ARG]),
        ))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let manager = Manager::new(data_dir.clone());
            app.manage(manager.clone());
            let torrents = Torrents::new();
            app.manage(torrents.clone());

            let default_dir = download_dir(app.handle(), &manager, None)?;
            let starting = torrents.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = starting.start(data_dir, default_dir).await {
                    eprintln!("{e}");
                }
            });

            // Start with the computer, like IDM, so the browser bridge is always there.
            {
                use tauri_plugin_autostart::ManagerExt;
                let autostart = app.autolaunch();
                if !autostart.is_enabled().unwrap_or(true) {
                    let _ = autostart.enable();
                }
            }
            if std::env::args().any(|a| a == HIDDEN_ARG) {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            tray(app)?;

            tauri::async_runtime::spawn(bridge::serve(
                app.handle().clone(),
                manager.clone(),
                torrents.clone(),
            ));

            // Push progress to the UI twice a second; persist every few seconds.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let interval = Duration::from_millis(500);
                let mut ticks = 0u32;
                loop {
                    tokio::time::sleep(interval).await;
                    let _ = handle.emit("downloads", all_downloads(&manager, &torrents, Some(interval)));
                    torrents.pause_finished().await;
                    ticks += 1;
                    if ticks % 6 == 0 {
                        manager.save();
                    }
                }
            });
            Ok(())
        })
        // Closing the main window keeps ADM running in the menu bar.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            add_download,
            list_downloads,
            pause_download,
            resume_download,
            remove_download,
            get_settings,
            set_settings,
            extension_dir
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| {
            // Clicking the Dock icon brings the hidden window back.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show_main(app);
            }
            let _ = (app, event);
        });
}

const HIDDEN_ARG: &str = "--hidden";

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Menu bar icon: open the window or quit for real.
fn tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let open = MenuItem::with_id(app, "open", "Open ADM", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit ADM", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tray = TrayIconBuilder::with_id("adm")
        .tooltip("ADM")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}
