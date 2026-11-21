# P0-06b 跨网络 TURN 视频实验报告

2026-11-21，状态：**实现与自动测试完成，等待两台真实 Windows PC 验收**。不能将本报告的本机 WSS、单机两个TURN分配或旧P005双机结果当成本轮双机视频通过。P006b保持doing；不进入P006c、实际输入或Phase1。

## 基线与实现

本轮ADR/契约先提交 `6f694de`，开发前基线 `4507fa9`；续接时保留既有未提交实现，重新fetch确认origin/main仍为4507fa9，没有覆盖远端更新。沿用Tauri/Vue、webrtc/rtc0.21.1与原DXGI/MF/H264/RTP、control.v1。默认关闭p006-relay；旧Loopback/LanPinned保持独立，旧本机gather预算仍4秒，新relay才10秒。

新增hub-probe-wire与隔离的Axum WSS实验服务；复用server-only TURN issuer。角色邀请、双向原生同意、临时配对码、HMAC、严格连续序号、期限、大小/速率/房间/连接上限与退休房间拒绝；不提供长期身份或无人值守访问。TLS使用应用专用CA/IP SAN，无系统证书安装或忽略验证。双方票据使用同一签发时间，避免跨秒产生不同会话期限。邀请最多2小时；短票会话期限180秒，客户端验票60..600秒，并受批准期限约束。

Rust与WebView2强制relay-only，同一视频PC承载可靠有序control.v1。两端真实selected pair必须为批准IP/端口的relay/relay UDP；未知保持null。Rust在核实路径前拒绝控制消息，浏览器在核实路径并实际播放视频后才发送100条模拟事件。Rust仅脱敏计数/ACK，没有调用Windows输入API。WSS只传SDP、心跳、统计与停止，不代传控制事件或ACK。

## 自动测试（已执行）

原始输出与退出码：`artifacts/p006b/automatic`。13组检查通过：前端typecheck/build/lint、16个Vitest用例、3个真实Edge测试、fmt、默认与all-feature严格clippy、all-feature Rust测试、旧p005-control桌面测试、p004-probe与p004-lan独立编译，以及显式custom-protocol,p006-relay优化版Windows构建。复现命令为 `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p006b-checks.ps1`，不读取私有配置、不连接云端或采集屏幕。

全feature Rust为60项通过；另旧桌面feature为3项通过。私有真实MF样本测试1项仍明确ignored，本轮没有新增采集或以fixture替代硬件结果。真实本机TLS/WSS集成测试验证信任链、错误CA/名称/邀请/Origin、两角色入房、转发和重放关闭；协议负例覆盖篡改、错误上下文、未知字段、失效、超大消息、反射、序号与限流。原有真实SCTP与本机UDP边界测试继续通过。Edge测试只证明浏览器构造真实PC/可靠通道与relay配置，不证明本轮媒体传输。

## 云端基础验证（已执行）

用户明确授权同一ECS启动服务，已核对既有服务/两个业务容器与专用端口。服务端在独立目录用锁定、checksum校验的vendor依赖离线构建；普通仓库源码/客户端不依赖该vendor运行。WSS专用非root服务，TURN沿用G6专用非root单元，均不启用自启；未操作其他业务或更改Linux防火墙。用户自行放行TCP8443安全组规则后公网WSS通过。

Ubuntu服务端实际执行4项Rust测试与release构建通过；初次Linux测试暴露fixture未等sender Waiting即让receiver加入的时序竞争，已等待该协议事件后重测通过，保留首次失败摘要。最终部署与源码一致的服务端二进制。两项服务设有本轮邀请到期后3秒内的临时自动停止任务；完成测试仍需主动收尾核查。

`artifacts/p006b/cloud/wss-before-ingress.json`保留首次公网失败：服务器本机TLS正常，但网卡未收到该端口连接包；未将负例网络失败当作认证拒绝。`wss.json`为放行后的实际公网TLS、错误CA/邀请拒绝、双方入房与短票结果。PC与服务器观察到约2秒时钟差，TTL校验按既有30秒偏差边界解释。

`turn-two-allocations.json`：**同一Windows电脑的两个独立UDP TURN分配**，两次Allocate与两次CreatePermission完整性校验通过，两个方向各一条随机测试载荷真实转发，双方Refresh释放成功。此项不含视频、不是WebRTC selected pair、更不是两台电脑或热点验收。

登录密码只进入SSH连接进程内存。TURN共享密钥和TLS私钥留服务器；客户端私有包仅含CA公证书及限时接入邀请，不进入Git。默认不保存画面、坐标、配对码、完整SDP或长期凭据。

## 双机验收（待执行）

| 项目 | 当前状态 |
|---|---|
| 宽带发送端与蜂窝热点接收端30秒720p30真实视频 | 未执行 |
| 两端真实relay/relay UDP selected pair | 未执行 |
| B发送100 / Rust接收100 / ACK写入100 / B确认100；连续序号与异常0 | 未执行 |
| 接收端停止、发送端停止、接收端播放中关窗、热点断网清理 | 未执行 |
| 无实际键鼠输入 | 源码检查与既有自动协议测试通过；双机现场尚待确认 |

步骤见 `P006B_RUNBOOK.md`。真实统计和用户观察将作为独立证据追加，未执行项不得标PASS。当前保留P004录屏差异，P006父任务及Phase0退出条件尚未完成。

## 限制与下一步

本轮无正式设备身份/长期ACL、直连优先/自动回退/重连、Windows11、长稳、完整RustSec审计或安全审计。原生TURN仅UDP；短票可在有效期内复用，不提供单票即时撤销。SDK/JavaScript内部凭据副本不承诺完全清零。信令4秒租约有意断线即终止；本轮不做恢复。结束双机测试后停止本次WSS/TURN、核实资源释放，再提交最终报告等待审查。
