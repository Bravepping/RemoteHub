# RemoteHub V2 — Clean Core

自用 Windows 远程控制软件：**Tauri v2 + Vue 3 / TypeScript + Rust**。本分支专注可用远控，不重做已成功的视频与输入链路。

**开发分支**：`v2/clean-core` · **提取基线**：`v2/fast-track@4385454`（含 `main@d7dbc9b` 的真实 LAN 视频与英文输入）。原 `main`、`v2/fast-track` 保持独立。

## 实际状态

| 模块 | 现状 |
| --- | --- |
| 局域网双机 720p H.264 视频 | 来源分支已真实验证：三轮 853 / 900 / 900 帧解码呈现 |
| DataChannel + SendInput | 来源分支实测英文输入；29 / 81 / 132 条消息分别全部收到 ACK |
| 原生授权、停止和临时配对码 | 已有产品 `p3-lan` 代码与历史人工操作确认 |
| 会话结束错误 | **P3-END-01 OPEN**：后两轮 `CONTROL_CHANNEL_ERROR`；尽管资源正常回收，根因未确定 |
| Axum WebSocket 信令 | `v2_signaling.rs` 已存在；**本分支尚未编译、执行或接入产品验证** |
| STUN/ICE + TURN | 历史研究/实验曾通过，V2 正式产品路径尚未集成；**R06 仍 OPEN** |
| Windows 安装包 | 未交付 |

**注意**：上述视频/键鼠结果是原版历史测试，不是当前精简分支重新运行的记录。远控只允许在已授权的 Windows 设备上进行。

## 核心目录

| 路径 | 用途 |
| --- | --- |
| `apps/desktop` | Tauri v2 / Vue 3 客户端、五页导航、`RemoteWorkspace.vue` 实际 LAN 控制 UI |
| `crates/hub-capture-win` | DXGI Desktop Duplication |
| `crates/hub-encode-win` | Media Foundation H.264 |
| `crates/hub-webrtc` | WebRTC、H.264、LAN、ICE、DataChannel、`remote_input.rs`、SendInput |
| `crates/hub-identity` | Ed25519 / 当前 Windows 用户 DPAPI |
| `crates/hub-rendezvous-probe` | 现有 Axum 依赖 + `src/bin/v2_signaling.rs` |
| `crates/hub-turn-credentials`、`hub-probe-wire` | TURN / 信令协议现有基础 |
| `scripts` | 少量本地工具链检测、开发与验证命令 |
| `deploy/coturn` | 后续 TURN 的示例配置；不代表已部署 |

这里**不包含**旧分支的 `artifacts/` 大量证据、历史 `docs/`、旧任务/报告；它们可在原 `main` 中查阅。代码依赖和自动测试文件随源码保留。本分支不宣称旧证据已在新工作树复现。

## Windows 上快速编译

在已安装 Rust 1.90、MSVC/Windows SDK、Node/pnpm 11、WebView2 的 Windows PC：

```powershell
git switch v2/clean-core
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test
cargo test -p hub-webrtc --features p3-lan --locked
cargo test -p hub-rendezvous-probe --bin v2_signaling --locked
cargo build -p remotehub-desktop --profile probe --features p3-lan,custom-protocol --locked
```

构建输出由 Cargo profile 决定，通常在 `target/probe/remotehub-desktop.exe`。**最后的 build 仅编译，不会启动或采集桌面。** 实际打开两台电脑的产品程序、测试真实画面/输入/停止前，先明确用户批准并记录 EXE SHA256。

WebSocket 本地信令独立程序采用 `REMOTEHUB_V2_TOKEN`（临时随机 64 位十六进制）、`REMOTEHUB_V2_BIND`（默认 `127.0.0.1:48620`），对外监听必须配置 `REMOTEHUB_V2_TLS_CERT` 和 `REMOTEHUB_V2_TLS_KEY`。当前**尚未接入真实远控会话，也没有部署你的阿里云服务器**。

## 敏捷开发约定

请阅读根 `AGENTS.md`：**修复关闭错误 → 稳定真实 LAN 会话 → 接通 Axum 信令 → 公网 STUN/TURN → Windows 安装器**。Agent 可连续提交代码/执行离线测试，不为每个小任务撰写 Plan/ADR/Contract/Evidence；每阶段在此 README 用少量文字记录实际结果、失败和遗留问题。真实桌面、键鼠和云端部署始终由用户审批具体操作。

Sunshine、Moonlight V+ PC 与 EasyTier 都不是本分支的必要依赖。
