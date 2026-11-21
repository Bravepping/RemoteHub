# RemoteHub · Phase 0 技术验证

这是已生成并在本机编译测试的 Windows 原型：**Tauri 客户端 + DXGI 主屏采集 + Media Foundation H.264 + Rust WebRTC → WebView2**。同机视频与默认关闭的指定局域网双机实验均已实测。DataChannel、正式配对/服务端与 TURN 尚未完成，整个 Phase 0 尚未通过。

原始需求基线位于 `docs/RemoteHub_完整开发文档_v1.0.md`；首轮范围、测量结果、未完成项见 `docs/PHASE0_TEST_REPORT.md` 和 `TASKS.md`。

P0-03 新实测见 `docs/PHASE0_P003_TEST_REPORT.md`：AMD 硬件 MFT，10 秒 1280×720 / 300 帧 / 零丢帧，标准解码与八色条检查通过。原始裸流不能自带容器 PTS，请用 sidecar 中的 MF 时间戳；开发验证生成的 MP4 已检查为 30fps、10 秒。

## P0-04a 同机视频实验

本机原生 WebView2 已实测：五轮 15 秒各 450 帧、一轮 30 秒 900 帧全部接收/解码/呈现；实际选中 127.0.0.1、接收丢包 0，主动停止与播放中关窗回收通过。报告见 [P0-04a 测试报告](docs/PHASE0_P004_TEST_REPORT.md)。P0-04 两机任务仍为 doing。

普通构建默认关闭此入口。显式构建实验版：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p004.ps1
# 只构建、不打开窗口
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p004.ps1 -BuildOnly
```

窗口内每轮须重新勾选本次同意，并由原生确认框批准；5–30 秒，随时停止或关窗。视频经本机 WebRTC/SRTP 传送，未走磁盘视频或 IPC 像素。结束后仅在程序目录 `artifacts/private/p004/` 保存统计，不保存画面。直接运行本轮交付包的 `RemoteHub-P004a.exe` 可进入该实验。

AMD 驱动实际 SPS 标识为 `42041f`，其中不适用约束位按 H.264 解码规则处理；不修改原 SPS，SDP 为 Baseline `42001f`。驱动瑕疵与互通边界见 [设计说明](docs/P004_FORMAT.md)。

## P0-04b 指定局域网双机实验

显式构建 `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p004b.ps1 -BuildOnly`，生成的优化程序支持发送/接收两个角色。每轮两端原生确认、一次性码认证，原生TCP/UDP仅允许指定对端，连接中断或最长30秒后停止；统计保存到程序目录 `artifacts/private/p004b`。普通构建和 P004a 的网络范围不变。

操作与限定防火墙设置见 [双机操作说明](docs/P004B_RUNBOOK.md)，范围/兼容性见 ADR0004。首轮RTCP候选校验错误已修复；v2真实双机30秒900帧全部接收/解码/呈现，1280×720、丢包0，主动停止/关窗/拔线回收通过。详见 [双机测试报告](docs/PHASE0_P004B_TEST_REPORT.md)。本轮不保存画面，基线要求的录屏未留存，P004父项仍保留证据差异。

## P0-03 本机编码 probe

以下命令在源码根目录执行，编译后也可直接运行 `target/release/encode-probe.exe`。默认不采集，只有显式 `--consent` 才采集；输出限定在 `artifacts/private/NAME` 的新目录。

```powershell
Set-ExecutionPolicy -Scope Process Bypass
. ./scripts/use-toolchain.ps1
cargo build -p hub-encode-win --bin encode-probe --release --locked
./target/release/encode-probe.exe --consent --seconds 10 --encoder auto --name my-local-test
```

`--encoder software` 可独立验证软件回退；`--stop-after-ms 2000` 用于主动停止测试，控制台 Ctrl+C 也请求正常停止并 drain。最长 30 秒，不覆盖已有样本。运动源复现、独立 FFmpeg 解码和统计口径见 P0-03 报告；FFmpeg 只用于开发验收，不是客户端运行依赖。原有 Tauri 采集窗口没有接入新编码器，本轮无需操作该窗口。

## 使用已编译程序

解压首次交付 ZIP，双击 `RemoteHub.exe`。需要 Windows x64 和已安装的 WebView2 Runtime。窗口启动时不采集；明确勾选本次同意后，点击“开始采集实验”，运行最多 5 秒并显示真实统计。窗口不保存截图或发送画面。关闭窗口即退出。

`capture-probe.exe` 是开发用的独立本机实验工具，不是产品运行依赖。只有显式执行下列命令才会保存当前主屏的 BMP 和 JSON：

```powershell
.\capture-probe.exe --consent --frames 30 --out .\private-capture-001
```

输出目录必须不存在，避免覆盖样本。样本包含桌面内容，自行决定保留。本轮软件为未签名测试构建，尚无安装器，也未做干净系统安装验证。

## 从源码运行

要求 Rust 1.100.0 MSVC、Microsoft C++ Build Tools / Windows SDK、Node 24、pnpm 11.25.0、WebView2。准备方式参照 [Tauri 官方 Windows 前置条件](https://v2.tauri.app/start/prerequisites/)。本机临时开发工具位于 `.tools`，没有修改全局 PATH；该目录不包含在源码 ZIP 中。换机请准备官方工具链，或在已配置的 Developer PowerShell 中执行。

在工程根目录运行：

```powershell
pnpm install --frozen-lockfile
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/doctor.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/dev.ps1 -Mode Desktop
```

其他入口：

```powershell
# 完整格式/类型/前端/Rust 测试，保存原始输出与各命令退出码
pnpm verify
# 浏览器验证需要本机 Microsoft Edge，仅作为预览测试
pnpm test:e2e
# 构建内嵌前端资源的独立 Windows 程序，不生成安装器
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/dev.ps1 -Mode Build
# 明确授权有限本机采集并导出样本
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/dev.ps1 -Mode Capture
```

`Browser` 模式只用于页面预览，无法调用 DXGI。不能把浏览器预览当成原生窗口已通过的证据。默认没有远程监听、配对旁路、键鼠注入、后台采集、自启或云端部署。

## 工程内容

- `apps/desktop`：Vue 3 / TypeScript / Tailwind / Tauri v2，受限本机 IPC。
- `crates/hub-capture-win`：D3D11 / DXGI Desktop Duplication、限时实验、BGRA 读回及 BMP 导出。
- `crates/hub-encode-win`：MF 编码、CLI 文件实验与无文件实时 AU 生产者。
- `crates/hub-webrtc`：只绑定 loopback 的本机视频会话、SDP/RTP、3 AU 队列与清理。
- `scripts`：环境检测、运行入口、可保存结果的验证脚本。
- `artifacts`：非画面测试证据；`artifacts/private` 中的桌面样本不进 Git 或交付 ZIP。
- `docs/ADR`：本机实验边界、未开放远控能力的原因及后续接入条件。

首轮真实测试：Windows 10 22H2，AMD Radeon RX 6750 GRE 10GB；30 帧、2560×1440、1059ms、观察帧率 28.315 FPS。该数字是 DXGI 收到桌面更新的计数，**不是视频编码、传输或解码帧率**。
