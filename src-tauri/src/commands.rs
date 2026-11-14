use crate::AppState;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

// Windows: 不创建控制台窗口（防止弹出黑框）
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

// Windows平台需要导入这个trait才能使用creation_flags方法
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// Windows平台辅助函数：给Command加CREATE_NO_WINDOW标志，避免弹出黑窗口
#[cfg(target_os = "windows")]
fn no_window(cmd: &mut Command) -> &mut Command {
    cmd.creation_flags(CREATE_NO_WINDOW)
}

/// 非Windows平台占位函数，什么都不做
#[cfg(not(target_os = "windows"))]
fn no_window(cmd: &mut Command) -> &mut Command {
    cmd
}

/// 支持的输出格式列表
#[derive(Serialize, Deserialize, Clone)]
pub struct FormatItem {
    pub ext: String,
    pub label: String,
    pub kind: String,
}

#[tauri::command]
pub fn get_formats() -> Vec<FormatItem> {
    vec![
        FormatItem { ext: "mp4".into(), label: "MP4 (H.264 + AAC，兼容性最好)".into(), kind: "video".into() },
        FormatItem { ext: "mkv".into(), label: "MKV (H.264 + AAC)".into(), kind: "video".into() },
        FormatItem { ext: "mov".into(), label: "MOV (H.264 + AAC，苹果原生)".into(), kind: "video".into() },
        FormatItem { ext: "webm".into(), label: "WebM (VP9 + Opus，网页友好)".into(), kind: "video".into() },
        FormatItem { ext: "avi".into(), label: "AVI (MPEG-4)".into(), kind: "video".into() },
        FormatItem { ext: "flv".into(), label: "FLV (H.264 + MP3，直播/流媒体)".into(), kind: "video".into() },
        FormatItem { ext: "wmv".into(), label: "WMV (Windows Media)".into(), kind: "video".into() },
        FormatItem { ext: "m4v".into(), label: "M4V (H.264 + AAC)".into(), kind: "video".into() },
        FormatItem { ext: "mpg".into(), label: "MPG (MPEG-2，DVD 兼容)".into(), kind: "video".into() },
        FormatItem { ext: "ts".into(),  label: "TS (H.264，HLS 切片)".into(), kind: "video".into() },
        FormatItem { ext: "3gp".into(), label: "3GP (小体积，手机旧格式)".into(), kind: "video".into() },
        FormatItem { ext: "gif".into(), label: "GIF (动图)".into(), kind: "video".into() },
        FormatItem { ext: "mp3".into(), label: "MP3 (320 kbps)".into(), kind: "audio".into() },
        FormatItem { ext: "wav".into(), label: "WAV (无损 PCM)".into(), kind: "audio".into() },
        FormatItem { ext: "aac".into(), label: "AAC (256 kbps)".into(), kind: "audio".into() },
        FormatItem { ext: "flac".into(), label: "FLAC (无损)".into(), kind: "audio".into() },
        FormatItem { ext: "ogg".into(), label: "OGG (Vorbis)".into(), kind: "audio".into() },
    ]
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FfmpegCheckResult {
    pub available: bool,
    pub path: Option<String>,
    pub source: Option<String>,
    pub version: Option<String>,
    pub message: String,
}

fn sidecar_filename() -> String {
    let triple = env!("BUILD_TARGET_TRIPLE");
    if cfg!(target_os = "windows") {
        format!("ffmpeg-{triple}.exe")
    } else {
        format!("ffmpeg-{triple}")
    }
}

const PLACEHOLDER_MAGIC: &str = "TAURI_FFMPEG_PLACEHOLDER";

fn is_placeholder_file(path: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else { return false; };
    let mut buf = vec![0u8; PLACEHOLDER_MAGIC.len()];
    f.read_exact(&mut buf).is_ok() && buf == PLACEHOLDER_MAGIC.as_bytes()
}

fn is_runnable_ffmpeg(path: &Path) -> bool {
    if !path.exists() || !path.is_file() { return false; }
    if is_placeholder_file(path) { return false; }
    let mut cmd = Command::new(path);
    cmd.arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).stdin(Stdio::null());
    no_window(&mut cmd);
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

pub fn locate_ffmpeg(app: &AppHandle) -> Option<(PathBuf, &'static str)> {
    if let Ok(res_dir) = app.path().resource_dir() {
        let c = res_dir.join("binaries").join(sidecar_filename());
        if is_runnable_ffmpeg(&c) { return Some((c, "bundled")); }
        let c2 = res_dir.join(sidecar_filename());
        if is_runnable_ffmpeg(&c2) { return Some((c2, "bundled")); }
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent();
        for _ in 0..5 {
            if let Some(d) = dir {
                let db = d.join("src-tauri").join("binaries").join(sidecar_filename());
                if is_runnable_ffmpeg(&db) { return Some((db, "bundled")); }
                let db2 = d.join("binaries").join(sidecar_filename());
                if is_runnable_ffmpeg(&db2) { return Some((db2, "bundled")); }
                dir = d.parent();
            } else { break; }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let c = if cfg!(target_os = "windows") { dir.join("ffmpeg.exe") } else { dir.join("ffmpeg") };
            if is_runnable_ffmpeg(&c) { return Some((c, "bundled")); }
        }
    }
    let (wc, wa) = if cfg!(target_os = "windows") { ("where", "ffmpeg") } else { ("which", "ffmpeg") };
    let mut which_cmd = Command::new(wc);
    which_cmd.arg(wa).stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null());
    no_window(&mut which_cmd);
    if let Ok(o) = which_cmd.output() {
        if o.status.success() {
            if let Some(line) = String::from_utf8_lossy(&o.stdout).lines().next() {
                let p = PathBuf::from(line.trim());
                if is_runnable_ffmpeg(&p) { return Some((p, "system")); }
            }
        }
    }
    None
}

