# RemoteHub｜P0-05 审查结论与 P0-06a Agent 开发任务

> 交付日期：2026-11-21  
> 仓库：https://github.com/Bravepping/RemoteHub  
> 本次核对的 main：`079275d1875e78bfbc811a429a52e4dbb539cb5a`  
> 审查性质：基于公开 GitHub 源码及测试证据的静态审查；**未在审查环境独立运行 Windows 双机测试**。  
> 下一轮限定范围：**P0-06a：TURN/ICE 架构与阿里云测试环境准备**。不得把 P0-06a 标记为整个 P0-06 完成。

## 1. P0-05 审查结论

**阶段性通过 P0-05**，可以开始 P0-06a。

已核对的交付事实：

- `TASKS.md` 将 P0-05 标记为 done，Phase 0 父任务为 **4/7（57.1%）**。P0-04a/b 已通过，但 P0-04 父任务因原录屏证据未留存而保留 doing。
- 两轮 30 秒真实双机记录均为：模拟鼠标事件 100 条由 WebView2 发送、Rust DataChannel `OnMessage` 接收 100 条、Rust 排队写出 100 条 ACK、WebView2 `onmessage` 验证收到 100 条 ACK；Ping/Pong 各一条，序号 1–101 连续。
- 两轮均记录视频输入/编码/发送 900 帧，最后一次浏览器回传 893 帧接收/解码/呈现、接收丢包 0。**893 是最后一次采样，不是最终总帧数，也不构成 7 帧丢失证据**。
- `dual-pc-cleanup.json` 留存四轮：接收端停止、发送端停止、关窗、断线。四轮 Rust 控制任务均已 join；拔线显示 `LAN_LEASE_EXPIRED`，源和会话资源释放。
- `artifacts/p005/final-checks/results.json` 记录 10 个构建/测试/检查命令均退出 0；报告称 Rust 43 项、前端 11 项、Edge 2 项。
- 已检查 `crates/hub-webrtc/src/control.rs`、`control_channel.rs`、`signaling.rs`、`session.rs`、`lan.rs` 与 `apps/desktop/src/features/p004/control.ts`。控制通道确实使用 WebRTC SCTP DataChannel，未发现用 TCP/IPC 替代事件和 ACK 的路径。
- 控制协议是**有界的模拟坐标**，不调用 SendInput/SetCursorPos，不是正式远控权限体系。

### 1.1 下一轮必须保留的既有约束

1. P0-04a 本机 loopback、P0-04b LAN video-only、P0-05 LAN video+control 必须独立构建并通过回归。
2. P0-05 的 HMAC 临时码与单轮授权**不能冒充持久设备身份**。日后公网会话正式安全设计仍需 Ed25519/DPAPI、双方设备配对与 session ACL。
3. 不得因为 TURN 需要公网地址而删除 LAN 的 `PinnedRuntime`、SDP 候选/IP/DTLS 检查；必须引入新的、显式隔离的网络策略。
4. 不改变已验证的 DXGI、Media Foundation、RTP/H.264、`control.v1` 消息语义。新网络层适配到现有 PeerConnection。
5. 继续保留“未测就是 null / 未执行不能标 PASS”；不保存桌面视频、坐标、配对码、完整 SDP/ICE 密钥。
6. 之前 P0-04 的录屏验收差异保持公开。若用户决定采用脱敏 stats + 现场目视作为替代证据，先记录用户批准的验收变更；不得 Agent 自行删掉原要求。

### 1.2 风险清单（非已证实漏洞）

