use crate::AppState;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::io::{BufRead, BufReader, Read};
use std::thread;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

/// 支持的输出格式列表（前端下拉框用）
#[derive(Serialize, Deserialize, Clone)]
pub struct FormatItem {
    pub ext: String,
    pub label: String,
    pub kind: String, // "video" | "audio"
}

#[tauri::command]
pub fn get_formats() -> Vec<FormatItem> {
    vec![
        FormatItem { ext: "mp4".into(), label: "MP4 (H.264 + AAC)".into(), kind: "video".into() },
        FormatItem { ext: "mkv".into(), label: "MKV (H.264 + AAC)".into(), kind: "video".into() },
        FormatItem { ext: "mov".into(), label: "MOV (H.264 + AAC)".into(), kind: "video".into() },
        FormatItem { ext: "webm".into(), label: "WebM (VP9 + Opus)".into(), kind: "video".into() },
        FormatItem { ext: "avi".into(), label: "AVI (MPEG-4)".into(), kind: "video".into() },
        FormatItem { ext: "flv".into(), label: "FLV (H.264 + MP3)".into(), kind: "video".into() },
        FormatItem { ext: "wmv".into(), label: "WMV".into(), kind: "video".into() },
        FormatItem { ext: "m4v".into(), label: "M4V (H.264 + AAC)".into(), kind: "video".into() },
        FormatItem { ext: "mpg".into(), label: "MPG (MPEG-2)".into(), kind: "video".into() },
        FormatItem { ext: "ts".into(), label: "TS (H.264)".into(), kind: "video".into() },
        FormatItem { ext: "3gp".into(), label: "3GP (H.264 + AAC)".into(), kind: "video".into() },
        FormatItem { ext: "gif".into(), label: "GIF (动图)".into(), kind: "video".into() },
        FormatItem { ext: "mp3".into(), label: "MP3 (320k)".into(), kind: "audio".into() },
        FormatItem { ext: "wav".into(), label: "WAV (无损)".into(), kind: "audio".into() },
        FormatItem { ext: "aac".into(), label: "AAC (256k)".into(), kind: "audio".into() },
        FormatItem { ext: "flac".into(), label: "FLAC (无损)".into(), kind: "audio".into() },
        FormatItem { ext: "ogg".into(), label: "OGG (Vorbis)".into(), kind: "audio".into() },
    ]
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FfmpegCheckResult {
    pub available: bool,
    /// 找到的 ffmpeg 可执行文件完整路径（available=true 时为 Some）
    pub path: Option<String>,
    /// "bundled"=随包分发 / "sidecar"=打包嵌入 / "system"=系统 PATH 安装
    pub source: Option<String>,
    pub version: Option<String>,
    pub message: String,
}

/// 按 Tauri sidecar 规范生成当前目标平台对应的 ffmpeg 二进制文件名。
/// 格式：ffmpeg-{target-triple}[.exe]
/// triple 由 build.rs 通过 cargo:rustc-env 注入，与 Tauri 打包时的命名完全一致，
/// 避免手写 cfg 分支在 ARM / 其他三元组上对不上。
fn sidecar_filename() -> String {
    let triple = env!("BUILD_TARGET_TRIPLE");
    if cfg!(target_os = "windows") {
        format!("ffmpeg-{triple}.exe")
    } else {
        format!("ffmpeg-{triple}")
    }
}

/// build.rs 生成的占位文件头部标记（须与 build.rs 中的同名常量保持一致）
const PLACEHOLDER_MAGIC: &str = "TAURI_FFMPEG_PLACEHOLDER";

/// 判断给定文件是否为 build.rs 生成的占位文件。
/// 占位文件只是为了让构建阶段放行，不能当真正的 ffmpeg 使用，必须跳过。
fn is_placeholder_file(path: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = vec![0u8; PLACEHOLDER_MAGIC.len()];
    f.read_exact(&mut buf).is_ok() && buf == PLACEHOLDER_MAGIC.as_bytes()
}

/// 验证给定路径是一个可运行的 ffmpeg
fn is_runnable_ffmpeg(path: &Path) -> bool {
    if !path.exists() || !path.is_file() {
        return false;
    }
    // 编译期占位文件不是真 ffmpeg，直接排除，
    // 让查找继续回退到「程序同目录」和「系统 PATH」。
    if is_placeholder_file(path) {
        return false;
    }
    Command::new(path)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 按优先级查找 ffmpeg：
///   ① Tauri 打包后随包 sidecar（resource_dir/binaries/ffmpeg-<triple>[.exe]）
///   ② 开发模式下 src-tauri/binaries/ 目录
///   ③ 程序所在目录（用户手动放旁边）
///   ④ 系统 PATH
///
/// 查找优先级严格：随包 > 同目录 > 系统，确保用户不会误用别的 ffmpeg 版本。
pub fn locate_ffmpeg(app: &AppHandle) -> Option<(PathBuf, &'static str)> {
    // 1) 打包后：Tauri 把 externalBin 复制到 <resource_dir>/binaries/<sidecar_filename>
    if let Ok(res_dir) = app.path().resource_dir() {
        let candidate = res_dir.join("binaries").join(sidecar_filename());
        if is_runnable_ffmpeg(&candidate) {
            return Some((candidate, "bundled"));
        }
        // 兼容性：有些平台 Tauri 直接放在 resource_dir 根目录
        let candidate2 = res_dir.join(sidecar_filename());
        if is_runnable_ffmpeg(&candidate2) {
            return Some((candidate2, "bundled"));
        }
    }

    // 2) 开发模式：从 src-tauri/binaries/ 查找
    //    tauri dev 运行时 cwd 通常是项目根目录；CARGO_MANIFEST_DIR 指向 src-tauri
    if let Ok(exe) = std::env::current_exe() {
        // 从 target/debug/ 或 target/release/ 向上找到项目根，再拼接 src-tauri/binaries
        let mut dir = exe.parent();
        for _ in 0..5 {
            if let Some(d) = dir {
                let dev_bin = d.join("src-tauri").join("binaries").join(sidecar_filename());
                if is_runnable_ffmpeg(&dev_bin) {
                    return Some((dev_bin, "bundled"));
                }
                // 也试试直接 binaries/（cwd 在项目根时）
                let dev_bin2 = d.join("binaries").join(sidecar_filename());
                if is_runnable_ffmpeg(&dev_bin2) {
                    return Some((dev_bin2, "bundled"));
                }
                dir = d.parent();
            } else {
                break;
            }
        }
    }

    // 3) 程序同目录（用户手动放的 ffmpeg.exe / ffmpeg）
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = if cfg!(target_os = "windows") {
                dir.join("ffmpeg.exe")
            } else {
                dir.join("ffmpeg")
            };
            if is_runnable_ffmpeg(&candidate) {
                return Some((candidate, "bundled"));
            }
        }
    }

    // 4) 系统 PATH
    let (which_cmd, which_arg) = if cfg!(target_os = "windows") {
        ("where", "ffmpeg")
    } else {
        ("which", "ffmpeg")
    };
    if let Ok(output) = Command::new(which_cmd).arg(which_arg).output() {
        if output.status.success() {
            if let Some(line) = String::from_utf8_lossy(&output.stdout).lines().next() {
                let p = PathBuf::from(line.trim());
                if is_runnable_ffmpeg(&p) {
                    return Some((p, "system"));
                }
            }
        }
    }
    None
}

