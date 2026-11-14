@echo off
REM Windows 便捷脚本：下载当前平台的 FFmpeg sidecar 二进制到 src-tauri/binaries/
setlocal
cd /d "%~dp0"

REM 优先用 python，其次用 py 启动器，避免只装了其中一个时失败
set PYEXE=
where python >nul 2>nul && set PYEXE=python
if "%PYEXE%"=="" (where py >nul 2>nul && set PYEXE=py -3)

if "%PYEXE%"=="" (
  echo [错误] 未检测到 Python 3，请先从 https://www.python.org/downloads/ 安装后再运行本脚本。
  pause
  exit /b 1
)

%PYEXE% scripts\download_ffmpeg.py %*
pause
