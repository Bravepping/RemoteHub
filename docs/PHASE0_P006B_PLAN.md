# P006b实施计划

基线4507fa9干净、main已fetch一致，已读AGENTS/TASKS/设计、G6、ADR0006/原控制契约。用户确认热点/公网IP+端口/服务启动授权。先提交ADR0007与契约，再实现。

1. 共享严格协议与实验Axum WSS服务：角色邀请/有界房间、server-only TURN签发、TLS/IP证书、限流/失效清理。出口：单元/TLS/WebSocket集成与负例；部署前核对专用端口/既有26服务2容器，云构建工具隔离。
2. 复用Session/RelayRuntime扩展显式relay入口、真实native pair和帧传输前的路径守卫；新增客户端会话信令/租约/报告。出口：旧scope/媒体/control语义不变，本机真实协议/构造与负例。
3. Tauri原生双方确认、默认关闭feature、Vue实验面板/真实浏览器relay、100事件/ACK与统计。出口：新页面驱动与旧UI回归，拒绝未批准/迟到结果/路径错误，关窗join。
4. 服务器隔离WSS与现有TURN开启；构建新的Windows双机私有实验包，包内含限时邀请/CA，源码交付不含秘密；用户移动新包到热点接收端后每轮亲自确认，先30秒再清理轮次。出口：真实视频、100/100、双端relay证据及停止/关窗/断网；未执行项不补造。
5. 完整fmt/clippy/typecheck/build/unit/browser及旧feature独立检查；报告、README/TASKS/交接、GitHub提交与包SHA核验，停止服务/不自启后等待审查。不实施P006c/P1或实际输入。

A1–A10按上文需求映射、出口/风险/证据/回退/文件影响/输入与人工配置/检查点落实；B2/3/5/6/9/10命中，DB/破坏迁移N/A；≥5文件每波checkpoint。主影响路径：hub-probe-wire→Axum server与hub-webrtc relay→Tauri p006→Vue；原Session是唯一媒体实现，不复制DXGI/MF/RTP。新增依赖固定版本并审许可/实际API，不盲升现有WebRTC；全局网络/证书配置不改，所有操作者同样检查，无管理员豁免。每波记录handoff/daily；失败回到本波并保留旧基线，不覆盖用户未确认成果。
