# P0-04 开发与交付验证

日期：2026-11-21。基线：`7fcbb519709a4655aa77aa4b8eca945c413891c2`。**P0-04 仍是 doing：P0-04a 已实现并完成自动化验证，未完成 Windows 原生验收；P0-04b 只有离线签名信令前置组件，未接入双机媒体。**

基线的 README 与 TASKS 均已标明 P0-03 本机 H.264 编码完成；没有重复修旧状态。本轮 README/TASKS 仅推进 P0-04 状态，不重写 P0-03 的历史证据。

## 实现与变更

| 范围 | 变更 |
| --- | --- |
| `crates/hub-encode-win` | 共享 P0-03 的 DXGI→NV12/MF 和采集租约；实时生产者在专用阻塞执行线程持有 COM/DXGI；完整 H.264 AU 无文件输出，停止与错误清理；原 CLI 保留。 |
| `crates/hub-webrtc` | 锁定 webrtc-rs 0.21.1；单路 H.264、RTP PT 96、模式 1、Baseline offer `42001f`，验证实际 SPS / 协商 level / 真正选中的同机候选。以实际 100ns PTS 四舍五入换算 90kHz 时钟，正常帧间隔 3000 ticks；大 NAL 使用 FU-A，只有 AU 最后一个包置 marker。 |
| `apps/desktop/src-tauri` | 内嵌应用的 Windows feature 才启用；主窗口 IPC 能力、Windows 原生同意框、一次一轮、5–30 秒。选中本机候选对后才开始采集。4 AU 队列满即中止，不丢参考帧后继续。停止、关窗、超时与异常由同一个取消和资源回收流程处理。 |
| `apps/desktop/src/features/p004` | 真实 JS PeerConnection 与 WebView2 `<video srcObject>`；IPC 只传信令和统计。不将 connected 当成播放。来源、提交 RTP、驱动 RTP、收到包、解码与呈现帧分别显示；不可读数据标记为「未测」。 |
| P0-04b 前置 | 独立测试 Ed25519 签名信令绑定与限时一次性验证。不联网、不改变同机入口；未接入设备配对、私钥存储、真实桌面跨主机通道。见 [P0-04b 计划](PHASE0_P004B_PLAN.md)。 |

本轮没有公网或局域网监听、STUN/TURN、远程输入、DataChannel、配对界面、服务端。传输层仅监听 IPv4 `127.0.0.1` UDP 临时端口。完整非 trickle SDP 通过 IPC；Rust 拒绝非同机候选并复核真实路径。RTCP-mux 下过滤 webrtc-rs 自动生成的冗余 component 2 本机候选，不接受对端的 component 2。Windows 特殊 Chromium 参数和独立 profile 只用于此实验，详见 [ADR 0003](ADR/0003-p004-loopback-media.md)。

## 已执行

环境：Debian 13 x86_64、Rust/Cargo 1.100.0、Node 24.22.3、pnpm 11.25.0。Windows MSVC 目标在 Linux 用 LLVM 工具交叉检查，不是 Windows 上的链接、运行或 GPU 证据。命令及退出码见 [verification.txt](../artifacts/p004/verification.txt)。

| 项目 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 退出 0。 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Linux 退出 0。 |
| `cargo test --workspace --locked` | Linux 退出 0；29 项通过：采集 6、编码 13、WebRTC/签名信令 10。 |
| MSVC 目标 `cargo check` / 启用实验的 `cargo clippy --all-targets` | 两项退出 0。包含 Windows 专用代码及测试的静态检查，未执行 Windows 测试二进制。 |
| `pnpm lint / typecheck / test / build` | 全部退出 0；Vitest 20 项通过。 |
| `pnpm test:e2e` | 退出 1：此环境无 Microsoft Edge；该套件仅预览，不是 WebView2 验收。 |

真网络自动化的 `sends_and_receives_rtp_over_real_loopback_only` 使用两个 Rust PeerConnection，实际完成回环 ICE、DTLS/SRTP，并验证接收端拿到 H.264 RTP PT 96、IDR 包与 marker。测试只发送合成 NAL，不代表推送了 Windows 画面或执行了解码。生产 PTS/GOP、MFT 名称和浏览器选中候选、解码帧、尺寸、首帧延迟、连续播放时长均**尚无本轮真机数据**。RTCP 反馈按可测字段分列，空值不记为 0。

## Windows 真机验收（待执行）

1. 用仓库 README 中的 Windows 开发环境和命令构建 `p004-loopback,custom-protocol`。不使用 Vite 浏览器窗口作为替代。记录 Windows、显卡、驱动、WebView2 Runtime 版本与提交号。
2. 先把测试机主屏放在可分享的环境。运行 `powershell -NoProfile -STA -ExecutionPolicy Bypass -File scripts/p003-motion.ps1 -Seconds 35`。在 RemoteHub 选择 15–20 秒，点击开始并在 **Windows 原生** 确认框同意。检查确实看到运动画面、1280×720、解码计数增加、连续播放 ≥10 秒；记录首帧、解码、丢包、SPS/fmtp、候选类型和实测时长。必要时第二块屏观察播放器，或者避免将播放器占满被捕获的主屏。
3. 从新会话依次测试拒绝同意、重复启动、手动停止、关窗、自动超时；至少五次启动/停止，验证可再启动。编码/ICE 失败案例需要记录错误码并确认无残留采集；不得以「按钮已变灰」代替资源回收。若报 `ICE_LOOPBACK_UNAVAILABLE` 或 WebView2 忽略实验参数，记录版本和失败点，不放宽绑定、不关闭防火墙。
4. 运行 `cargo build -p hub-encode-win --bin encode-probe --release --locked` 后，用 `./target/release/encode-probe.exe --consent --seconds 10 --encoder auto --name p004-p003-regression` 回归旧 CLI；按 P0-03 报告核对输出和 FFmpeg 独立解码。原始桌面文件、完整 SDP、网络地址和诊断截图只留私有目录；公开证据只记录必要、无地址的统计或脱敏运动窗口证据。
5. 完成设备身份、配对、会话批准/撤销以后，才在两台有授权的 Windows PC 上执行 P0-04b；两机验证须另记结果。同机通过不等于双机通过。

P0-04a 跨平台验证先排除了 RTP、约束及部分清理逻辑问题；下一关是原生 WebView2 回环候选与真实解码，其后才接入跨机授权链路。