- **重要：** 现有 P0-04b 只允许固定 RFC1918 双机地址，TCP 信令还需要同一局域网直接连接；无法直接用于不同 NAT 后的两台机器。公网必须有经认证的远程信令交换和通用 ICE/relay candidate 策略。
- **重要：** 现有 WebRTC `PinnedRuntime` 拒绝不等于指定 peer IP 的 UDP；TURN 使用的是云服务器地址，不能仅把现有对端 IP 改为服务器地址，否则会错误限制真正 ICE/RTCP 媒体路径。应分离 LAN-pinned / TURN-only / direct-preferred 网络策略，并设计安全边界。
- **重要：** TURN 不是“无需凭据的 UDP 转发服务器”。必须限制签发短期凭据，避免开放中继、滥用、DDoS 转发和流量账单失控。
- **待测：** 可靠有序 `control.v1` 目前按 20 Hz/100 条验证，不代表真实游戏鼠标的低延迟适配；高频指针和实际输入属于之后的开发阶段。
- **待测：** P0-05 未测 Windows 11、公网弱网、20 分钟长稳、完整 RustSec/第三方许可审查、硬件丢失后的恢复。

## 2. 下一轮目标：P0-06a（仅准备与技术验证）

**本轮目标**：为“P2P 优先、失败时由阿里云 TURN 中继”建立安全可验证的服务端配置、ICE 抽象、短期凭据和技术决策，不把全功能公网远控一口气做完。

原有云服务器条件（用户此前提供）：Linux、阿里云中国内地、约 50 Mbps 公网出方向带宽。**主机地址、系统发行版、域名、访问权限与已运行服务需开工时实际核对，不得假设已拥有 SSH/控制台权限**。

### 2.1 推荐分层

```text
控制端 Windows (Tauri/WebView2)
  ├─ WSS 信令客户端 ────┐
  ├─ ICE/STUN          │       阿里云 Linux
  └─ ICE/TURN UDP/TCP ├────► 受认证的临时 rendezvous / 信令服务
                       └────► coturn：STUN + TURN 中继
被控端 Windows (Tauri/Rust WebRTC)
  ├─ WSS 信令客户端
  ├─ ICE/STUN
  └─ ICE/TURN UDP/TCP

连接顺序：收集 host/srflx/relay → ICE 检测选择直连 → 失败时 relay。
本轮 P0-06a 不得宣称已完成自动选择；P0-06b 强制 TURN，后续再验证 P2P 优先/回退。
```

- **媒体与 DataChannel 都在 WebRTC DTLS/SRTP/SCTP 上**；WSS 仅传认证消息、SDP/候选与心跳，不转发屏幕像素或鼠标事件。
- TURN 服务器只转发密文媒体；也能观察源/目的 IP 和流量元数据。信令服务器与 coturn 可先在同一台 ECS，但服务与端口分离。
- 初期推荐 coturn 使用 Linux 官方包 + systemd，若服务器无 Docker 不应阻塞；容器方案仅作可选替代。

### 2.2 工作包（严格分步）

**A. 开工审计、ADR 与 API 调研**

1. `git fetch`，核对 main 最新 SHA / 工作树；阅读 `AGENTS.md`、`TASKS.md`、最新 P005 报告、ADR0004/0005 与 `CONTROL_V1.md`。
2. 新增 ADR0006，界定 `LanPinned`、`RelayOnly`、`DirectPreferred` 模式及安全隔离。说明旧 HMAC 码如何过渡到经认证 WSS 信令；不得擅自将临时码当成正式设备身份。
3. 固定 `webrtc/rtc=0.21.1`，阅读**实际锁定版本源码**，确认 Rust 侧 ICE Server、TURN 凭据、relay-only、candidate 事件和 selected candidate pair API。**不要凭旧 webrtc-rs 示例编造接口**。

**B. coturn 配置与服务器预检（可先本地只读）**