#[tauri::command]
pub fn check_ffmpeg(app: AppHandle, state: tauri::State<'_, AppState>) -> FfmpegCheckResult {
    let located = locate_ffmpeg(&app);
    {
        let mut cached = state.ffmpeg_path.lock().unwrap();
        *cached = located.as_ref().map(|(p, _)| p.clone());
    }
    let Some((path, source)) = located else {
        return FfmpegCheckResult {
            available: false, path: None, source: None, version: None,
            message: "未检测到 FFmpeg".into(),
        };
    };
    let mut cmd = Command::new(&path);
    cmd.arg("-version").stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null());
    no_window(&mut cmd);
    let ver_output = cmd.output().ok();
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
            ("bundled", Some(v)) => format!("FFmpeg 已就绪（随附版本）\n{v}"),
            ("system",  Some(v)) => format!("FFmpeg 已就绪（系统已安装：{display}）\n{v}"),
            (_, None) => format!("已找到 FFmpeg：{display}"),
            _ => "FFmpeg 可用".into(),
        },
    }
}

#[tauri::command]
pub fn get_output_path(input_path: String, format: String) -> String {
    let p = Path::new(&input_path);
    let parent = p.parent().unwrap_or_else(|| Path::new("."));
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    parent.join(format!("{stem}.{format}")).to_string_lossy().to_string()
}