/// 前端可调用的检测命令：重新定位 ffmpeg 并返回详细信息
#[tauri::command]
pub fn check_ffmpeg(app: AppHandle, state: tauri::State<'_, AppState>) -> FfmpegCheckResult {
    let located = locate_ffmpeg(&app);

    {
        let mut cached = state.ffmpeg_path.lock().unwrap();
        *cached = located.as_ref().map(|(p, _)| p.clone());
    }

    let Some((path, source)) = located else {
        return FfmpegCheckResult {
            available: false,
            path: None,
            source: None,
            version: None,
            message: "未检测到 FFmpeg".into(),
        };
    };

    let ver_output = Command::new(&path).arg("-version").output().ok();
    let version = ver_output.and_then(|o| {
        String::from_utf8_lossy(&o.stdout).lines().next().map(|l| l.to_string())
    });

    let display = path.to_string_lossy().to_string();
    FfmpegCheckResult {
        available: true,
        path: Some(display.clone()),
        source: Some(source.into()),
        version: version.clone(),
        message: match (source, version.as_ref()) {
            ("bundled", Some(v)) => format!("已使用随包 FFmpeg\n{v}"),
            ("system", Some(v)) => format!("已检测到系统 FFmpeg：{display}\n{v}"),
            (_, None) => format!("已找到 FFmpeg：{display}"),
            _ => "FFmpeg 可用".into(),
        },
    }
}