4. 交付 `deploy/coturn/turnserver.conf.example`、`deploy/coturn/README.md`、`deploy/coturn/verify.sh`、`deploy/coturn/.env.example`；必须不含任何真实秘钥/IP/域名及用户隐私。
5. 配置 TURN REST API 时间限制凭据：`use-auth-secret` + 非仓库保存的共享密钥 + 唯一 `realm`。短期凭据由受认证的服务端签发，**不把 static-auth-secret 编入客户端**。
6. 建议测试端口：`3478/udp`（STUN/TURN）及按需 `3478/tcp`；中继 UDP 范围可从 `49160–49259` 起步，需与 coturn 配置、Linux 防火墙、阿里云安全组一致。若启用 TURN TLS，另对 `5349/tcp` 配证书；不得仅因为开放 5349 就宣称 TLS 可用。
7. ECS 如使用公网 EIP 映射私网地址，核对 coturn `external-ip` 真实映射；若直接持有公网 IP，不要盲目填写 NAT 配置。不可默认服务器所有网络接口都会被安全允许。
8. 拒绝匿名 TURN Allocate；拒绝内网/回环/链路本地/组播等危险 peer 目的地（配置依目标部署验证）；不启用 `allow-loopback-peers`；设置会话配额、并发数、带宽、日志脱敏、进程最低权限。
9. **未经用户明确授权，不登录、修改或部署到真实阿里云**。无云权限时只交付可审查脚本、验证步骤与阻塞事实，不假报服务上线。

**C. 临时凭据与认证边界**

10. 单独实现可测试的临时 TURN credential minting 纯函数/服务端模块：`username = expiry_unix:session_id`，`credential = base64(HMAC-SHA1(shared_secret, username))`（此为 coturn TURN REST 兼容机制，非通用登录签名）。由**受认证且会话已批准**的请求触发；有效期建议 5–10 分钟，可配置、设置最小/最大 TTL 和时间偏差测试。
11. 服务端密钥仅从受限环境/密钥存储加载；绝不写到日志、Git 或客户端配置。浏览器与 Rust 侧只拿会话短期 `iceServers`，内存用完释放，不写本地长期配置。
12. 未完成正式身份服务前，可定义一个**显式实验级** `rendezvous-probe` 协议/草案，要求双方手动批准、会话随机码+HMAC、明确到期、限流、大小/序号限制以及 SDP/DTLS 指纹完整性保护。不得部署一个任何人输入设备 ID 即可获取 SDP/凭据的公共 API。
13. 交付 `IceConfig {mode, ice_servers, expires_at}` 等**逻辑契约**，具体 Rust/TS API 以锁定库为准。显式区分 `relay-only` 试验和 `direct-preferred` 模式；禁止不经授权将本机 probe 变为网络远控服务。

**D. ICE 网络抽象与最小验证**

14. 在 `hub-webrtc` 中抽象网络策略，保持已有 `NetworkScope::Loopback` 和 `LanScope` 行为不变；未来 relay 模式的 UDP/ICE 候选和收发规则不得通过简单删除校验来实现。
15. 添加独立、默认关闭的 `p006-turn-probe` feature 框架，仅接受有界、短期测试配置。浏览器的 relay-only 模式应要求 `iceTransportPolicy:'relay'`；Rust 侧须采用该版本实际支持的 relay-only 策略，不能仅由 UI 文字表示。
16. 最小验收可以先是**coturn Allocate 与 STUN 检查**，记录非敏感的成功/失败和时延；尚未跨公网传输视频时严禁将 P0-06 标记 done。
17. 在没有服务端授权时，优先完成可编译代码、配置检查、单测、负例测试和可复现 runbook；不运行真实端口扫描或访问不属于用户的服务器。

### 2.3 安全测试要求

必须覆盖：

- 缺少/错误/过期 TURN 凭据：不能 Allocate；正确短期凭据可用。
- 错误 realm、非法端口、超范围 TTL、泄漏密钥的日志样例、凭据重放/跨会话：拒绝或明确说明 coturn REST 的限制与额外的会话层约束。
- 未授权或过期会话不得拿到临时 TURN 票据。
- 旧 loopback/LAN pinned 模式继续拒绝公网候选、application 越权或未知主机。
- 超长 SDP、异常候选、错误 DTLS fingerprint、不受信任的信令服务器替换 SDP 必须失败关闭；不得关闭证书验证。
- 浏览器和 Rust 的实际 selected candidate pair 诊断必须基于真实 ICE 状态；未测 RTT、丢包保持 null。
- 禁止无界日志、明文屏幕录制、控制事件流落盘、自动开启无人值守。