// ---------------------------------------------------------------------------
// 事件结构：全部带 job_id，便于前端按任务更新
// ---------------------------------------------------------------------------
#[derive(Serialize, Clone)]
#[serde(tag = "type", content = "data")]
pub enum ConvertEvent {
    Queued    { job_id: String, total: usize },
    Start     { job_id: String, pid: u32, input: String, output: String, duration_sec: Option<f64> },
    Progress  { job_id: String, percent: f64, elapsed_sec: f64, fps: f32, speed: String },
    Stderr    { job_id: String, line: String },
    Log       { job_id: String, line: String, level: String }, // info/warn/error
    Done      { job_id: String, success: bool, message: String },
    Cancelled { job_id: String, message: String },
    AllDone   { total: usize, success_count: usize, fail_count: usize, cancel_count: usize },
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
        "-y".into(),
        "-i".into(), input.to_string(),
        "-hide_banner".into(),
        "-progress".into(), "pipe:1".into(),
        "-loglevel".into(), "error".into(),
        "-stats_period".into(), "0.2".into(),
    ];
    match format {
        "mp4" | "m4v" | "mov" => args.extend(["-c:v","libx264","-preset","medium","-crf","23","-c:a","aac","-b:a","192k","-movflags","+faststart"].map(String::from)),
        "mkv" => args.extend(["-c:v","libx264","-preset","medium","-crf","23","-c:a","aac","-b:a","192k"].map(String::from)),
        "webm" => args.extend(["-c:v","libvpx-vp9","-crf","30","-b:v","0","-c:a","libopus","-b:a","128k"].map(String::from)),
        "avi" => args.extend(["-c:v","mpeg4","-q:v","5","-c:a","libmp3lame","-b:a","192k"].map(String::from)),
        "flv" => args.extend(["-c:v","libx264","-preset","medium","-crf","23","-c:a","libmp3lame","-b:a","192k"].map(String::from)),
        "wmv" => args.extend(["-c:v","wmv2","-b:v","2M","-c:a","wmav2","-b:a","192k"].map(String::from)),
        "mpg" => args.extend(["-c:v","mpeg2video","-b:v","4M","-c:a","mp2","-b:a","192k"].map(String::from)),
        "ts"  => args.extend(["-c:v","libx264","-preset","fast","-crf","23","-c:a","aac","-b:a","192k"].map(String::from)),
        "3gp" => args.extend(["-c:v","libx264","-preset","fast","-crf","26","-c:a","aac","-b:a","128k","-s","352x288"].map(String::from)),
        "gif" => args.extend(["-vf","fps=10,scale=480:-1:flags=lanczos","-loop","0"].map(String::from)),
        "mp3" => args.extend(["-vn","-c:a","libmp3lame","-b:a","320k"].map(String::from)),
        "wav" => args.extend(["-vn","-c:a","pcm_s16le"].map(String::from)),
        "aac" => args.extend(["-vn","-c:a","aac","-b:a","256k"].map(String::from)),
        "flac" => args.extend(["-vn","-c:a","flac"].map(String::from)),
        "ogg" => args.extend(["-vn","-c:a","libvorbis","-q:a","5"].map(String::from)),
        _ => {}
    }
    args.push(output.to_string());
    args
}

fn spawn_ffmpeg(exe: &Path, args: &[String]) -> std::io::Result<Child> {
    let mut cmd = Command::new(exe);
    cmd.args(args)
       .stdin(Stdio::null())
       .stdout(Stdio::piped())
       .stderr(Stdio::piped());
    no_window(&mut cmd);
    cmd.spawn()
}

/// 更新任务状态（辅助函数）
fn set_job_state(jobs: &Arc<Mutex<HashMap<String, crate::ConvertJob>>>, job_id: &str,
                 status: &str, progress: Option<f64>, message: Option<&str>) {
    if let Ok(mut m) = jobs.lock() {
        if let Some(j) = m.get_mut(job_id) {
            j.status = status.to_string();
            if let Some(p) = progress { j.progress = p; }
            if let Some(msg) = message { j.message = msg.to_string(); }
        }
    }
}

