# RemoteHub Phase 0 首次工程与测试报告

日期：2026-11-21（Asia/Shanghai）。版本：0.1.0。范围：P0-01 与首个 P0-02 主屏捕获实验，P0-07 首轮记录。

**结论：首轮工程可构建，真实 WebView2 窗口和本机 DXGI 捕获已验证。整个 Phase 0 尚未通过，当前不能进行两机远控。**

## 已交付实现

Cargo/pnpm monorepo 和锁文件；Rust 1.100.0 MSVC 固定工具链；Tauri v2 / Vue 3 / TypeScript strict / Tailwind 最小窗口；真实运行信息 IPC；本次同意开关和原生实验并发限制。

DXGI 实现实际枚举主显示器所属适配器，建立 D3D11 device，调用 DuplicateOutput / AcquireNextFrame，复制到 staging texture 后按 RowPitch 读回 BGRA。每帧有单调时间戳；frame acquisition 与 mapping 使用 RAII 释放。等待超时计数；Access Lost 明确退出，要求重新启动实验。没有绕过操作系统桌面保护，也没有启动 Sunshine/Moonlight/EasyTier。

CLI 导出一张 top-down BGRA BMP 与 JSON，不覆盖已有输出。UI 只收到统计，不持有桌面像素；默认无网络远控或输入注入。

## 环境

| 项目 | 本次实际环境 |
|---|---|
| OS | Windows 10 Pro 22H2，19045.6466，x64 |
| 活动采集适配器 | AMD Radeon RX 6750 GRE 10GB |
| 注册表驱动版本 | 32.0.21045.16010 |
| Rust / Cargo | 1.100.0 / 1.100.0 |
| MSVC / Windows SDK | 14.44.35207 / 10.0.26100.0 |
| Node / pnpm | 24.22.3 / 11.25.0 |
| Tauri Rust / Build / JS API / CLI | 2.12.7 / 2.7.6 / 2.12.5 / 2.12.6 |
| WebView2 Runtime | 157.0.4341.60 |
| 浏览器 E2E | 本机 Microsoft Edge，Playwright 1.65.1 |
| 测试拓扑 | 一台 Windows PC；无信令、STUN、TURN 或第二台 PC |

环境机器可用性与计划不同，因此先实测现有 Windows 10；Windows 11 仍需单独验收。最初没有 Rust/MSVC/SDK，已在项目 `.tools` 下准备；未改变全局 PATH、云配置、系统登录自启或现有远控软件。

## 实际测试结果

| 命令/操作 | 结果 | 证据 |
|---|---|---|
| `pnpm lint` | PASS，退出 0 | `artifacts/verification.txt` / JSON |
| `pnpm typecheck` | PASS，退出 0 | 同上 |
| `pnpm test` | PASS，3/3 | 同上 |
| `pnpm build` | PASS，Vue 前端产物生成 | 同上 |
| `cargo fmt --all -- --check` | PASS，退出 0 | 同上 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | PASS，退出 0 | 同上 |
| `cargo test --workspace --locked` | PASS，6/6 Rust 用例，无失败 | 同上 |
| `cargo build -p remotehub-desktop --features custom-protocol --locked` | PASS，原生程序含内嵌前端，无需开发服务器 | 本轮实际编译输出；原生窗口操作记录 |
| `pnpm --filter @remotehub/desktop tauri build --no-bundle` | PASS，release 测试程序 8,553,472 字节 | `artifacts/release-build.txt` |
| release 程序启动烟测 | PASS，启动后持续运行 3 秒，已清理测试进程；未替代正式 UI/安装测试 | `artifacts/release-smoke.txt` |
| `pnpm test:e2e` | PASS，1/1，正常退出 | `artifacts/e2e.txt`、`browser-preview.png` |
| 真实 WebView2 窗口 | PASS，启动/核心 IPC/重新检测/默认采集禁用/关闭 | `artifacts/native-window.txt`；对话中的原生窗口截图 |
| CLI 缺少 `--consent` | PASS，拒绝，退出 1；不产生目录 | `artifacts/capture-cli.txt` |
| CLI `--frames 0` | PASS，拒绝，退出 1；不产生目录 | 同上 |
| `pnpm audit --json` | 已执行，修复后已知漏洞 0 | `artifacts/pnpm-audit.json` |
| `cargo audit --version` | 未安装 cargo-audit，没有执行 RustSec 审计 | 明确未验证，不等于无 Rust 依赖漏洞 |

单元测试用的 IPC 成功/失败样本仅存在测试文件中，不进入产品 UI 状态。浏览器 E2E 使用真实页面、真实 Edge 和实际不可用的桌面 bridge，未注入假成功。原生 UI 实测只验证启动与刷新；没有点击其采集同意开关，真实捕获经 CLI 独立验证。

## DXGI 实测

