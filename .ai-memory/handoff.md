# P006c 首步交付检查点

- 生命周期独立代码/报告已推main 88f1edc，用户随后明确授权P006c自动云测试（最多40分钟/2分配/只既有coturn/WSS关闭/无配置FW SG业务改动）。ADR0009先提交b8b0214，新增默认关闭p006-direct-probe；尚未接现有视频Session/WebView2/业务信令，不标P006c或P006父项done。
- 2026-11-22北京时间07:13:41至07:17:31实际启动专用coturn，单PID324151；同一物理Windows机两个真实原生PC、无媒体/输入，ICE All：正常host/host 100/100/100、直连进程内阻断自动relay/relay 100/100/100（34.873秒）、TURN进程内阻断仍host/host 100/100/100（22.051秒）、两路阻断35.014秒明确DIRECT_CONNECT_TIMEOUT、0消息/no pair。双方正常/回退均保留host/srflx/relay各1，不改relay-only。故障真实丢包仅进程内，不是物理NAT/防火墙试验。
- 4实际云分配各授予600秒，Refresh0成功回应+1.140至1.394秒首次观察socket消失；同PID最多2，逐轮0再继续。后两轮4票无成功Allocate；本轮即时释放后提前停止，原租约deadline检查点null（上一轮6例完整期限证据独立保留）。最终shutdown证实停服务前socket0、coturn inactive/disabled、WSS始终停、timer取消、原配置/unit/FW策略/业务容器与服务集合不变，原始控制捕获/短票清理，SSH关闭。
- 报告docs/PHASE0_P006C_TEST_REPORT.md，脱敏artifacts/p006c（真实云、local与offline/development分开）；24项最终检查/66 Rust、旧桌面3、16前端、3Edge、11Python；MF1ignored。最终审查后加EOF失败保护与真实闭PC回归，保留云前二进制/sourcehash及最终差异，未为此重启服务。首个component2拒绝及闭通道fixture类型/handler错误留development，修复后重跑。
- 约束：用户明确跳过接收端主动停止补测，保留原独立日志限制。P006c完整视频/control.v1与WebView2接入、真实物理双机/NAT、trickle/restart/切网恢复未做，SDP外prflx仍拒绝。下一步复用旧视频链路接入，不进入输入或P1。新云启动仍需新范围授权。
- 运维资料外部清单写入首次因自动审批工作区额度不足未执行，未绕过；后续Git上传审批恢复后，通过同一审批通道成功更新生命周期及P006c的真实结论/最终停用状态，未保存凭据。仓库报告/交接证据完整保留，Git最终SHA以提交/对话为准，避免自引用。

# TURN生命周期 / P006c开工检查点（历史）

- 用户明确接受接收端主动停止现场确认并要求跳过补测，保留原900帧独立提前停止证据限制，不再要求重复双机操作。P006b据此收尾，P006父项仍doing，P006c需后续开发。
- 开工main ed6ff20干净，先提交ADR0008 dde4649。用户单独批准既有coturn最长60分钟/最多2分配/WSS关闭/无配置防火墙安全组业务修改。2026-11-21北京时间22:39至23:06真实完成6例：协议主动释放、停止续租、Windows原生/Edge gather后close、已连接Rust/Edge data-only双方close。5例Refresh0请求/成功回应+1.110至1.464秒首次观察socket消失；实际租约均600秒，不续租到期后0.318秒消失。原始deadline+15秒均无残留，同PID286888、最多2分配；判定结束前socket已0，未用服务停止冒充单例释放。
- docs/TURN_ALLOCATION_LIFECYCLE_TEST_REPORT.md、artifacts/turn-lifecycle为真实证据；19组检查符合预期，61 Rust/16前端/3Edge/11Python，私有MF1ignored。Rust与Edge在同一物理Windows机器真实SCTP ping/pong、双方relay/relay；无采集，无新双机/WebView2视频负载实测，不能扩大解释。本轮未归因上轮遗留1socket。
- 23:05:59已停专用coturn且disabled，WSS始终停用，timer取消，原始控制捕获和短票清理；shutdown.json证明配置/unit/防火墙hash、业务容器启动时间及服务集合不变。后续云端再次启动或变更需提交新具体范围取得授权。
- 下一步：先独立提交并推送生命周期结果，再写P006c ADR/契约、实现P2P优先及TURN自动回退，尽可能无画面自动Windows测试；不能将尚未执行的真实双机验收标PASS。不进入输入注入或Phase1。