/// 内部：跑单个转换任务（阻塞），用于 worker 线程
fn run_one_job(
    app: &AppHandle,
    jobs_map: &Arc<Mutex<HashMap<String, crate::ConvertJob>>>,
    current_pid: &Arc<Mutex<Option<u32>>>,
    ffmpeg_exe: &Path,
    job_id: &str,
    input: &str,
    output: &str,
    format: &str,
) -> (bool, String) {
    let args = build_ffmpeg_args(input, output, format);

    let _ = app.emit("convert-event", ConvertEvent::Log {
        job_id: job_id.into(), line: format!("启动 FFmpeg，输出 → {}", output), level: "info".into(),
    });

    let mut child = match spawn_ffmpeg(ffmpeg_exe, &args) {
        Ok(c) => c,
        Err(e) => return (false, format!("启动 FFmpeg 失败：{e}")),
    };

    let pid = child.id();
    {
        let mut g = current_pid.lock().unwrap();
        *g = Some(pid);
    }
    if let Ok(mut m) = jobs_map.lock() {
        if let Some(j) = m.get_mut(job_id) { j.pid = Some(pid); }
    }

    let _ = app.emit("convert-event", ConvertEvent::Start {
        job_id: job_id.into(), pid,
        input: input.into(), output: output.into(), duration_sec: None,
    });

    // stderr 线程：解析总时长 + 输出错误日志
    let stderr = match child.stderr.take() {
        Some(s) => s, None => return (false, "无法获取 stderr".into()),
    };
    let app_e = app.clone();
    let jid_e = job_id.to_string();
    let dur: Arc<Mutex<Option<f64>>> = Arc::new(Mutex::new(None));
    let dur_c = Arc::clone(&dur);
    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        let mut found = false;
        for line in reader.lines().flatten() {
            let trimmed = line.trim().to_string();
            if trimmed.is_empty() { continue; }
            if !found {
                if let Some(d) = parse_duration(&trimmed) {
                    *dur_c.lock().unwrap() = Some(d);
                    found = true;
                    let _ = app_e.emit("convert-event", ConvertEvent::Start {
                        job_id: jid_e.clone(), pid: 0,
                        input: String::new(), output: String::new(), duration_sec: Some(d),
                    });
                }
            }
            let _ = app_e.emit("convert-event", ConvertEvent::Stderr {
                job_id: jid_e.clone(), line: trimmed,
            });
        }
    });

    // stdout 线程：进度解析
    let stdout = match child.stdout.take() {
        Some(s) => s, None => return (false, "无法获取 stdout".into()),
    };
    let app_o = app.clone();
    let jid_o = job_id.to_string();
    let dur_o = Arc::clone(&dur);
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        let mut elapsed: f64 = 0.0;
        let mut fps: f32 = 0.0;
        let mut speed = String::new();
        for line in reader.lines().flatten() {
            let line = line.trim().to_string();
            if line.is_empty() { continue; }
            if let Some(rest) = line.strip_prefix("out_time_ms=") {
                if let Ok(us) = rest.parse::<f64>() { elapsed = us / 1_000_000.0; }
            } else if let Some(rest) = line.strip_prefix("fps=") {
                fps = rest.parse().unwrap_or(0.0);
            } else if let Some(rest) = line.strip_prefix("speed=") {
                speed = rest.to_string();
            } else if line.starts_with("progress=") {
                if line == "progress=end" { break; }
                let d = *dur_o.lock().unwrap();
                let pct = match d {
                    Some(t) if t > 0.0 => (elapsed / t * 100.0).min(100.0),
                    _ => 0.0,
                };
                let _ = app_o.emit("convert-event", ConvertEvent::Progress {
                    job_id: jid_o.clone(), percent: pct, elapsed_sec: elapsed, fps, speed: speed.clone(),
                });
            }
        }
    });

    // 主流程：等子进程退出
    let status = child.wait();
    {
        let mut g = current_pid.lock().unwrap();
        *g = None;
    }
    match status {
        Ok(s) if s.success() => (true, format!("转换完成：{}", output)),
        Ok(s) => (false, format!("FFmpeg 退出，代码：{}", s.code().unwrap_or(-1))),
        Err(e) => (false, format!("等待进程结束出错：{e}")),
    }
}

