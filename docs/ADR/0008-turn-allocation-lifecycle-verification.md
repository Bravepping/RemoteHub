# ADR0008：TURN Allocation 生命周期的独立验证

2026-11-21，基线/最新origin/main为ed6ff20022d17186b1c38379aff8c00eee1934d1，工作树干净。用户明确跳过接收端主动停止补测（保留原日志限制），优先生命周期验证，要求独立报告/提交后才进入P006c。用户已授权本轮仅启动remotehub-turn-g6、不自启、不启动WSS；既有UDP端口、配置、业务服务、防火墙及安全组不改，最多两个分配，最长60分钟，结束停止专用服务。

## 决策

复用G6的RFC STUN/TURN工具、server-only短票签发以及Windows TurnProbe。新增操作者诊断脚本：协议Allocate/Refresh(0)与停止续租；原生webrtc0.21.1 PeerConnection gather/close；自动Edge实际PeerConnection gather/close。均无屏幕、视频、实际输入或双机人工操作。Edge是自动化Chromium对照，不能冒充嵌入WebView2；gather建立的真实Allocation不冒充connected媒体会话。

先记录原行为，不为了获得即时释放而停止服务、修改服务器租约、改网络范围或修改下载的依赖源码。库close返回、进程退出、短票到期、Allocation租约到期分开记录。必要的客户端修复须在实际根因证据后决定，不将配置或理论当实测。

进一步补强已连接状态：本机Rust与Edge通过私有stdio broker交换一次data-only SDP，逻辑候选仍仅批准relay范围，双方实际选择relay/relay并交换固定ping/pong，无屏幕/媒体/输入；Edge先关闭，Rust随后关闭。最多两个分配，默认客户端无新IPC入口。gather-only和已连接data-only分别记录，不冒充双机串流；完整SDP只在broker内存传递，禁止输出到交付日志。

## 证据

云端专用临时观察器只看本次STUN Allocate/Refresh控制消息（不抓媒体）和专用turnserver的relay UDP socket。私有原始捕获与端点映射限本轮目录；导出的记录仅有相对测试ID、时间戳、事务散列、实际授予lifetime及socket存在/消失状态，不导出IP、短票、nonce、realm或密钥。客户端和云端分别记录UTC epoch纳秒/单调时间，另提供北京时间；记录观察采样与时钟偏差。

每例必须有baseline、allocation_seen、client_close、close后即时窗口、lease_deadline、deadline后宽限窗口；到期时间按最后一次成功Allocate或正lifetime Refresh响应计算。Refresh(0)请求+对应成功响应lifetime0+该分配socket消失才认即时主动释放；无0释放证据而在租约到期附近消失认超时回收；超过期限+宽限仍存在认残留/失败。无法关联或原始捕获不完整认inconclusive，不填PASS。

协议实验先顺序执行；Windows原生与Edge如并发最多两个分配，按relay端口/控制socket tuple私有关联，不用全局socket数归因。观察期间服务保持同一PID；整体stop仅作最后收尾且不得进入单会话成功判据。原子写本地记录、保存失败轮次、私有输入与可审计输出分离。

## 回退与后续

默认客户端无新网络入口；诊断仅显式操作者参数执行。云端不自启，最长窗口自动停止作为兜底，若触发则未判定测试标中断。结束停止本轮coturn、核实无实验socket、恢复原业务集合状态并保留差异。独立报告和Git提交完成后，再按用户新授权设计P006c，云端新增范围另行提交授权。
