mod commands;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};

pub struct AppState {
    /// 当前 ffmpeg 子进程的 PID，None 表示没有在跑
    pub converting_pid: Arc<Mutex<Option<u32>>>,
    /// 已解析出的 ffmpeg 可执行文件路径（启动与重检时更新）
    pub ffmpeg_path: Arc<Mutex<Option<PathBuf>>>,
}

/// 返回当前操作系统："windows" / "macos" / "linux"
#[tauri::command]
fn get_platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// 返回程序所在目录（供"把 ffmpeg 放到程序同目录"提示用）
#[tauri::command]
fn get_app_dir(_app: AppHandle) -> Option<String> {
    // macOS 打包后 exe 在 <App>.app/Contents/MacOS/；
    // Windows 打包后在安装目录；Linux 在 AppImage 挂载目录。
    let exe = std::env::current_exe().ok()?;
    exe.parent().map(|d| d.to_string_lossy().to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            converting_pid: Arc::new(Mutex::new(None)),
            ffmpeg_path: Arc::new(Mutex::new(None)),
        })
        .invoke_handler(tauri::generate_handler![
            get_platform,
            get_app_dir,
            commands::get_formats,
            commands::check_ffmpeg,
            commands::get_output_path,
            commands::start_convert,
            commands::cancel_convert,
            commands::pick_input_file,
            commands::pick_output_file,
            commands::open_url,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let located = commands::locate_ffmpeg(&handle);
            {
                let state = app.state::<AppState>();
                *state.ffmpeg_path.lock().unwrap() = located.map(|(p, _)| p);
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