/// 启动批处理 worker：从队列里依次取任务跑（单实例串行，避免占满 CPU）
/// 这里直接接收 Arc 克隆，可以安全 move 到新线程。
/// 跑完一批后会检查是否有新增的 pending 任务，有就继续跑，直到队列清空才退出。
fn spawn_batch_worker(
    app: AppHandle,
    jobs_map: Arc<Mutex<HashMap<String, crate::ConvertJob>>>,
    ffmpeg_path_state: Arc<Mutex<Option<PathBuf>>>,
    current_pid: Arc<Mutex<Option<u32>>>,
    worker_running_flag: Arc<Mutex<bool>>,
    initial_queue: Vec<String>,
) {

    thread::spawn(move || {
        // 先告诉前端首批排队情况
        for (i, jid) in initial_queue.iter().enumerate() {
            let _ = app.emit("convert-event", ConvertEvent::Queued {
                job_id: jid.clone(), total: initial_queue.len() - i,
            });
        }

        // 定位 ffmpeg（整个 worker 生命周期只查一次）
        let ffmpeg_exe: Option<PathBuf> = {
            let cached = ffmpeg_path_state.lock().unwrap().clone();
            cached.or_else(|| locate_ffmpeg(&app).map(|(p, _)| p))
        };

        // 待处理的队列（按添加顺序）
        let mut queue: VecDeque<String> = VecDeque::from(initial_queue);
        let mut total_ok = 0usize;
        let mut total_fail = 0usize;
        let mut total_cancel = 0usize;
        let mut total_all = queue.len();

        // 主循环：只要队列里还有任务就继续
        loop {
            // 补齐在循环过程中新加入但未入队的 pending 任务
            {
                let m = jobs_map.lock().unwrap();
                for j in m.values() {
                    if j.status == "pending" && !queue.contains(&j.id) {
                        queue.push_back(j.id.clone());
                        total_all += 1;
                        let _ = app.emit("convert-event", ConvertEvent::Queued {
                            job_id: j.id.clone(), total: queue.len(),
                        });
                    }
                }
            }

            let Some(jid) = queue.pop_front() else {
                // 队列空了
                break;
            };

            // 如果找不到 ffmpeg，把剩下所有任务都标失败
            let Some(ref ffmpeg_bin) = ffmpeg_exe else {
                set_job_state(&jobs_map, &jid, "error", Some(0.0), Some("未找到可用的 FFmpeg"));
                let _ = app.emit("convert-event", ConvertEvent::Done {
                    job_id: jid, success: false,
                    message: "未找到可用的 FFmpeg，请先在界面上配置".into(),
                });
                total_fail += 1;
                // 处理剩下队列里的任务
                while let Some(jid2) = queue.pop_front() {
                    set_job_state(&jobs_map, &jid2, "error", Some(0.0), Some("未找到可用的 FFmpeg"));
                    let _ = app.emit("convert-event", ConvertEvent::Done {
                        job_id: jid2, success: false,
                        message: "未找到可用的 FFmpeg，请先在界面上配置".into(),
                    });
                    total_fail += 1;
                }
                break;
            };

            // 先看是否被用户标记为取消
            let cancelled = {
                let m = jobs_map.lock().unwrap();
                m.get(&jid).map(|j| j.status == "cancelled").unwrap_or(false)
            };
            if cancelled {
                total_cancel += 1;
                let _ = app.emit("convert-event", ConvertEvent::Cancelled {
                    job_id: jid, message: "任务已取消，跳过".into(),
                });
                continue;
            }

            // 读取任务信息
            let (input, output, format) = {
                let m = jobs_map.lock().unwrap();
                match m.get(&jid) {
                    Some(j) => (j.input_path.clone(), j.output_path.clone(), j.format.clone()),
                    None => { total_fail += 1; continue; }
                }
            };

            let idx_in_batch = total_ok + total_fail + total_cancel + 1;
            set_job_state(&jobs_map, &jid, "running", Some(0.0),
                Some(&format!("开始处理（{}/?）", idx_in_batch)));
            let _ = app.emit("convert-event", ConvertEvent::Log {
                job_id: jid.clone(),
                line: format!("{} → {}",
                    Path::new(&input).file_name().and_then(|s| s.to_str()).unwrap_or(&input),
                    Path::new(&output).file_name().and_then(|s| s.to_str()).unwrap_or(&output)),
                level: "info".into(),
            });

            let (ok, msg) = run_one_job(&app, &jobs_map, &current_pid, ffmpeg_bin, &jid, &input, &output, &format);

            let was_cancelled = {
                let m = jobs_map.lock().unwrap();
                m.get(&jid).map(|j| j.status == "cancelled").unwrap_or(false)
            };

            if was_cancelled {
                total_cancel += 1;
                set_job_state(&jobs_map, &jid, "cancelled", None, Some("已取消"));
                let _ = app.emit("convert-event", ConvertEvent::Cancelled {
                    job_id: jid, message: "已取消".into(),
                });
            } else if ok {
                total_ok += 1;
                set_job_state(&jobs_map, &jid, "done", Some(100.0), Some(&msg));
                let _ = app.emit("convert-event", ConvertEvent::Done {
                    job_id: jid, success: true, message: msg,
                });
            } else {
                total_fail += 1;
                set_job_state(&jobs_map, &jid, "error", None, Some(&msg));
                let _ = app.emit("convert-event", ConvertEvent::Log {
                    job_id: jid.clone(), line: msg.clone(), level: "error".into(),
                });
                let _ = app.emit("convert-event", ConvertEvent::Done {
                    job_id: jid, success: false, message: msg,
                });
            }
        }

        // 保险：清空 current_pid
        *current_pid.lock().unwrap() = None;
        let _ = app.emit("convert-event", ConvertEvent::AllDone {
            total: total_all, success_count: total_ok, fail_count: total_fail, cancel_count: total_cancel,
        });
        *worker_running_flag.lock().unwrap() = false;
    });
}

