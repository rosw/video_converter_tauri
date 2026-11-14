mod commands;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};

/// 单个转换任务的状态
#[derive(Clone, Debug)]
pub struct ConvertJob {
    pub id: String,
    pub input_path: String,
    pub output_path: String,
    pub format: String,
    /// pending / running / done / error / cancelled
    pub status: String,
    pub progress: f64,
    pub message: String,
    pub pid: Option<u32>,
}

pub struct AppState {
    /// 任务表：job_id -> job
    pub jobs: Arc<Mutex<HashMap<String, ConvertJob>>>,
    /// 当前正在运行的任务 pid（用于取消）
    pub current_pid: Arc<Mutex<Option<u32>>>,
    /// 已解析出的 ffmpeg 可执行文件路径
    pub ffmpeg_path: Arc<Mutex<Option<PathBuf>>>,
    /// 是否正在处理任务（防止重复启动批处理线程）
    pub worker_running: Arc<Mutex<bool>>,
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
    let exe = std::env::current_exe().ok()?;
    exe.parent().map(|d| d.to_string_lossy().to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            jobs: Arc::new(Mutex::new(HashMap::new())),
            current_pid: Arc::new(Mutex::new(None)),
            ffmpeg_path: Arc::new(Mutex::new(None)),
            worker_running: Arc::new(Mutex::new(false)),
        })
        .invoke_handler(tauri::generate_handler![
            get_platform,
            get_app_dir,
            commands::get_formats,
            commands::check_ffmpeg,
            commands::get_output_path,
            commands::pick_input_file,
            commands::pick_input_files,
            commands::pick_output_file,
            commands::pick_output_dir,
            commands::start_convert,
            commands::start_batch_convert,
            commands::cancel_convert,
            commands::cancel_job,
            commands::cancel_all,
            commands::remove_job,
            commands::clear_jobs,
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
