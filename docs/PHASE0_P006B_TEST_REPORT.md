# P0-06b 跨网络 TURN 视频实验报告

后续用户决定（2026-11-21）：接收端主动停止已在真实双机手动确认正常，明确要求跳过补测、保留原报告独立日志限制，不再要求重复操作。下文为原交付时的历史证据和状态，不追填新的程序日志；该项不再作为后续推进阻塞。TURN Allocation生命周期本轮另立ADR0008与独立报告，不用停止整体服务替代逐分配释放验证。

2026-11-21，状态：**跨网络双机视频/控制主链路及主要清理场景实测通过；接收端单独提前停止的独立证据不足，完整验收保持doing，等待审查**。下列双机结果来自本轮真实Windows程序统计和用户现场确认，未用本机WSS、两个TURN分配或旧P005结果替代。未进入P006c、实际输入或Phase1。

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

## 双机验收（已执行及证据边界）

| 项目 | 当前状态 |
|---|---|
| 宽带发送端与蜂窝热点接收端30秒720p30真实视频 | PASS；两轮完整实验分别900帧输入/编码/发送/接收/解码/呈现，1280×720，接收丢包0、冻结0；用户确认变化画面 |
| 两端真实relay/relay UDP selected pair | PASS；全部5轮Rust实际选中路径与WebView2 getStats回传均匹配批准范围；不是只读取配置 |
| B发送100 / Rust接收100 / ACK写入100 / B确认100；连续序号与异常0 | PASS；5轮各100条，ping/pong各1，双向序号1..101；pending/anomalies/rejected/error均0 |
| 发送端主动停止 | PASS；用户按指定顺序操作，round-03在194帧/约6.46秒提前结束，源与控制任务回收 |
| 接收端播放中关窗 | PASS（源端证据+用户现场确认）；round-04在196帧/约6.55秒提前结束，源端观察到DataChannel OnClose，任务join和源回收通过；未收取B本地window-close文件 |
| 接收端热点断网 | PASS（源端证据+用户现场确认）；round-05在244帧/约8.12秒提前结束，资源回收；记录error为null，不虚构为lease超时错误 |
| 接收端单独主动停止 | 用户确认正常；独立提前停止证据不足。按顺序对应round-02仍900帧完整结束。请求补测后用户回复正常，但未找到新增源端报告，因此不标独立PASS |
| 无实际键鼠输入 | 本轮运行仍仅control.v1模拟消息，源码及自动检查无输入API调用；不保存鼠标坐标或画面 |

证据位于 `artifacts/p006b/two-pc`：round-01..05-native.json为白名单字段检查后的真实原生终态与认证接收端快照，带原文件SHA256；statistics.json为计数汇总，user-observations.json单独记录现场反馈。两个完整轮次的媒体时钟约29992/29986ms。三轮提前结束时最后回传解码数183/177/233，早于源停止的194/196/244帧，不能将差值当作丢帧。

全部5轮源端resources_released及control.task_joined为true。源端final文件中的browserClosed只属于发送端页面，不能冒充接收端WebView2的独立回收证明；B本地final/window-close未收取，现场关闭/回收由用户确认。真实OnError未专门触发，该回调的自动测试不能冒充物理网络事件。

## 本轮云端收尾（已执行）

北京时间22:17:25，已停止remotehub-p006b与remotehub-turn-g6，两者inactive/disabled/MainPID=0，取消临时到期timer。TCP8443、UDP3478、relay范围和实验进程socket均为0，TURN运行目录已回收，SSH连接已关闭。证据见 `artifacts/p006b/cloud/shutdown.json`。基线26项运行服务仍在，两个容器名称与运行状态保持；另观察到fwupd.service运行，未操作该服务、未核实启动原因，不声称前后服务集合完全一致。用户增加的安全组规则未由Agent删除，服务已停止且该端口无监听。

停止服务前仍观察到1个relay范围socket，尚未对齐具体分配/会话，**未证明每次PeerConnection close都会立即完成TURN Refresh(0)**；停止专用TURN后清零。浏览器/原生资源释放与云端分配即时释放分开报告，后者作为后续审查与生命周期诊断项。

P006b视频/控制及主要清理功能有真实证据，接收端独立提前停止专项不标PASS；P006b/P006父项保持doing。P004录屏差异及Phase0退出条件继续保留，停止扩展，等待审查。

## 限制与下一步

本轮无正式设备身份/长期ACL、直连优先/自动回退/重连、Windows11、长稳、完整RustSec审计或安全审计。原生TURN仅UDP；短票可在有效期内复用，不提供单票即时撤销。SDK/JavaScript内部凭据副本不承诺完全清零。信令4秒租约有意断线即终止；本轮不做恢复。下一阶段仅建议审查接收端主动停止证据、逐会话TURN分配释放，以及是否另行批准P006c；本轮不实施这些扩展。