/// 批量入参
#[derive(Deserialize)]
pub struct BatchItem {
    pub input: String,
    pub output: String,
    pub format: String,
}

/// 单个转换（兼容旧接口：内部转成单任务批量）
#[tauri::command]
pub fn start_convert(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    input_path: String,
    output_path: String,
    format: String,
) -> Result<String, String> {
    let job_id = uuid_like_id();
    let job = crate::ConvertJob {
        id: job_id.clone(),
        input_path: input_path.clone(),
        output_path: output_path.clone(),
        format: format.clone(),
        status: "pending".into(),
        progress: 0.0,
        message: "排队中…".into(),
        pid: None,
    };
    {
        let mut m = state.jobs.lock().unwrap();
        m.insert(job_id.clone(), job);
    }
    // 单任务模式：若已有 worker 在跑，直接作为 pending 等待即可
    let mut running = state.worker_running.lock().unwrap();
    if !*running {
        *running = true;
        drop(running);
        spawn_batch_worker(
            app,
            Arc::clone(&state.jobs),
            Arc::clone(&state.ffmpeg_path),
            Arc::clone(&state.current_pid),
            Arc::clone(&state.worker_running),
            vec![job_id.clone()],
        );
    }
    Ok(job_id)
}

/// 批量转换
#[tauri::command]
pub fn start_batch_convert(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    items: Vec<BatchItem>,
) -> Result<Vec<String>, String> {
    if items.is_empty() { return Err("任务列表为空".into()); }

    // 若 worker 已经在跑，把新任务追加到 pending 即可（这里直接起一个新 worker 串行跑剩余 pending 更简单：
    // 我们通过 worker_running 标记控制同一时刻只有一个 worker，新的调用会直接把任务加到 jobs 表，
    // 等待现有 worker 结束后再启动新 worker 处理剩下的 pending）。
    let mut ids = Vec::with_capacity(items.len());
    {
        let mut m = state.jobs.lock().unwrap();
        for it in &items {
            if !Path::new(&it.input).exists() { continue; }
            let jid = uuid_like_id();
            m.insert(jid.clone(), crate::ConvertJob {
                id: jid.clone(),
                input_path: it.input.clone(),
                output_path: it.output.clone(),
                format: it.format.clone(),
                status: "pending".into(),
                progress: 0.0,
                message: "排队中…".into(),
                pid: None,
            });
            ids.push(jid);
        }
    }

    let mut running = state.worker_running.lock().unwrap();
    if !*running {
        *running = true;
        drop(running);
        // 收集所有 pending 任务，按插入顺序跑
        let queue: Vec<String> = {
            let m = state.jobs.lock().unwrap();
            m.values()
             .filter(|j| j.status == "pending")
             .map(|j| j.id.clone())
             .collect()
        };
        spawn_batch_worker(
            app,
            Arc::clone(&state.jobs),
            Arc::clone(&state.ffmpeg_path),
            Arc::clone(&state.current_pid),
            Arc::clone(&state.worker_running),
            queue,
        );
    }
    Ok(ids)
}

