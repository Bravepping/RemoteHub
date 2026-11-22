# RemoteHub V2 — Clean Core

自用 Windows 远程控制软件：**Tauri v2 + Vue 3 / TypeScript + Rust**。本分支专注可用远控，不重做已成功的视频与输入链路。

**开发分支**：`v2/clean-core` · **提取基线**：`v2/fast-track@4385454`（含 `main@d7dbc9b` 的真实 LAN 视频与英文输入）。原 `main`、`v2/fast-track` 保持独立。

## 实际状态

| 模块 | 现状 |
| --- | --- |
| 局域网双机 720p H.264 视频 | 来源分支已真实验证：三轮 853 / 900 / 900 帧解码呈现 |
| DataChannel + SendInput | 来源分支实测英文输入；29 / 81 / 132 条消息分别全部收到 ACK |
| 原生授权、停止和临时配对码 | 已有产品 `p3-lan` 代码与历史人工操作确认 |
| 会话结束错误 | **P3-END-01 已通过 Windows 双机局域网回归**（精简分支构建 `77479f2`）：先撤销输入、通知原生停止，再关闭浏览器 DataChannel；正常结束提前发已认证的收尾信号，活动期通道故障仍报错。历史后两轮曾报 `CONTROL_CHANNEL_ERROR` |
| Axum WebSocket 信令 | Rust 原生客户端与 Axum 已在回环打通。两个真实 WebRTC PeerConnection 通过带会话鉴权的 offer/answer/ICE 建立 DataChannel 并传输合成文本；桌面可选构建增加原生授权后的本机信令诊断；实际远控仍使用既有 LAN TCP |
| STUN/ICE + TURN | 历史研究/实验曾通过，V2 正式产品路径尚未集成；**R06 仍 OPEN** |
| Windows 安装包 | 未交付 |

**注意**：表中帧数和输入 ACK 数来自原版历史测试；本次精简分支双机回归未单独记录这些计数。远控只允许在已授权的 Windows 设备上进行。

## 核心目录

| 路径 | 用途 |
| --- | --- |
| `apps/desktop` | Tauri v2 / Vue 3 客户端、五页导航、`RemoteWorkspace.vue` 实际 LAN 控制 UI |
| `crates/hub-capture-win` | DXGI Desktop Duplication |
| `crates/hub-encode-win` | Media Foundation H.264 |
| `crates/hub-webrtc` | WebRTC、H.264、LAN、ICE、DataChannel、`remote_input.rs`、SendInput |
| `crates/hub-identity` | Ed25519 / 当前 Windows 用户 DPAPI、一次性会话签名验证 |
| `crates/hub-rendezvous-probe` | Axum 服务端 `src/bin/v2_signaling.rs` 与 V2 回环集成测试 |
| `crates/hub-signal-v2` | 原生回环 WebSocket 客户端与共享信令协议 |
| `crates/hub-turn-credentials`、`hub-probe-wire` | TURN / 信令协议现有基础 |
| `scripts` | 少量本地工具链检测、开发与验证命令 |
| `deploy/coturn` | 后续 TURN 的示例配置；不代表已部署 |

这里**不包含**旧分支的 `artifacts/` 大量证据、历史 `docs/`、旧任务/报告；它们可在原 `main` 中查阅。代码依赖和自动测试文件随源码保留。本分支不宣称旧证据已在新工作树复现。

## Windows 上快速编译

在已安装项目锁定的 Rust 1.100.0、MSVC/Windows SDK、Node 24、pnpm 11.25.0 和 WebView2 的 Windows PC：

```powershell
git fetch origin
git switch v2/clean-core
git pull --ff-only
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test
cargo test -p hub-webrtc --features p3-lan --locked
cargo test -p remotehub-desktop --features p3-lan --locked
cargo test -p hub-signal-v2 --locked
cargo test -p hub-rendezvous-probe --bin v2_signaling --locked
pnpm build
cargo build -p remotehub-desktop --profile probe --features p3-lan,custom-protocol --locked
```