# P0-06b 双机收尾检查点（历史）

- 实现提交977f97fe3aac287283fa73da93cf60b9261e965b已推GitHub；源码/二进制本轮收尾不再修改。ADR先6f694de，基线4507fa9，沿用同一视频PC与control.v1，未做P006c/输入/P1。
- 真实宽带+蜂窝热点双机5轮：两轮30秒各900帧完整发送/接收/解码/呈现，720p、lost0/freeze0；每轮真实双方relay/relay UDP、100条发送/接收/ACK/确认、1..101连续序号、异常0。三轮提前194/196/244帧，按用户给定顺序对应发送端停止/接收端关窗/热点断网，源/控制join回收。首个接收端停止尝试仍900帧；补测请求后用户称正常，无新增记录，只记现场确认，不标独立PASS。B本地final/window-close未收取，A browserClosed不冒充B证明。
- 自动13组PASS：60 Rust+旧桌面3、16前端、3Edge；私有MF1 ignored。Ubuntu WSS4测试/build、公网TLS/错误CA/邀请拒绝与本机两分配双向TURN通过，和双机结果分开。artifacts/p006b/two-pc为脱敏实测/原报告SHA/用户观察；cloud/shutdown为收尾。
- 北京时间2026-11-21 22:17:25已停remotehub-p006b与remotehub-turn-g6、均inactive/disabled/MainPID0；8443/3478/relay范围/实验进程socket均0，TURN运行目录回收，取消临时timer，SSH关闭。基线26服务仍运行，两个容器保持；另fwupd运行（未操作/原因未查），不称集合完全相同。停服务前尚1个relay范围socket，未归属每会话，逐PC即时Refresh(0)未证实。秘密仅云端受限/本机忽略目录；临时TURN票已从本机删除。
- 状态：双机主链路及主要清理通过，P006b/P006完整验收doing（上述独立证据限制）；P004录屏差异保留，Phase0正式仍4/7。交付报告/脱敏证据，停止扩展等待审查。原私有实验包限时邀请不再用于在线实验；最终Git/归档SHA见BUILD_INFO。

# P0-06a / G6 交付检查点

- 已完成带证据：准备基线079275d、ADR先行3a8f67f、实现5e05c09。用户后续单独授权指定ECS的G6，开工fetch仍5e05c09干净；实际Ubuntu24.04/coturn4.6.1、非root专用unit与EIP映射，Windows原生两轮各1 relay候选/gather81和92ms/close成功，真实UDP11项全部通过（无/错/过期401、错realm437专用原因、五危险peer403），Refresh释放。P006a现infrastructure-verified。完整17组回归达预期，54 Rust/13前端/3Edge/3配置Python+4协议Python，私有MF1ignored。apps/crates/锁和实测CLI二进制与5e05c09不变。
- 相关文件：docs/PHASE0_G6_TEST_REPORT/PLAN、历史P006a报告/ADR0006；scripts/g6_turn_wire.py及4项离线测试/RFC许可；deploy/coturn单元pidfile/userdb专用/run路径；artifacts/p006a/g6保留真实结果/首轮断言差异/本轮全回归/收尾摘要。服务器本次新增remotehub-turn-g6.service、/etc/remotehub-turn-g6和/opt/remotehub-turn-g6-tools，实际地址/秘密仅服务端私有或本地忽略目录，登录密码未保存。最终SHA/包以BUILD_INFO为准。
- 未完成：P006父项仍待不同网络双机视频/control真实relay与pair证据，不能以Allocate代替；Phase0仍4/7、P004录屏差异不改。测试专用及默认coturn均inactive/disabled，短票本机文件已移除，运行时socket与/run目录回收；保留已安装包/受限配置，后续需另轮授权启动。既有26服务/2容器仍运行，INPUT/OUTPUT和归一化nft策略不变，没有改安全组/防火墙。完成交付后停等审查，不进P006b/c/P1。
- 风险/发现：0.21.1原生TURN仅UDP；P006b/WSS/长期身份未实现，REST票据可复用且无单票即时撤销。G6第一次错realm返回437而断言期望401/441，保留首轮；核对coturn4.6.1源码Allocate分支确为437+专用原因，新增严格回归后真实重测11/11。nft原始hash因40处计数变化，去counter数值后结构完全相同。STUN/Allocate事务耗时不是ICE RTT；未建PC对端故pair/RTT/loss仍null。目标4.6.1 help已验证pidfile/userdb，单元不写受保护默认目录；其它目标仍须独立核对。

