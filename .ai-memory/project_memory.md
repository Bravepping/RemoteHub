# RemoteHub 项目约定

# 当前决议与恢复入口（2026-11-22，P1-01）

用户已审查767ff44并明确接受R06尚未关闭剩余风险、批准Phase0限定退出，正式父项7/7；当前只授权P1-01工程基础，完成后提交等审查。R06技术OPEN_NOT_REPRODUCED/历史根因未知，不把风险接受当修复。P004例外和旧失败/统计限制保留，旧报告不改。

当前入口：docs/PHASE0_EXIT_DECISION.md、P101_TEST_REPORT.md、P101_DESKTOP_CONTRACT.md、DEVELOPMENT_GUIDE.md及ADR0016。pnpm verify:p1固定新证据artifacts/p101：47质量命令、8真实Cargo配置、9隔离测试，82全feature/56默认Rust各1ignored、34前端/3Edge/12Python；截图隔离改动后Edge补测，旧图还原并核对。产品Rust/Vue/权限/锁不改，不新增P1-04命令/Pinia。Foundation/V+ PC/EasyTier仍是未实现、来源/接口/许可未核验的兼容目标。

历史云授权已结束，最后已知停服13:40:16来自7fde54e证据，本轮未实时查询。当前不启阿里云、不采集真实桌面、不输入注入，P1-02至06/兼容运行原型未开工。以下是历轮历史记忆，旧待批准或运行授权只适用于当时，不覆盖本段和最新TASKS/用户指令。

2026-11-21 首次工程。Windows / Tauri v2 / Vue 3 / Rust，遵循原交付设计；Phase 0 真实媒体优先。不可用时报告失败，不显示假设备/RTT/FPS。不实现未授权后台监控、权限提升、凭据采集。

## Decision Record

首轮使用有限本机实验，理由与备选见 ADR 0001。远控身份未完成时不开放网络入口；本机显式同意仅作用本次本机采集。完成配对、签名与 Session ACL 后方可重新评估远程集成。

P0-03 采用真实 MF MFT，AMD 硬件异步事件驱动与 Microsoft 软件同步回退，CPU 缩放/NV12；见 ADR 0002。真实 10 秒运动样本通过。视频时钟必须在 GPU 计数器初始化之后开始；允许有界短时落后复用最近真实 NV12。未来有实测瓶颈再评估 GPU 纹理路径。

P0-04a 采用 webrtc/rtc 0.21.1 的新异步 driver API + StaticRTP，3 AU队列，Windows线程持有COM，90kHz以原MF PTS整数换算。只绑定127 UDP，拒绝外机SDP；原生MessageBox本轮授权；feature默认关闭。真实WebView2 157.0.4341.60已通过450/900帧视频与主动停止/关窗。P004父任务仍需双机。见ADR0003/P004_FORMAT。

P0-04b新增默认关闭p004-lan、短时指定RFC1918两端scope和原生双向IP socket过滤；每轮双端原生确认及随机码HMAC认证信令，120秒等待、3秒心跳租约，复用标准WebRTC媒体。v2双机900帧全接收/解码/呈现与两端停止/接收端关窗/拔线通过。基线录屏未留存（约定不保存画面），P004父项仍doing。见ADR0004/P004B报告。此实验授权不替代正式身份/ACL。

## Convention Capture

P0-05在同一视频PC上新增默认关闭control.v1可靠有序模拟事件，ADR0005/CONTROL_V1。P005原生双端确认和HMAC信令绑定本轮controlSession，严格一video一application同BUNDLE，原P004a/04b仍video-only。DataChannel poll任务独立有界且源worker负责close/join；模拟坐标不调用输入API。两轮双机100/100 ACK与四项清理通过；43 Rust/11前端/2Edge。只完成P005，停止等审查，不进P006/P1。

Rust fmt、TS strict、Vue/TS Prettier。模块依赖 Tauri -> hub-capture-win -> windows。输出样本放 artifacts/private 并排除 Git/打包。正确：测试返回值可来自 fixture；错误：产品显示固定假测量。每次以 artifacts 和当前实测结果更新 TASKS，不继承未实测的成功。

## Known Issues

P0-06a遵循ADR0006，5e05c09先交implementation-ready；后用户单独授权G6真实云STUN/Allocate/凭据及peer拒绝通过，升infrastructure-verified，P006父项仍待独立双机relay验收/Phase0仍4/7。新server-only issuer不进desktop，feature默认关闭且无公网桌面入口；原媒体/控制/锁不变，17组回归+54 Rust/13前端/3Edge/7Python通过。0.21.1原生TURN仅UDP；WSS、长期身份/ACL、DirectPreferred未实施。coturn4.6.1错realm Allocate实际为437且原因明确，不能泛化437为通过；REST票仍可复用/无即时单票撤销。G6测试unit已停、不自启，26旧服务/2容器保持，nft去计数后策略相同。实际地址/秘密/原日志不进Git；区分事务耗时和未测ICE RTT。

DataChannel::poll还会产生buffered事件，测试必须等OnMessage，不能把任意poll返回当ACK。PC销毁时OnClose计数可能为0，close_requested和task_joined单独记录，不补造事件。B最后回传样本早于本地终态，不能按发送900减893宣称丢帧。PowerShell合并stderr会包装NativeCommandError，Node spawn捕获退出码/UTF8输出更适合保存检查证据。

DXGI 目前 CPU staging、主屏、无光标合成、旋转拒绝、access lost 终止。Windows 10 本机通过，Windows 11 未测。PowerShell 5 脚本使用 ASCII，避免无 BOM UTF-8 中文解析问题。此机交互 PATH 引号异常，在本次构建进程内修复，不修改全局设置。

P003 raw Annex B 的 ffprobe r_frame_rate 是估计值，本机报告 60/1，不能覆盖实际 MF 30Hz PTS；MP4 已逐包检查 30fps/10 秒且解码帧与原始 AU 一致。AMD GPU 计数器叫 Video Codec 0，不能只看通用 VideoEncode 名称。WinForms 运动源必须 DPI aware / AutoScaleMode None，使用真实物理边界验证颜色。P0-03 不等于 WebRTC/浏览器或公网互通。

## Glossary

AMD MFT SPS 42041f 的 constraint_set5 对 profile66 不适用；标准要求解码器忽略，不能逐字节等同42001f或暗改SPS。初始严格比较导致真实失败，已加回归。浏览器过滤host/mDNS候选后的空candidate offer可在本机经Rust127答案形成prflx，实测成功不等于跨机穿透。stop后异步授权/统计结果迟到必须撤销/忽略。私有真实MF测试的环境变量需绝对目录；普通cargo test保留ignored，不假报通过。未经有效测量计数不得把库RTT默认0当作实测0ms。

webrtc0.21.1真实应答即使有rtcp-mux仍附component2 RTCP host候选。只在answer+rtcp-mux时兼容同一指定IP/UDP的component2；offer仍仅component1。认证控制连接回传的统计每秒采样，断线后最后解码数不等于停止瞬间最终数；不要用发送与最后回传差值声称丢帧。原生错误必须返回真实固定错误码，不能统一用oneshot掉线吞成LAN_SIGNAL_FAILED。

观察 FPS：收到 DXGI 桌面更新的计数/实验总秒数，不能等同视频编码/传输/解码 FPS。首轮交付完成不等于 Phase 0 退出条件满足。
