#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
自动下载 FFmpeg 二进制到 src-tauri/binaries/，供 Tauri sidecar 打包使用。

命名严格遵循 Tauri sidecar 规范：ffmpeg-{target-triple}[.exe]

用法：
    python3 scripts/download_ffmpeg.py                 # 当前主机平台
    python3 scripts/download_ffmpeg.py --all           # 四个主流平台（交叉打包用）
    python3 scripts/download_ffmpeg.py --target macos-aarch64
    python3 scripts/download_ffmpeg.py --clean         # 先清空已有二进制

每个平台都配置了多个下载源，按顺序自动回退；全部失败时会给出手动放置指引。
"""
import argparse
import hashlib
import io
import platform as _platform
import stat
import sys
import tarfile
import urllib.request
import zipfile
from pathlib import Path

UA = {"User-Agent": "ffmpeg-sidecar-fetcher/2.0"}

# ---------------------------------------------------------------------------
# 每个平台多个源，按顺序回退。
#   kind = zip      -> 从 zip 里按后缀挑成员
#   kind = tar.xz   -> 从 tar.xz 里按后缀挑成员
#   kind = raw      -> URL 本身就是单个可执行文件
# 优先取带 libx264 / libvpx / libmp3lame 的 GPL 完整构建。
# ---------------------------------------------------------------------------
TARGETS = {
    "windows-x86_64": {
        "triple": "x86_64-pc-windows-msvc",
        "ext": ".exe",
        "sources": [
            {"kind": "zip",
             "url": "https://github.com/BtbN/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-gpl.zip",
             "match": ["bin/ffmpeg.exe"]},
            {"kind": "zip",
             "url": "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip",
             "match": ["bin/ffmpeg.exe"]},
            {"kind": "raw",
             "url": "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffmpeg-win32-x64"},
        ],
    },
    "macos-aarch64": {
        "triple": "aarch64-apple-darwin",
        "ext": "",
        "sources": [
            {"kind": "raw",
             "url": "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffmpeg-darwin-arm64"},
            {"kind": "zip",
             "url": "https://www.osxexperts.net/ffmpeg71arm.zip",
             "match": ["ffmpeg"]},
        ],
    },
    "macos-x86_64": {
        "triple": "x86_64-apple-darwin",
        "ext": "",
        "sources": [
            {"kind": "raw",
             "url": "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffmpeg-darwin-x64"},
        ],
    },
    "linux-x86_64": {
        "triple": "x86_64-unknown-linux-gnu",
        "ext": "",
        "sources": [
            {"kind": "tar.xz",
             "url": "https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-amd64-static.tar.xz",
             "match": ["/ffmpeg"]},
            {"kind": "raw",
             "url": "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffmpeg-linux-x64"},
        ],
    },
    "linux-aarch64": {
        "triple": "aarch64-unknown-linux-gnu",
        "ext": "",
        "sources": [
            {"kind": "raw",
             "url": "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffmpeg-linux-arm64"},
        ],
    },
}


def detect_host() -> str:
    sysname = _platform.system().lower()
    machine = _platform.machine().lower()
    if sysname == "windows":
        return "windows-x86_64"
    if sysname == "darwin":
        return "macos-aarch64" if machine in ("arm64", "aarch64") else "macos-x86_64"
    if sysname == "linux":
        return "linux-aarch64" if machine in ("arm64", "aarch64") else "linux-x86_64"
    raise RuntimeError(f"不支持的平台：{sysname} {machine}")


def download(url: str) -> bytes:
    req = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(req, timeout=300) as resp:
        total = int(resp.headers.get("Content-Length", 0) or 0)
        data = bytearray()
        block = 256 * 1024
        while True:
            chunk = resp.read(block)
            if not chunk:
                break
            data.extend(chunk)
            if total:
                pct = min(len(data) / total * 100, 100)
                sys.stdout.write(
                    f"\r    下载 {pct:5.1f}%  {len(data)/1048576:.1f}/{total/1048576:.1f} MB"
                )
                sys.stdout.flush()
        if total:
            sys.stdout.write("\n")
        return bytes(data)


def pick_member(names, patterns):
    """按后缀挑压缩包成员，排除 ffprobe。"""
    for pat in patterns:
        cands = [
            n for n in names
            if n.replace("\\", "/").endswith(pat) and "ffprobe" not in n.lower()
        ]
        if cands:
            # 取路径最短的，避免误选 doc/ 之类的副本
            return sorted(cands, key=len)[0]
    return None


def extract(src_cfg: dict, payload: bytes) -> bytes:
    kind = src_cfg["kind"]
    if kind == "raw":
        return payload
    if kind == "zip":
        with zipfile.ZipFile(io.BytesIO(payload)) as zf:
            member = pick_member(zf.namelist(), src_cfg["match"])
            if not member:
                raise RuntimeError("压缩包内未找到 ffmpeg 可执行文件")
            print(f"    从 zip 提取：{member}")
            return zf.read(member)
    if kind == "tar.xz":
        with tarfile.open(fileobj=io.BytesIO(payload), mode="r:xz") as tf:
            member = pick_member(tf.getnames(), src_cfg["match"])
            if not member:
                raise RuntimeError("压缩包内未找到 ffmpeg 可执行文件")
            print(f"    从 tar.xz 提取：{member}")
            fobj = tf.extractfile(member)
            if fobj is None:
                raise RuntimeError(f"无法读取 {member}")
            return fobj.read()
    raise RuntimeError(f"未知压缩格式：{kind}")


def fetch_one(key: str, dest_dir: Path):
    cfg = TARGETS[key]
    target_name = f"ffmpeg-{cfg['triple']}{cfg['ext']}"
    dest = dest_dir / target_name

    if dest.exists():
        print(f"[跳过] {target_name} 已存在（重新下载请加 --clean）")
        return True

    print(f"\n[下载] {key}  ->  {target_name}")
    for i, src in enumerate(cfg["sources"], 1):
        print(f"  源 {i}/{len(cfg['sources'])}：{src['url']}")
        try:
            payload = download(src["url"])
            binary = extract(src, payload)
        except Exception as e:
            print(f"    x 失败：{e}")
            continue

        if len(binary) < 1_000_000:
            print(f"    x 拿到的文件只有 {len(binary)} 字节，判定为无效，换下一个源")
            continue

        dest.write_bytes(binary)
        if not cfg["ext"]:
            mode = dest.stat().st_mode
            dest.chmod(mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        sha = hashlib.sha256(binary).hexdigest()[:16]
        print(f"  √ 已写入 {dest}")
        print(f"    {len(binary)/1048576:.1f} MB  sha256={sha}…")
        return True

    print(f"  x {target_name} 所有下载源均失败。")
    print(f"    可手动下载 ffmpeg 可执行文件，重命名为 {target_name} 后放到：{dest_dir}")
    return False


def main():
    parser = argparse.ArgumentParser(description="下载 FFmpeg sidecar 二进制")
    parser.add_argument("--all", action="store_true", help="下载全部平台")
    parser.add_argument("--target", help="只下载指定平台，如 macos-aarch64")
    parser.add_argument("--clean", action="store_true", help="下载前清空已有 ffmpeg-*")
    args = parser.parse_args()

    project_root = Path(__file__).resolve().parent.parent
    binaries_dir = project_root / "src-tauri" / "binaries"
    binaries_dir.mkdir(parents=True, exist_ok=True)

    print(f"项目根目录：{project_root}")
    print(f"输出目录  ：{binaries_dir}")
    print(f"可选平台  ：{', '.join(TARGETS.keys())}")

    if args.clean:
        print("\n[清理] 删除已有 ffmpeg-* 文件…")
        for f in binaries_dir.glob("ffmpeg-*"):
            print(f"  删除 {f.name}")
            f.unlink()

    if args.target:
        if args.target not in TARGETS:
            sys.exit(f"未知平台：{args.target}")
        keys = [args.target]
    elif args.all:
        keys = list(TARGETS.keys())
    else:
        keys = [detect_host()]
        print(f"检测到当前主机：{keys[0]}")

    ok = all(fetch_one(k, binaries_dir) for k in keys)

    if ok:
        print("\n完成，现在可以执行 cargo tauri build 打包（FFmpeg 会随安装包分发）。")
    else:
        print("\n部分平台未下载成功，详见上面的提示。")
        sys.exit(1)


if __name__ == "__main__":
    main()