/// 取消当前正在跑的任务（兼容旧接口）
#[tauri::command]
pub fn cancel_convert(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let pid = { state.current_pid.lock().unwrap().clone() };
    if let Some(pid) = pid { kill_process(pid); }
    // 找到当前 running 的任务并标记为 cancelled
    if let Ok(mut m) = state.jobs.lock() {
        for (_, j) in m.iter_mut() {
            if j.status == "running" {
                j.status = "cancelled".into();
                let _ = app.emit("convert-event", ConvertEvent::Cancelled {
                    job_id: j.id.clone(), message: "已取消".into(),
                });
                break;
            }
        }
    }
    Ok(())
}

/// 取消单个任务（pending 或 running）
#[tauri::command]
pub fn cancel_job(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    job_id: String,
) -> Result<(), String> {
    let (to_kill, is_running) = {
        let mut m = state.jobs.lock().unwrap();
        match m.get_mut(&job_id) {
            Some(j) if j.status == "running" => {
                j.status = "cancelled".into();
                (j.pid, true)
            }
            Some(j) if j.status == "pending" => {
                j.status = "cancelled".into();
                j.message = "已取消".into();
                (None, false)
            }
            _ => return Err("任务不存在或已结束".into()),
        }
    };
    if is_running {
        if let Some(pid) = to_kill { kill_process(pid); }
    } else {
        let _ = app.emit("convert-event", ConvertEvent::Cancelled {
            job_id, message: "已取消".into(),
        });
    }
    Ok(())
}

/// 取消全部：终止当前任务 + 所有 pending 标记为 cancelled
#[tauri::command]
pub fn cancel_all(app: AppHandle, state: tauri::State<'_, AppState>) -> Result<(), String> {
    // 先杀当前进程
    let pid = { state.current_pid.lock().unwrap().clone() };
    if let Some(pid) = pid { kill_process(pid); }
    let mut ids = Vec::new();
    {
        let mut m = state.jobs.lock().unwrap();
        for (_, j) in m.iter_mut() {
            if j.status == "running" || j.status == "pending" {
                j.status = "cancelled".into();
                j.message = "已取消".into();
                ids.push(j.id.clone());
            }
        }
    }
    for jid in ids {
        let _ = app.emit("convert-event", ConvertEvent::Cancelled {
            job_id: jid, message: "已取消".into(),
        });
    }
    Ok(())
}

/// 删除一个已结束的任务（从列表移除）
#[tauri::command]
pub fn remove_job(state: tauri::State<'_, AppState>, job_id: String) -> Result<(), String> {
    let mut m = state.jobs.lock().unwrap();
    if let Some(j) = m.get(&job_id) {
        if j.status == "running" || j.status == "pending" {
            return Err("任务仍在进行中，请先取消".into());
        }
    }
    m.remove(&job_id);
    Ok(())
}