### 2.4 后续阶段（**不在 P0-06a 内宣称完成**）

- **P0-06b：** 两台真实 Windows PC 分属不同网络；经受认证 WSS/SDP 建连；**强制双方仅选择 TURN relay**；至少一个 30 秒 720p30 H.264 视频与 100 条 control.v1 模拟事件均成功；双方通过真实 ICE candidate pair / getStats 证明 relay，不以“配置了 TURN”代替。
- **P0-06c（可选独立里程碑）：** host/srflx 优先的正常 ICE、直连失败自动 TURN、断线/切网重连与成本/时延对照。普通 ICE 候选选择本就存在，但需要真实故障与路径统计，不可凭理论标记。
- **P0-07：** 汇总 Phase 0 技术决策、已知性能瓶颈、备选路线与基线证据差异。

## 3. P0-06a 交付验收口径

| 编号 | 验收要求 | 证据 |
|---|---|---|
| G1 | 实际 HEAD、依赖版本、工作树核对 | Git SHA + 命令结果 |
| G2 | ADR0006 与明确的网络策略契约 | 文档和代码变更 |
| G3 | coturn Linux 示例配置及可审计的手工部署/回滚步骤 | 配置模板 + runbook |
| G4 | 临时凭据算法有已知测试向量与过期/非法输入负例 | 自动测试日志 |
| G5 | 保留原有 P0-04a/b、P0-05 行为 | fmt/clippy/typecheck/构建/回归记录 |
| G6 | 如已获授权，实际云端 coturn Allocate 成功与错误凭据拒绝 | 脱敏命令原始输出、服务端日志摘要；未测则 blocked |
| G7 | 默认构建无公网入口、不启用远程访问 | feature/配置检查 |
| G8 | README/TASKS/测试报告与未测项同步 | 提交记录 |

验收分级：G1–G5/G7/G8 完成可标 `P0-06a implementation-ready`；仅 G6 获得真实云端结果时可标 `P0-06a infrastructure-verified`。**任何情况下都不能仅凭本轮标 P0-06 done。**

## 4. Agent 执行提示词（直接复制）

> 请在 https://github.com/Bravepping/RemoteHub 继续开发。首先同步并确认 main，审查基线 079275d 与最新差异，阅读 AGENTS.md、TASKS.md、CONTROL_V1.md、P0-05 测试报告和本文件。当前 P0-05 已获得证据支持的阶段性验收；本轮仅实现 P0-06a（阿里云 coturn + ICE/凭据/信令技术准备），不要推进键鼠注入、正式 UI、长期设备配对或 P0-06b 的双机 relay 验收。先写 ADR0006，再分波实现独立网络策略、coturn 安全模板、短期凭据纯函数与验证、可复现测试脚本及诊断。遵守既有 P004a/P004b/P005 的权限与性能回归。不要删除 IP/SDP/DTLS 检查来让 TURN“能连”，不要硬编码 TURN 密钥，也不要未经用户授权访问/修改阿里云 ECS/安全组。没有真实云端或跨网条件时，交付可编译的代码、脚本、清晰阻塞，不宣称已部署或 P0-06 通过。完成后更新 README/TASKS、文档、测试原始证据及 commit SHA，停止扩展等待审查。

## 5. 来源与依据

- 本次上传报告：`PHASE0_P005_TEST_REPORT.md`。
- GitHub：`Bravepping/RemoteHub@079275d` 的 `artifacts/p005/dual-pc-full.json`、`dual-pc-repeat.json`、`dual-pc-cleanup.json`、`final-checks/results.json`，以及新控制模块源码。
- coturn 官方文档：https://github.com/coturn/coturn/wiki/turnserver ；示例配置：https://github.com/coturn/coturn/blob/master/examples/etc/turnserver.conf 。
- webrtc-rs v0.21.1 文档：https://docs.rs/webrtc/0.21.1/webrtc/ 。

**注意：** 本文为技术开发任务说明，不是已经部署的服务器配置、已经通过的云中继验收或代码安全保证。
