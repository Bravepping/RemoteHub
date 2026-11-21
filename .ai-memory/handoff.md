# P0-06a 准备检查点

- 已完成带证据：开工main=079275d且干净；ADR0006/ICE与rendezvous契约先提交3a8f67f。P006a implementation-ready：server-only短票据签发、隔离RelayOnly策略/socket、受限SDP与离线信令MAC、真实API诊断框架/显式操作者CLI、TS relay工厂、coturn模板/校验/渲染及Linux人工部署回退。17组最终检查达到预期：54 Rust/13前端/3Edge/3Python，default原生build与四feature独立check通过；私有MF1项ignored。source-audit证明533个既有锁包不变、issuer不进desktop、旧采集/编码/控制/入口diff为空、无输入API。没有新云或双机实测。
- 相关文件：docs/PHASE0_P006A_TEST_REPORT.md、ADR0006/P006_ICE_CONTRACT/PHASE0_P006A_PLAN；crates/hub-turn-credentials；hub-webrtc ice_policy/relay_runtime/turn_probe/rendezvous_probe/signaling/bin；TS features/p006；deploy/coturn；scripts/p006a-checks.mjs；artifacts/p006a。默认关闭feature，无新Tauri命令/公网页面；默认CLI拒绝、占位配置在建PC前拒绝。最终Git SHA/交付包以会话与包内BUILD_INFO为准。
- 未完成：G6真实云端Allocate/STUN/错误或过期凭据拒绝blocked：没有核实主机/系统/权限，也没有云登录/部署/测试明确授权，不SSH不探测不部署。G1–5/G7/G8完成仅implementation-ready；P006父项blocked，Phase0仍4/7，P004录屏差异保留。本轮停止等待审查，不进P006b/c/P1。获得目标资料和独立授权后才安排G6。
- 风险/发现：0.21.1原生TURN仅UDP；coturn Wiki自述过时，模板未由目标包实际加载。REST票据可复用/无单票即时撤销/realm不在HMAC中，issuer错realm拒绝不代替coturn负例；临时信令非长期身份，WSS传输未实现。SDK/JS秘密副本不保证清零；CLI私有JSON短期且应及时销毁。Windows bash指定实际Python，避免Store别名。新增负例的unwrap_err要求Debug已改直接匹配，完整重跑通过；Node固定pnpm shim提示如实保留。

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