/// 根据输入路径和目标格式，生成默认输出路径（放到同目录，后缀替换）
#[tauri::command]
pub fn get_output_path(input_path: String, format: String) -> String {
    let p = Path::new(&input_path);
    let parent = p.parent().unwrap_or_else(|| Path::new("."));
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let out = parent.join(format!("{stem}.{format}"));
    out.to_string_lossy().to_string()
}

#[derive(Serialize, Clone)]
#[serde(tag = "type", content = "data")]
pub enum ConvertEvent {
    Start { pid: u32, output: String, duration_sec: Option<f64> },
    Progress { percent: f64, elapsed_sec: f64, fps: f32, speed: String },
    Stderr { line: String },
    Done { success: bool, message: String },
    Cancelled { message: String },
}

fn parse_duration(line: &str) -> Option<f64> {
    let re = Regex::new(r"Duration:\s*(\d+):(\d+):(\d+\.?\d*)").ok()?;
    let caps = re.captures(line)?;
    let h: f64 = caps.get(1)?.as_str().parse().ok()?;
    let m: f64 = caps.get(2)?.as_str().parse().ok()?;
    let s: f64 = caps.get(3)?.as_str().parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

fn parse_elapsed(line: &str) -> Option<f64> {
    let re = Regex::new(r"time=(\d+):(\d+):(\d+\.?\d*)").ok()?;
    let caps = re.captures(line)?;
    let h: f64 = caps.get(1)?.as_str().parse().ok()?;
    let m: f64 = caps.get(2)?.as_str().parse().ok()?;
    let s: f64 = caps.get(3)?.as_str().parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

fn build_ffmpeg_args(input: &str, output: &str, format: &str) -> Vec<String> {
    let mut args = vec![
        "-y".to_string(),
        "-i".to_string(), input.to_string(),
        "-hide_banner".to_string(),
        "-progress".to_string(), "pipe:1".to_string(),
        "-loglevel".to_string(), "error".to_string(),
        "-stats_period".to_string(), "0.2".to_string(),
    ];
    match format {
        "mp4" | "m4v" | "mov" => {
            args.extend(["-c:v", "libx264", "-preset", "medium", "-crf", "23",
                         "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"].map(String::from));
        }
        "mkv" => {
            args.extend(["-c:v", "libx264", "-preset", "medium", "-crf", "23",
                         "-c:a", "aac", "-b:a", "192k"].map(String::from));
        }
        "webm" => {
            args.extend(["-c:v", "libvpx-vp9", "-crf", "30", "-b:v", "0",
                         "-c:a", "libopus", "-b:a", "128k"].map(String::from));
        }
        "avi" => {
            args.extend(["-c:v", "mpeg4", "-q:v", "5", "-c:a", "libmp3lame", "-b:a", "192k"].map(String::from));
        }
        "flv" => {
            args.extend(["-c:v", "libx264", "-preset", "medium", "-crf", "23",
                         "-c:a", "libmp3lame", "-b:a", "192k"].map(String::from));
        }
        "wmv" => {
            args.extend(["-c:v", "wmv2", "-b:v", "2M",
                         "-c:a", "wmav2", "-b:a", "192k"].map(String::from));
        }
        "mpg" => {
            args.extend(["-c:v", "mpeg2video", "-b:v", "4M",
                         "-c:a", "mp2", "-b:a", "192k"].map(String::from));
        }
        "ts" => {
            args.extend(["-c:v", "libx264", "-preset", "fast", "-crf", "23",
                         "-c:a", "aac", "-b:a", "192k"].map(String::from));
        }
        "3gp" => {
            args.extend(["-c:v", "libx264", "-preset", "fast", "-crf", "26",
                         "-c:a", "aac", "-b:a", "128k", "-s", "352x288"].map(String::from));
        }
        "gif" => {
            args.extend(["-vf", "fps=10,scale=480:-1:flags=lanczos",
                         "-loop", "0"].map(String::from));
        }
        "mp3" => {
            args.extend(["-vn", "-c:a", "libmp3lame", "-b:a", "320k"].map(String::from));
        }
        "wav" => {
            args.extend(["-vn", "-c:a", "pcm_s16le"].map(String::from));
        }
        "aac" => {
            args.extend(["-vn", "-c:a", "aac", "-b:a", "256k"].map(String::from));
        }
        "flac" => {
            args.extend(["-vn", "-c:a", "flac"].map(String::from));
        }
        "ogg" => {
            args.extend(["-vn", "-c:a", "libvorbis", "-q:a", "5"].map(String::from));
        }
        _ => {}
    }
    args.push(output.to_string());
    args
}

#[tauri::command]
pub fn start_convert(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    input_path: String,
    output_path: String,
    format: String,
) -> Result<(), String> {
    {
        let mut pid_guard = state.converting_pid.lock().unwrap();
        if pid_guard.is_some() {
            return Err("已有转换任务正在进行".into());
        }
    }

    if !Path::new(&input_path).exists() {
        return Err(format!("输入文件不存在：{input_path}"));
    }

    // 优先使用缓存的 ffmpeg 路径；没有就现场再找一次
    let ffmpeg_exe: PathBuf = {
        let cached = state.ffmpeg_path.lock().unwrap().clone();
        cached
            .or_else(|| locate_ffmpeg(&app).map(|(p, _)| p))
            .ok_or_else(|| {
                "未找到可用的 FFmpeg。请按界面上的引导卡片任选一种方式准备好 FFmpeg，完成后点「重新检测」再试。".to_string()
            })?
    };

    let args = build_ffmpeg_args(&input_path, &output_path, &format);

    let mut child: Child = Command::new(&ffmpeg_exe)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 FFmpeg 失败：{e}\n路径：{}", ffmpeg_exe.display()))?;

    let pid = child.id();
    {
        let mut pid_guard = state.converting_pid.lock().unwrap();
        *pid_guard = Some(pid);
    }

    let _ = app.emit("convert-event", ConvertEvent::Start {
        pid,
        output: output_path.clone(),
        duration_sec: None,
    });

    // stderr 线程：拿总时长 + 错误信息
    let stderr = child.stderr.take().ok_or("无法获取 stderr")?;
    let app_stderr = app.clone();
    let pid_arc_stderr = Arc::clone(&state.converting_pid);
    let out_path_stderr = output_path.clone();
    let duration: Arc<Mutex<Option<f64>>> = Arc::new(Mutex::new(None));
    let duration_clone = Arc::clone(&duration);

    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        let mut found_duration = false;
        for line in reader.lines().flatten() {
            if !found_duration {
                if let Some(d) = parse_duration(&line) {
                    *duration_clone.lock().unwrap() = Some(d);
                    found_duration = true;
                    let _ = app_stderr.emit("convert-event", ConvertEvent::Start {
                        pid,
                        output: out_path_stderr.clone(),
                        duration_sec: Some(d),
                    });
                }
            }
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let _ = app_stderr.emit("convert-event", ConvertEvent::Stderr { line: trimmed.to_string() });
            }
        }
        let _ = pid_arc_stderr.lock().unwrap().take();
    });

    // stdout 线程：进度解析
    let stdout = child.stdout.take().ok_or("无法获取 stdout")?;
    let app_stdout = app.clone();
    let pid_arc_stdout = Arc::clone(&state.converting_pid);
    let duration_stdout = Arc::clone(&duration);
    let out_path_done = output_path.clone();

    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        let mut elapsed: f64 = 0.0;
        let mut fps: f32 = 0.0;
        let mut speed = String::new();
        let mut end_signaled = false;

        for line in reader.lines().flatten() {
            let line = line.trim();
            if line.is_empty() { continue; }
            if let Some(rest) = line.strip_prefix("out_time_ms=") {
                if let Ok(us) = rest.parse::<f64>() {
                    elapsed = us / 1_000_000.0;
                }
            } else if let Some(rest) = line.strip_prefix("fps=") {
                fps = rest.parse().unwrap_or(0.0);
            } else if let Some(rest) = line.strip_prefix("speed=") {
                speed = rest.to_string();
            } else if line.starts_with("progress=") {
                if line == "progress=end" {
                    end_signaled = true;
                    break;
                }
                let d = *duration_stdout.lock().unwrap();
                let percent = match d {
                    Some(t) if t > 0.0 => (elapsed / t * 100.0).min(100.0),
                    _ => 0.0,
                };
                let _ = app_stdout.emit("convert-event", ConvertEvent::Progress {
                    percent,
                    elapsed_sec: elapsed,
                    fps,
                    speed: speed.clone(),
                });
            }
        }

        let status = child.wait();
        *pid_arc_stdout.lock().unwrap() = None;

        let (success, msg) = match status {
            Ok(s) if s.success() => {
                (true, format!("转换完成：{}", out_path_done))
            }
            Ok(s) => {
                (false, format!("ffmpeg 退出，代码：{}", s.code().unwrap_or(-1)))
            }
            Err(e) => (false, format!("等待进程结束出错：{e}")),
        };

        if end_signaled && success {
            let _ = app_stdout.emit("convert-event", ConvertEvent::Done { success: true, message: msg });
        } else {
            let _ = app_stdout.emit("convert-event", ConvertEvent::Done { success, message: msg });
        }
    });

    Ok(())
}

