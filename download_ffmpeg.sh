#!/usr/bin/env bash
# 便捷脚本：下载当前平台的 FFmpeg sidecar 二进制到 src-tauri/binaries/
# 所有参数原样透传给 scripts/download_ffmpeg.py，例如：
#   ./download_ffmpeg.sh --all
#   ./download_ffmpeg.sh --target windows-x86_64
#   ./download_ffmpeg.sh --clean
set -e
cd "$(dirname "$0")"

if command -v python3 >/dev/null 2>&1; then
  PY=python3
elif command -v python >/dev/null 2>&1; then
  PY=python
else
  echo "[错误] 未检测到 Python 3，请先安装：https://www.python.org/downloads/"
  exit 1
fi

"$PY" scripts/download_ffmpeg.py "$@"