/// 清空所有已结束的任务
#[tauri::command]
pub fn clear_jobs(state: tauri::State<'_, AppState>) -> Result<usize, String> {
    let mut m = state.jobs.lock().unwrap();
    let before = m.len();
    m.retain(|_, j| j.status == "running" || j.status == "pending");
    Ok(before - m.len())
}

fn kill_process(pid: u32) {
    if cfg!(target_os = "windows") {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string(), "/F", "/T"]);
        no_window(&mut cmd);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let _ = cmd.status();
    } else {
        let mut cmd = Command::new("kill");
        cmd.arg(pid.to_string())
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        no_window(&mut cmd);
        let _ = cmd.status();
    }
}

fn uuid_like_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let r: u32 = rand_u32();
    format!("j-{t:x}-{r:08x}")
}

// 避免额外依赖 rand，用一个简单的 xorshift
fn rand_u32() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static S: AtomicU32 = AtomicU32::new(0x12345678);
    let mut x = S.load(Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    S.store(x, Ordering::Relaxed);
    x
}

// ---------------------------------------------------------------------------
// 文件对话框 & 打开外链
// ---------------------------------------------------------------------------
const MEDIA_EXTS: &[&str] = &[
    "mp4","mkv","mov","avi","webm","flv","wmv","m4v","mpg","mpeg","ts","m2ts",
    "3gp","gif","mp3","wav","aac","m4a","flac","ogg","opus","wma","rm","rmvb",
    "asf","vob","ogv",
];

fn file_path_to_string(fp: tauri_plugin_dialog::FilePath) -> Option<String> {
    fp.into_path().ok().map(|p| p.display().to_string())
}

/// 单选文件（兼容旧接口）
#[tauri::command]
pub async fn pick_input_file(app: AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择要转换的文件")
        .add_filter("视频 / 音频文件", MEDIA_EXTS)
        .pick_file(move |p| { let _ = tx.send(p.and_then(file_path_to_string)); });
    rx.await.map_err(|e| format!("对话框异常：{e}"))
}

/// 多选文件（批量导入）
#[tauri::command]
pub async fn pick_input_files(app: AppHandle) -> Result<Vec<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择要转换的文件（可多选）")
        .add_filter("视频 / 音频文件", MEDIA_EXTS)
        .pick_files(move |paths| {
            let list: Vec<String> = paths
                .unwrap_or_default()
                .into_iter()
                .filter_map(file_path_to_string)
                .collect();
            let _ = tx.send(list);
        });
    rx.await.map_err(|e| format!("对话框异常：{e}"))
}

/// 选输出目录（批量输出用）
#[tauri::command]
pub async fn pick_output_dir(app: AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择输出目录")
        .pick_folder(move |p| {
            let _ = tx.send(p.and_then(|fp| fp.into_path().ok().map(|p| p.display().to_string())));
        });
    rx.await.map_err(|e| format!("对话框异常：{e}"))
}

/// 单个"另存为"
#[tauri::command]
pub async fn pick_output_file(app: AppHandle, file_name: String) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut builder = app.dialog().file().set_title("选择输出位置");
    if !file_name.is_empty() { builder = builder.set_file_name(file_name); }
    builder.save_file(move |p| { let _ = tx.send(p.and_then(file_path_to_string)); });
    rx.await.map_err(|e| format!("对话框异常：{e}"))
}

/// 用系统浏览器打开链接
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("仅允许打开 http/https 链接".into());
    }
    let res = if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", &url]);
        no_window(&mut c);
        c.spawn()
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(&url);
        no_window(&mut c);
        c.spawn()
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(&url);
        no_window(&mut c);
        c.spawn()
    };
    res.map(|_| ()).map_err(|e| format!("打开链接失败：{e}"))
}