#[tauri::command]
pub fn cancel_convert(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let pid = {
        let mut guard = state.converting_pid.lock().unwrap();
        guard.take()
    };
    let Some(pid) = pid else {
        return Err("当前没有正在进行的转换任务".into());
    };

    let killed = if cfg!(target_os = "windows") {
        Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F", "/T"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    } else {
        Command::new("kill").arg(pid.to_string()).status().map(|s| s.success()).unwrap_or(false)
    };

    let _ = app.emit("convert-event", ConvertEvent::Cancelled {
        message: if killed { "已取消转换".into() } else { "取消命令已发送，可能仍需等待".into() },
    });
    Ok(())
}


// ---------------------------------------------------------------------------
// 原生对话框 & 打开外链
// 说明：withGlobalTauri 只注入 core / event 等基础 API，插件的 JS 接口不会
//       自动挂到 window.__TAURI__ 上。所以文件对话框与外链打开统一在 Rust 侧
//       封装成自定义命令，前端只 invoke 自己的命令，纯静态页面也能跑。
// ---------------------------------------------------------------------------

/// 文件对话框里可选中的扩展名
const MEDIA_EXTS: &[&str] = &[
    "mp4", "mkv", "mov", "avi", "webm", "flv", "wmv", "m4v", "mpg", "mpeg", "ts", "m2ts",
    "3gp", "gif", "mp3", "wav", "aac", "m4a", "flac", "ogg", "opus", "wma", "rm", "rmvb",
    "asf", "vob", "ogv",
];