构建输出由 Cargo profile 决定，通常在 `target/probe/remotehub-desktop.exe`。**最后的 build 仅编译，不会启动或采集桌面。** 实际打开两台电脑的产品程序、测试真实画面/输入/停止前，先明确用户批准并记录 EXE SHA256。

WebSocket 本地信令独立程序采用 `REMOTEHUB_V2_TOKEN`（临时随机 64 位十六进制）、`REMOTEHUB_V2_BIND`（默认 `127.0.0.1:48620`），此二进制只允许回环，TLS 证书/私钥须成对配置。握手使用 Rust 客户端的 Bearer 请求头；共享 token 只用于访问本地中继，不是设备身份。端到端信令复用配对密钥和 HMAC，将消息的房间、会话、方向及顺序绑定；这不加密信令。桌面诊断默认不编入：仅 `v2-loopback-diagnostic,custom-protocol` 构建显示入口，原生确认后连接固定的 `127.0.0.1:48620` 并验证四条内置消息及退出；需先单独启动本机中继，两个进程使用同一临时令牌。不会连接其他设备、采集屏幕或操作键鼠；不替代真实 LAN TCP 路径。

原生库的回环 WSS 使用独立配发的 CA 验证服务端证书链与 IP SAN，不修改系统信任，握手失败不回退到明文。回环测试另用两枚临时 Ed25519 身份签名，通过预先信任的完整公钥校验房间、会话、角色、一次性挑战、有效期及信令内容，再使用 SDP/ICE 建立 DataChannel。该测试的公钥和挑战由测试夹具预先提供；真实跨机公钥配对、授权界面和生产会话接入仍未实现。桌面产品继续使用 LAN TCP，桌面本机诊断仍只使用回环 WS；未部署云端。

## 本分支已执行验证

- Linux：`pnpm install --frozen-lockfile`、`pnpm typecheck`、`pnpm test`（118 项）、`pnpm lint`、`pnpm build` 通过。
- Linux：`cargo test -p hub-webrtc --features p3-lan,v2-loopback-signaling --locked` 22 项通过、1 项忽略；覆盖 V2 信令鉴权以及旧 LAN v1 帧逐字节兼容。
- Linux：`cargo test -p hub-rendezvous-probe --bin v2_signaling --locked` 14 项通过，覆盖 WS/WSS 的原生 WebRTC 回环链路、设备签名后应用 SDP/ICE、认证失败/取消清理、TLS 证书和 IP 校验与禁止降级；未采集屏幕或注入键鼠。`cargo test -p hub-signal-v2 --locked` 2 项、`cargo fmt --all -- --check` 通过。
- Linux：`cargo test -p hub-identity --features ephemeral-test --locked` 7 项通过，覆盖设备角色、错误公钥/会话、篡改、过期与重放。
- Windows x64 MSVC 目标：桌面常规 `p3-lan` 与可选 `v2-loopback-diagnostic` 交叉检查、诊断版严格 Clippy 和 WebRTC 目标测试代码检查通过；未在 Windows 真机运行诊断版。交叉检查本身不是真机证据。
- Windows 实机（用户报告，2026-11-22）：两台电脑在同一局域网使用同一份 `77479f2` Windows x64 包，按约定回归场景测试均正常（画面、自然结束及英文键入后手动停止）。这不覆盖公网、TURN、安装器或 R06 的全量可靠性验证。

## 敏捷开发约定

请阅读根 `AGENTS.md`：**修复关闭错误 → 稳定真实 LAN 会话 → 接通 Axum 信令 → 公网 STUN/TURN → Windows 安装器**。Agent 可连续提交代码/执行离线测试，不为每个小任务撰写 Plan/ADR/Contract/Evidence；每阶段在此 README 用少量文字记录实际结果、失败和遗留问题。真实桌面、键鼠和云端部署始终由用户审批具体操作。

Sunshine、Moonlight V+ PC 与 EasyTier 都不是本分支的必要依赖。