## P0-05 基线

- 已完成带证据：fetch确认main=b914942且开工工作树干净；先提交ADR0005/CONTROL_V1（f68e42e），再实现。默认关闭p005-control，复用同一视频PC。两台物理Windows10有线电脑两轮30秒视频+100事件，A接收/ACK写入/B发送/确认均100，连续1..101序号、ping/pong1、异常0；源900帧，B最后回传视频893帧、720p、丢包0（不伪称最终900）。四种清理：B停止188帧/A停止207/B关窗210/B拔线279（LAN_LEASE_EXPIRED），全部控制join/源/PC回收。B停止/关窗/拔线有A真实OnClose1。43 Rust、11前端、2Edge和全部10最终命令exit0；本机真实SCTP负例与双机分开，私有MF1项ignored。
- 相关文件：control.rs/control_channel.rs；signaling/session/lan；Tauri p004_lan/feature/main；Vue control.ts/lan.ts/LanVideoProbe；scripts/p005.ps1。旧P004a/04b构造器仍video-only，P005才接受一video一application的同BUNDLE SDP。TCP只运认证上下文、心跳和统计，无控制事件/ACK代理。源码不含SendInput调用。
- 未完成/限制：P005范围已验收，收尾Git与归档标识见交付BUILD_INFO/会话结果，完成后停止等待审查，不进P006/P1。B终态本地final未收取，其界面/回收由用户确认；B运行中真实ACK统计由A认证连接保存。没有物理DataChannel OnError，error回调覆盖是自动fixture，不伪造。P004基线录屏差异继续保留；无实际输入、长稳、Win11或独立安全审计。
- 发现/修正：webrtc fixture需add_track（不采集）才能sendonly；fixture候选先模拟前端component1过滤；真实DataChannel poll还产生buffered事件，集成测试需等待OnMessage而非把任意事件当ACK。H264匹配限video节、每节独立mid。前端测试RTCErrorEvent类型已修。PowerShell把stderr包装NativeCommandError，最终用Node捕获UTF8原始stdout/stderr和退出码避免误判；实际最终10命令全0。真实P004b基线保留，不把旧记录当P005新实测。

## P0-04b 基线

