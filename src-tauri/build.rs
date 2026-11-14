use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// 占位文件头部标记。
/// 作用：区分「真实 FFmpeg 二进制」与「为放行编译而生成的占位文件」。
/// 运行时的 commands.rs::is_runnable_ffmpeg 也会读取该标记，命中即跳过该候选路径。
const PLACEHOLDER_MAGIC: &str = "TAURI_FFMPEG_PLACEHOLDER";

/// Tauri sidecar 文件名的规范：ffmpeg-{target-triple}[.exe]
fn sidecar_file_name(triple: &str) -> String {
    if triple.contains("windows") {
        format!("ffmpeg-{triple}.exe")
    } else {
        format!("ffmpeg-{triple}")
    }
}

/// 读取文件头部，判断是否为占位文件
fn is_placeholder(path: &Path) -> bool {
    let Ok(mut f) = fs::File::open(path) else {
        return false;
    };
    let mut buf = vec![0u8; PLACEHOLDER_MAGIC.len()];
    f.read_exact(&mut buf).is_ok() && buf == PLACEHOLDER_MAGIC.as_bytes()
}

/// 写入占位文件（内容为可读说明，用户直接打开也能看懂）
fn write_placeholder(path: &Path) -> std::io::Result<()> {
    let mut f = fs::File::create(path)?;
    writeln!(f, "{PLACEHOLDER_MAGIC}")?;
    writeln!(f, "这是一个占位文件，不是真正的 FFmpeg 可执行文件。")?;
    writeln!(f, "它由 build.rs 自动生成，唯一作用是让 cargo tauri dev / cargo build")?;
    writeln!(f, "顺利通过 sidecar 的存在性校验，从而不下载也能启动界面调试。")?;
    writeln!(f)?;
    writeln!(f, "运行时程序会识别该标记并主动跳过它，回退去用系统 PATH 里的 ffmpeg。")?;
    writeln!(f, "正式打包分发前，请执行 ./download_ffmpeg.sh（Windows 为 download_ffmpeg.bat）")?;
    writeln!(f, "把本文件替换为真正的 FFmpeg 二进制。")?;
    drop(f);

    #[cfg(unix)]
    {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

/// 当前平台的下载脚本名（用于提示文案）
fn download_script(triple: &str) -> &'static str {
    if triple.contains("windows") {
        "download_ffmpeg.bat"
    } else {
        "./download_ffmpeg.sh"
    }
}

/// 保证 sidecar 路径上有文件存在，但不要求它一定是真 FFmpeg：
///   * 已有真实二进制            → 直接放行
///   * 缺失 / 是占位，且 debug   → 自动生成占位文件，编译继续（开发调试零依赖）
///   * 缺失 / 是占位，且 release → 报错中止，避免打出不含 FFmpeg 的安装包，
///     除非显式设置环境变量 FFMPEG_ALLOW_PLACEHOLDER=1
fn ensure_sidecar(triple: &str) {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR 未设置"));
    let dir = manifest_dir.join("binaries");
    let expected = dir.join(sidecar_file_name(triple));

    // 真实二进制已就位，什么也不用做
    if expected.exists() && !is_placeholder(&expected) {
        return;
    }

    let profile = std::env::var("PROFILE").unwrap_or_default();
    let is_release = profile == "release";
    let allow_placeholder = std::env::var("FFMPEG_ALLOW_PLACEHOLDER")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if is_release && !allow_placeholder {
        let bar = "=".repeat(74);
        panic!(
            "\n{bar}\n\
             [缺少随包 FFmpeg 二进制，已中止打包]\n\
             {}\n\n\
             正式发行包需要内置 FFmpeg，最终用户才能开箱即用。请先下载：\n\
             \x20 {}\n\
             \x20 通用 : python3 scripts/download_ffmpeg.py\n\n\
             只是想在开发模式跑界面？执行 cargo tauri dev 即可，\n\
             debug 构建会自动生成占位文件放行，不需要额外操作。\n\n\
             确实要打一个不含 FFmpeg 的包（体积更小，由用户自备 ffmpeg）：\n\
             \x20 FFMPEG_ALLOW_PLACEHOLDER=1 cargo tauri build\n\
             {bar}\n",
            expected.display(),
            download_script(triple),
        );
    }

    if !expected.exists() {
        if let Err(e) = fs::create_dir_all(&dir).and_then(|_| write_placeholder(&expected)) {
            panic!("无法写入 sidecar 占位文件 {}：{e}", expected.display());
        }
    }

    println!(
        "cargo:warning=[FFmpeg] 未找到随包二进制，已生成占位文件：{}",
        expected.display()
    );
    println!("cargo:warning=[FFmpeg] 开发模式不受影响，运行时会自动改用系统 PATH 里的 ffmpeg。");
    println!(
        "cargo:warning=[FFmpeg] 正式打包前请执行 {}",
        download_script(triple)
    );
}

fn main() {
    // 把目标三元组注入编译期常量，供 commands.rs 拼 sidecar 文件名，
    // 保证与 Tauri 打包时使用的命名完全一致。
    let triple = std::env::var("TARGET").unwrap_or_else(|_| String::from("unknown"));
    println!("cargo:rustc-env=BUILD_TARGET_TRIPLE={triple}");
    // 下载完 ffmpeg 后无需 cargo clean，直接重新触发构建脚本即可
    println!("cargo:rerun-if-changed=binaries");

    ensure_sidecar(&triple);
    tauri_build::build();
}
