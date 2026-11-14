# 视频格式转换器 · Tauri 2 + FFmpeg

极简的视频 / 音频格式转换工具。界面用系统原生浏览器内核渲染
（Windows 用 WebView2、macOS 用 WKWebView、Linux 用 WebKitGTK），
不打包 Chromium，安装包体积可控；转换交给**随包分发**的 FFmpeg 完成，
最终用户装完即用，不需要自己安装或配置任何东西。

---

## 一、先决条件

| 项 | 要求 | 说明 |
|---|---|---|
| Rust | **1.90 或更高** | Tauri 2 当前版本线（`tauri-utils` 2.10+ / `tauri-build` 2.7+ / `tauri-plugin-dialog` 2.8+）的最低要求 |
| Tauri CLI | 2.x | 见下方安装命令 |
| Python 3 | 任意版本 | 仅用于下载随包 FFmpeg 的脚本（开发调试可不用） |

### 第一步：确认并升级 Rust

```bash
rustc -V                      # 低于 1.90 必须先升级
rustup update stable
```

国内网络下给 rustup 加速（写进 `~/.zshrc` 或 `~/.bashrc` 后 `source` 一下）：

```bash
export RUSTUP_DIST_SERVER="https://rsproxy.cn"
export RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup"
```

### 第二步：安装 Tauri CLI

```bash
cargo install tauri-cli --version "^2.0" --locked
```

---

## 二、随包 FFmpeg：开发可跳过，打包必须下载

`src-tauri/tauri.conf.json` 声明了 `bundle.externalBin: ["binaries/ffmpeg"]`，
Tauri 会去找 `src-tauri/binaries/ffmpeg-<target-triple>[.exe]`。

| 场景 | 是否需要先下载 | 行为 |
|---|---|---|
| `cargo tauri dev`（开发调试） | **不需要** | 缺失时 `build.rs` 自动生成占位文件放行构建，界面照常启动 |
| `cargo tauri build`（正式打包） | **必须** | 仍是占位 / 缺失时直接报错中止，避免打出不含 FFmpeg 的包 |

占位文件带 `TAURI_FFMPEG_PLACEHOLDER` 头部标记，运行时会被识别并跳过，
程序继续按「程序同目录 → 系统 PATH」的顺序去找真正的 FFmpeg；
系统里也没有时，界面弹出引导卡，而不是崩溃。

下载命令：

```bash
chmod +x download_ffmpeg.sh
./download_ffmpeg.sh                  # macOS / Linux，自动识别当前平台
```

```bat
download_ffmpeg.bat                   REM Windows
```

其他用法：

```bash
python3 scripts/download_ffmpeg.py --all                        # 下载全部平台
python3 scripts/download_ffmpeg.py --target macos-aarch64       # 指定平台
python3 scripts/download_ffmpeg.py --clean                      # 清空后重下
```

每个平台配了多个下载源（BtbN / gyan.dev / osxexperts / johnvansickle / eugeneware），
按顺序自动回退；全部失败时会告诉你手动该放到哪个路径。

生成的文件名必须严格符合 Tauri sidecar 规范：

| 平台 | 文件名 |
|---|---|
| Windows x64 | `ffmpeg-x86_64-pc-windows-msvc.exe` |
| macOS Apple Silicon | `ffmpeg-aarch64-apple-darwin` |
| macOS Intel | `ffmpeg-x86_64-apple-darwin` |
| Linux x64 | `ffmpeg-x86_64-unknown-linux-gnu` |
| Linux arm64 | `ffmpeg-aarch64-unknown-linux-gnu` |

> 想在没下载 FFmpeg 的情况下打一个「不含 FFmpeg」的小体积包（由用户自备 ffmpeg），
> 执行 `FFMPEG_ALLOW_PLACEHOLDER=1 cargo tauri build` 强制放行即可。

---

## 三、开发与打包

```bash
cargo tauri dev        # 开发调试（无 dev server，直接加载 src/index.html）
cargo tauri build      # 打正式安装包（需先下载 FFmpeg，见上一节）
```

- `cargo tauri dev` 可以直接跑，**不需要**先准备 FFmpeg；界面会按当前机器实际情况显示
  「已就绪」或「未检测到」两种状态。
- 控制台出现 `[FFmpeg] 未找到随包二进制，已生成占位文件…` 的 warning 属正常现象，
  它是开发模式的放行机制，不是错误。
- `cargo tauri build` 若提示 `[缺少随包 FFmpeg 二进制，已中止打包]`，
  按提示跑一次下载脚本即可。

产物位置：`src-tauri/target/release/bundle/`

---

## 四、Windows 分发

- 执行 `cargo tauri build` 会同时产出 **NSIS 安装包（.exe）** 和 **MSI 安装包（.msi）**，
  位于 `src-tauri/target/release/bundle/nsis/` 与 `.../msi/`。
- NSIS 已配置中文 / 英文双语选择器，默认**按当前用户安装**，不需要管理员权限。
- WebView2 采用 `downloadBootstrapper`：Win10 1803+ / Win11 一般自带；
  没装的机器会在安装时自动从微软拉取。要做**完全离线**包，把它改成 `"embedBootstrapper"`。
- 用户拿到的安装包**已内置 FFmpeg**，双击安装完打开就能拖文件转码。
- 包体积约 **80–120 MB**（体积主要来自随包的 FFmpeg）。

> ⚠️ Tauri **不支持跨平台交叉编译**。要出 Windows 安装包，必须在 Windows 机器上执行
> `cargo tauri build`；macOS 上只能出 `.app` / `.dmg`。
> 在 macOS 上可以先跑 `./download_ffmpeg.sh --all` 把各平台二进制都备好。