执行：`target/debug/capture-probe.exe --consent --frames 30 --out artifacts/private/first-run`，退出 0。

| 指标 | 真实结果 |
|---|---|
| 帧尺寸 | 2560×1440 |
| 实际帧数 | 30 |
| 总耗时 | 1059 ms（包含初始化、采集和节流；不包含样本磁盘写入） |
| 观察 FPS | 28.3150319 |
| DXGI 等待超时 | 0 |
| 首/末帧单调时间 | 17741 / 981979 μs（从 capture 实例创建后计时） |
| 首帧文件 | 14,745,654 字节，BMP，BGRA；ffprobe 确认 2560×1440 可解析 |

原始统计：`artifacts/capture-report.json`，包含全部 30 个时间戳。本机原始 BMP/PNG 位于 `artifacts/private/first-run/`，含桌面内容，排除于 Git 与交付 ZIP。样本格式已解析，不以其内容推断编码/视频成功。

本次是短时捕获实验，没有做 20 分钟压测、CPU/GPU/内存基线、编码延迟或端到端操作延迟测量。观察 FPS 按 DXGI 桌面更新计数，静态桌面可能返回较少更新；不能当成 720p30 编码或持续视频性能。

## 发生过的失败与处理

1. 缺少 MSVC 链接器，首个原生测试构建失败。准备官方 MSVC/SDK 后，Rust 测试与原生构建通过。
2. 受限进程中的 HTTPS/SDK 解压无法工作；在正常构建进程中完成官方依赖读取和项目内 SDK 解压，没有关闭证书校验。
3. TypeScript 7 与当前 Vue 类型工具 peer 范围冲突，固定到可验证的 6.0.3，类型检查通过。
4. 宿主 PATH 含异常引号，子命令找不到 Node/pnpm；仅规范化当前构建进程 PATH，复测通过。
5. PowerShell 5 对无 BOM 中文脚本及一个路径正则解析失败；脚本改为 ASCII 提示和字面路径判断，doctor 真实退出 0。
6. 初版 Playwright 成功后开发服务器未退出；改为脚本内启动/关闭 Vite，复跑 1/1 通过并在约 1.4 秒正常退出。
7. npm 审计发现两个开发期传递依赖中危问题（glob / minimatch）。锁定补丁版本覆盖并复测，最终报告漏洞数为 0。此结果不替代完整供应链/分发许可审查。

## 安全和代码审查

审查范围：全部新增 Rust、Tauri 配置、前端 bridge/同意状态、CLI 参数、输出写入、验证与运行脚本。重点核查无授权拒绝优先于 GPU 操作、时间/帧数上限、并发、错误路径资源释放、RowPitch/长度/溢出、不覆盖样本、CSP、无远程网络入口及依赖锁定。未发现首轮范围内已知阻塞级问题。

使用 fmt/clippy/类型检查及全文检索复核；没有占位函数冒充媒体实现。未完成的媒体与安全模块没有创建空实现。原生媒体 UI 的成功、错误、忙碌分支有组件测试，实际 UI 同意后采集整合仍需下一轮操作验证。

## 限制及后续工作

- P0-03 尚未实现 MF H.264；P0-04 尚未实现 webrtc-rs → WebView2 真实视频；P0-05 尚未实现 DataChannel。
- P0-06 尚无视频通路和 coturn 节点；本机无 Docker，未提供第二台 PC 或云端配置。没有云部署成功的结论。
- 身份、DPAPI、配对、签名信令、Session ACL 尚未实现，因此当前不能启用任何远程访问。此限制不是可以在演示时移除的检查。
- 旋转显示器明确拒绝；没有光标合成；失效后不自动恢复；HDR、多 DPI、锁屏、保护内容、显示休眠、分辨率切换未实测。
- 未进行 Windows 11、干净 Windows 系统、安装/卸载、签名、完整传递许可清单与 RustSec 审计。
- 测试版 exe 构建不代表正式发行。退出条件要求两台真实 Windows PC 的连续视频/控制和强制 TURN relay，仍未满足。

下一轮最多三项：P0-03 MF 720p30/H.264/SPS/PPS/IDR；P0-04 受授权的 webrtc-rs/WebView2 本机媒体实验；P0-05 DataChannel 模拟事件。完成后再安排双机和 coturn 实测。

## 交接与恢复

恢复入口：TASKS.md、本文、docs/ADR/0001-phase0-local-probe-boundary.md。
原始需求包未修改；工程 Git 初始提交和压缩包包含代码及非画面测试证据。无需部署回滚；停止测试程序即可停止本机实验。没有常驻远控服务或数据库需要清理。

📊 完成度: Phase 0 已验收 2/7（28.6%） | 主线: 进行中 | 产出: 首次源码、原生客户端与真实 DXGI 实测记录 | 待办: MF/WebRTC/DataChannel/TURN/双机验收 | 下一步: P0-03