fn file_path_to_string(fp: tauri_plugin_dialog::FilePath) -> Option<String> {
    fp.into_path().ok().map(|p| p.display().to_string())
}

/// 弹出原生"打开文件"对话框，返回选中的绝对路径（取消返回 None）
#[tauri::command]
pub async fn pick_input_file(app: AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择要转换的文件")
        .add_filter("视频 / 音频文件", MEDIA_EXTS)
        .pick_file(move |p| {
            let _ = tx.send(p.and_then(file_path_to_string));
        });
    rx.await.map_err(|e| format!("对话框异常：{e}"))
}

/// 弹出原生"另存为"对话框，返回选中的绝对路径（取消返回 None）
#[tauri::command]
pub async fn pick_output_file(app: AppHandle, file_name: String) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut builder = app.dialog().file().set_title("选择输出位置");
    if !file_name.is_empty() {
        builder = builder.set_file_name(file_name);
    }
    builder.save_file(move |p| {
        let _ = tx.send(p.and_then(file_path_to_string));
    });
    rx.await.map_err(|e| format!("对话框异常：{e}"))
}

/// 用系统默认浏览器打开 http/https 链接
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("仅允许打开 http/https 链接".into());
    }
    #[cfg(target_os = "windows")]
    let res = Command::new("cmd").args(["/C", "start", "", &url]).spawn();
    #[cfg(target_os = "macos")]
    let res = Command::new("open").arg(&url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let res = Command::new("xdg-open").arg(&url).spawn();
    res.map(|_| ()).map_err(|e| format!("打开链接失败：{e}"))
}