### 国内打 Windows 包时 NSIS 工具下载失败

Tauri 首次在 Windows 上打包会下载 NSIS 工具链，国内可能超时。手动放置即可：

1. 下载 `nsis-3.11.zip`：<https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.11/nsis-3.11.zip>
2. 解压到 `%LOCALAPPDATA%\tauri\NSIS\`
3. 下载 `nsis_tauri_utils.dll`：<https://github.com/tauri-apps/nsis-tauri-utils/releases>
   放入 `%LOCALAPPDATA%\tauri\NSIS\Plugins\x86-unicode\`
4. 重新执行 `cargo tauri build`

---

## 五、工程结构

```
video_converter_tauri/
├── download_ffmpeg.sh / .bat          # 一键下载随包 FFmpeg
├── scripts/download_ffmpeg.py         # 下载脚本本体（多源回退）
├── src/index.html                     # 前端（纯 HTML/CSS/JS，无 npm / 无打包器）
└── src-tauri/
    ├── Cargo.toml
    ├── build.rs                       # 注入 target triple；缺 FFmpeg 时生成占位放行 dev
    ├── tauri.conf.json                # 窗口 / 打包 / sidecar 声明
    ├── capabilities/default.json      # 权限：core + dialog
    ├── icons/                         # 含 .ico 与 .icns
    ├── binaries/                      # 随包 FFmpeg 放这里（开发时是自动生成的占位文件）
    └── src/
        ├── main.rs                    # 入口
        ├── lib.rs                     # Builder、全局状态、命令注册
        └── commands.rs                # FFmpeg 定位、转码、进度解析、取消
```

---

## 六、FFmpeg 的查找顺序

程序启动时按下面的优先级定位 FFmpeg，界面会显示实际用的是哪一个：

1. **随包 sidecar**：`<resource_dir>/binaries/ffmpeg-<triple>[.exe]`（正式发行版命中这一层）
2. **开发模式**：项目内 `src-tauri/binaries/ffmpeg-<triple>[.exe]`
3. **程序同目录**：用户手动把 `ffmpeg.exe` / `ffmpeg` 放在可执行文件旁边
4. **系统 PATH**：`which ffmpeg` / `where ffmpeg` 兜底

前两层如果拿到的是占位文件（带 `TAURI_FFMPEG_PLACEHOLDER` 标记），会被直接跳过，
继续往后找，所以开发阶段不下载也不会用到一个假的 ffmpeg。

四层都没找到时，界面会弹出一张引导卡，按当前系统给出下载页、一键复制安装命令、
以及「把 ffmpeg 放到这个目录」的绝对路径，并提供「我已安装，重新检测」按钮，无需重启程序。

---

## 七、常见问题

**1. `error on build > devUrl: "../src/index.html" is not a "uri"`**

Tauri 2 里 `devUrl` 只能填 URL（dev server 地址）。本项目不用 dev server，
已改为只保留 `"frontendDist": "../src"`，dev 模式直接加载这个目录。

**2. `Additional properties are not allowed ('fileDropEnabled' was unexpected)`**

Tauri 2 已把窗口的该字段改名为 **`dragDropEnabled`**，配置里已同步。

**3. `error: rustc 1.86.0 is not supported by the following packages`**

升级工具链：`rustup update stable`（配好 rsproxy 后很快）。
也可以 `rustup toolchain install stable` 后 `rustup override set stable`。

**4. `resource path "binaries/ffmpeg-x86_64-apple-darwin" doesn't exist`**

这是 Tauri 在**构建阶段**对 `externalBin` 做的存在性校验，和运行时无关，
所以「没下载就不让编译」是它的默认行为。

- 开发调试：本工程已由 `build.rs` 自动生成占位文件放行，直接 `cargo tauri dev` 即可，
  不需要任何额外操作。
- 正式打包：执行 `./download_ffmpeg.sh`（Windows 用 `download_ffmpeg.bat`）下载真实二进制，
  或加 `FFMPEG_ALLOW_PLACEHOLDER=1` 打一个不含 FFmpeg 的包。

**5. 界面提示「未检测到 FFmpeg」**

正式发行包不会出现此提示（FFmpeg 已内置）。开发模式出现时，跑一下下载脚本，
或按界面引导卡装一个到系统里，再点「我已安装，重新检测」即可。

**6. crates.io 依赖下载慢**

编辑 `~/.cargo/config.toml`：

```toml
[source.crates-io]
replace-with = 'rsproxy-sparse'

[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"

[net]
git-fetch-with-cli = true
```

---

## 八、支持的输出格式

**视频（12 种）**：MP4、MKV、MOV、WebM、AVI、FLV、WMV、M4V、MPG、TS、3GP、GIF

**纯音频提取（5 种）**：MP3、WAV、AAC、FLAC、OGG

主流格式已预设合理编码参数（H.264 / VP9 视频，AAC / MP3 / FLAC 音频）。

---

## 九、许可提醒

下载脚本默认取 **GPL 完整版** FFmpeg（因为要包含 `libx264` 才能编码 MP4，
而 libx264 是 GPL 协议）。个人使用、内部分发、开源项目都可以直接用。

若要**闭源商业分发**，需要二选一：

1. 遵守 FFmpeg 的 GPL 条款（开放你的应用源码）；
2. 换用 LGPL 版 FFmpeg 构建，并把 `commands.rs` 里依赖 `libx264` 的
   `-c:v libx264` 参数改成 LGPL 兼容的编码器。

> eugeneware/ffmpeg-static 提供的二进制是单文件裸可执行文件，本项目把它作为
> BtbN / gyan.dev 之外的备用源。