- 已完成带证据：基线5fb0867，ADR先提交dabb5fa。两台真实Windows10有线PC，v2完整30秒900帧输入/编码/发送/接收/解码/呈现全部一致，1280×720，8365包、接收丢包0，实际selected host/host UDP指定对端匹配。两端停止114/144帧、接收端关窗125帧、拔线187帧（LAN_LEASE_EXPIRED）提前停止，发送端源/会话释放。用户亲自确认并操作，回报四项均回收。36 Rust(all features)、8前端、1Edge、verify/clippy通过，1私有MF测试保持ignored。
- 相关文件：ADR0004、hub-webrtc scope/pinned_runtime/lan_wire/lan、Tauri p004_lan/feature/命令ACL、Vue LanVideoProbe/lan.ts、p004b脚本/限定规则脚本、P004B_RUNBOOK和PHASE0_P004B_TEST_REPORT。脱敏实际统计artifacts/p004b。私有目录不得打包；具体IP/配对码/原SDP不进Git。
- 未完成：P004b功能通过，P004基线所列录屏未留存（本轮原生授权明确不保存画面），父任务保留doing/正式计数3/7。接收端本地final/window-close未收取，接收端关闭/回收由用户确认；发送端持有真实接收端指标回传，不能把A的browserClosed当B证明。P005 DataChannel、P006 TURN、公网、长稳、Win11未做。Git/归档信息以交付BUILD_INFO与会话结果为准，避免将最终提交哈希自引用写入Git。
- 风险/复盘：首轮配对成功但0帧，B拒绝库真实应答component2，A SIGNAL_DISCONNECTED。v2仅在应答有rtcp-mux时接受同指定IP/UDP的component2，真实库应答回归先失败后通过；B实际错误直接传出，避免LAN_SIGNAL_FAILED掩盖根因。停止/断线最后回传计数落后源停止，不当作丢帧；RTT无有效测量为null。防火墙脚本只解析/Inspect，未找到其专属规则组，不声称安装/移除已测；没有替用户修改防火墙。

## P0-04a 基线

- 已完成带证据：基线 7fcbb51，ADR 先行 6d38af3。用户亲自批准并操作原生 WebView2，五轮15秒450帧 + 30秒900帧全接收/解码/呈现、丢包0；主动停止47帧、关窗33帧清理通过，最终窗口/进程消失。artifacts/p004a 原始脱敏计数、pnpm verify、25 Rust(all features)、5前端、1Edge、显式1项私有真实MF AU测试均通过。
- 相关文件：stream.rs（Windows线程、不写视频）、hub-webrtc/session.rs/video.rs/signaling.rs、Tauri p004.rs/feature/ACL、Vue features/p004、scripts/p004.ps1、P004_FORMAT.md、PHASE0_P004_TEST_REPORT.md、ADR0003。默认 feature 关闭；显式脚本优化构建到 target/probe。视频统计仅存程序目录 artifacts/private/p004。
- 未完成：P0-04a done，原P0-04仍doing；P0-05/06未做，不进入P1。下一步双机范围与授权设计。剩余本轮流程是最终 Git 提交/上传及交付归档核验，最终SHA/包见后续会话记录。
- 已知风险/限制：AMD真实SPS42041f，按标准忽略对Baseline不适用constraint5做兼容，不改字节。browser无candidate offer由127答案建立prflx已实测；不能推广所有环境。原生同意需用户亲自操作。无Win11/长稳/物理GPU丢失/PLI端到端/RustSec；GUI OS退出码与播放截图未单独留存，报告明确。私有MF样本环境变量必须绝对路径，Cargo测试cwd是crate目录。

## P0-03 基线

- 已完成（带证据）：P0-01、P0-02 保留；P0-03 AMDh264Encoder 10 秒/300 帧/720p30/零丢 tick，标准解码、八色条、PTS、GOP 通过；软件 2 秒/60 帧通过。17 Rust / 3 前端 / 1 浏览器测试。证据 artifacts/p003/ 和 docs/PHASE0_P003_TEST_REPORT.md。
- 相关文件：crates/hub-encode-win/{src/mf.rs,src/probe.rs,src/h264.rs,src/color.rs}；docs/P003_FORMAT.md；ADR 0002；TASKS.md。视频样本仅在 artifacts/private/p003-accepted，不能提交/上传。
- 未完成：P0-04 WebRTC 视频、P0-05 DataChannel、P0-06 TURN、P0-07 全阶段结论。下一轮先做有界本机受授权 WebRTC -> WebView2；不进入 P1。恢复验证：设置 REMOTEHUB_VERIFY_DIR 到新证据目录，然后 pnpm verify。
- 阻塞/限制：真实硬件丢失与 Windows 11 未测；raw ffprobe 速率估计不代表实际 FPS，MF PTS 和 MP4 已验证 30Hz。远程身份/配对/签名/ACL 尚未实现，不要把本机 consent 当作远程授权；两机/TURN 出口仍缺第二台 PC 与节点。
