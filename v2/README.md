# RemoteHub V2 · Fast Track

自用 Windows 远程控制软件。**Tauri v2 + Vue 3 + Rust**，直接复用原版经过实验验证的 DXGI、Media Foundation H.264、WebRTC 和本机身份模块。原版 `main` 不变。

## 进度与验收

| 增量 | 可运行目标 | 当前状态 |
|---|---|---|
| P1 | Axum WebSocket 信令 + 两端握手 | 初版源码和测试已提交，**尚无编译/运行日志** |
| P2 | DXGI → MF H.264 → WebRTC → WebView2 正式 UI 画面 | 待实施 |
| P3 | 经被控端同意的 DataChannel + SendInput | 待实施；原版只做模拟鼠标 |
| P4 | 配对码、身份校验、撤销及会话授权 | 待实施；P1 临时 Bearer + 随机房间不是正式配对 |
| P5 | STUN/ICE 直连 + TURN 回退 | 待实施；原版 R06 历史问题仍开放 |
| P6 | Windows 安装包 + 两台真实电脑验收 | 待实施 |

P1 当前只有信令服务初版，不能宣称产品已经能远控或已经部署服务器。

## P1：独立信令服务

源码：`crates/hub-rendezvous-probe/src/bin/v2_signaling.rs`。复用现有 Axum/Tokio/Rustls 依赖，不重建工程。

- `GET /healthz`：本地健康检查。
- `GET /v2/ws`：Bearer 令牌认证的 WS 协商；`hello` 角色为 host/viewer，返回 waiting/paired。
- `signal`：viewer 发 offer、host 发 answer、双方发 ice；校验顺序、消息大小、角色与限流。
- `leave`：退出后通知对端 peer_left，移除房间；每房间最多两端，房间生命期 10 分钟。
- 默认监听 `127.0.0.1:48620`。非回环监听必须同时配置 TLS 证书和私钥。
- 不采集桌面、不做键鼠输入；不在服务器保存媒体或密钥。

### 本地编译与启动

在具备现有 Rust 工作区依赖的 Windows 环境中，从仓库根目录运行：

```powershell
git switch v2/fast-track
cargo test -p hub-rendezvous-probe --bin v2_signaling --locked
cargo build -p hub-rendezvous-probe --bin v2_signaling --release --locked

# 仅在当前终端生成临时 256 位令牌，不要提交到 Git
$bytes = [byte[]]::new(32)
[System.Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
$env:REMOTEHUB_V2_TOKEN = [Convert]::ToHexString($bytes).ToLowerInvariant()

cargo run -p hub-rendezvous-probe --bin v2_signaling --release --locked
# 另一个终端验证
curl.exe http://127.0.0.1:48620/healthz
```

**重要**：本次 GitHub 修改是远程写入，没有在你的 Windows 环境实际编译或执行。上述测试命令包含独立 Rust 单元测试及真实回环 WS 双端握手测试，结果以 Agent 运行日志为准。

### 服务端配置

| 环境变量 | 含义 |
|---|---|
| `REMOTEHUB_V2_TOKEN` | 必填，64 位十六进制开发 Bearer 令牌 |
| `REMOTEHUB_V2_BIND` | 监听地址；默认 `127.0.0.1:48620` |
| `REMOTEHUB_V2_TLS_CERT` | 对外监听时的 TLS PEM 证书 |
| `REMOTEHUB_V2_TLS_KEY` | 对外监听时的 TLS PEM 私钥 |

服务地址应由你提供准确 IP/域名，不能使用“你的服务器IP”字样猜测实际地址。对外访问使用 `wss://<域名或IP>:48620/v2/ws`，证书必须与访问名称匹配；生产环境不复用临时开发令牌。

### 双端握手帧（占位测试）

WS Upgrade 头带 `Authorization: Bearer <临时令牌>`。host 先进入随机 16 字节 hex 房间，viewer 再进入相同房间：

```json
{"type":"hello","version":2,"room":"0123456789abcdef0123456789abcdef","role":"host"}
```

```json
{"type":"hello","version":2,"room":"0123456789abcdef0123456789abcdef","role":"viewer"}
```

房间标识仅是文档示例，实际必须安全随机生成。双方收到 `paired` 后，viewer 可以转发：

```json
{"type":"signal","seq":1,"signal":"offer","data":"SDP_TEST_ONLY"}
```

后续 P2 才接入真实 SDP/ICE；当前只是消息转发，**不是 WebRTC 会话验收**。任意一方可发送 `{"type":"leave"}`。

## V2 敏捷规则

1. P1–P6 连续实现可运行增量；不强制新建 Plan/ADR/Contract/Evidence。
2. 每阶段只维护本 README 中的状态、测试命令、实际结果及遗留问题；代码保留必要测试和简短注释。
3. Agent 可连续开发、执行离线自动测试并正常提交 Git；遇编译/功能失败先修复，不把未运行测试写成通过。
4. **实际桌面采集、SendInput 人工测试、公网部署、服务器端口/防火墙修改、后台远控等操作需用户确认具体目标与测试步骤**。连续开发授权不等于自动接管用户计算机。
5. 被控端必须明确允许本次观看与控制，控制默认关闭、可见且可随时停止；不绕过系统权限/UAC、不实现隐蔽控制。
6. 不在公开 Git 中提交 TLS 私钥、令牌、真实设备标识或用户画面。

## 复用模块

- `crates/hub-capture-win`：DXGI Desktop Duplication。
- `crates/hub-encode-win`：Media Foundation H.264。
- `crates/hub-webrtc`：WebRTC/ICE/视频/DataChannel 实验能力。
- `crates/hub-identity`：Ed25519 + DPAPI 本地身份。
- `apps/desktop`：Tauri v2 / Vue 3 客户端。
- `crates/hub-rendezvous-probe`：现成 Axum/WebSocket 依赖；V2 新增独立二进制入口。

Sunshine、Moonlight V+ PC、EasyTier 不是 V2 启动的必要依赖，后续如有需要再实现可选兼容。
