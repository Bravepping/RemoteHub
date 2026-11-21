# ADR0005：P0-05 在既有视频连接中验证模拟控制

2026-11-21，基线b914942；状态：实施中，真实双机未测前不标通过。

范围只含可靠有序DataChannel模拟鼠标消息，沿用Tauri/Vue、webrtc/rtc0.21.1、既有DXGI/MF/RTP和P004b临时配对。不调用SendInput，不移动系统鼠标，不部署云资源、不改防火墙，不进入P006/P1。

新增默认关闭p005-control feature（依赖p004-lan），原P004a/P004b默认构造器及视频-only校验继续拒绝application m-line。P005显式路径只接受一个recvonly H264视频和一个UDP/DTLS/SCTP webrtc-datachannel application，独立mid、同一BUNDLE、rtcp-mux、SCTP端口和有界消息大小；所有媒体节继续逐条检查指定IP/UDP候选、DTLS SHA256指纹。协议见CONTROL_V1.md。

WebView2在现有PeerConnection上createDataChannel，label和protocol均control.v1，ordered=true，可靠、in-band DCEP。Rust只接受一个且属性匹配的通道；所有鼠标/ping/ACK/pong字节仅由DataChannel收发。现有TCP仅做HMAC认证SDP、权限上下文、心跳与统计传送，不转发控制事件或生成ACK。

双方P005原生确认明确同意“视频+模拟事件，不执行实际输入”。认证offer/answer用独立消息种类和受限JSON信令信封，携带本轮随机controlSession UUID并双向核对；不能与旧video-only会话混用。权限仅存本轮原生对象，在停止/到期/断线/关窗时撤销；nonce与递增序号防跨轮消息复用。输入检查对手动/自动消息一视同仁，无管理员豁免。

Rust DataChannel API使用0.21.1的on_data_channel及poll事件，不套用旧版回调。回调只交付有界单通道句柄，独立受控任务验证属性、处理open/message/error/close及响应；视频任务结束时关闭通道、join任务，再报告资源释放。协议拒绝为fail-closed：统计固定错误码、停止本轮，无无界重试。限制应用消息1024字节、每秒60条、每轮1000条、发送缓冲16KiB，SCTP接收缓存有界。每条消息核对权限/stop/deadline、版本/类型/字段/坐标/seq/session。

控制端实验面板自动以20Hz发100条确定性归一化模拟事件（仅通道open且视频真正播放后），并发送ping；用户可查看发送/ACK/待确认/异常、通道事件及最后错误。Rust不保存坐标/原消息，只保存计数和序号范围。接收端统计可经认证TCP回传供A保存，但验收以Rust实际OnMessage/ACK写入和B实际DataChannel.onmessage共同确认，IPC/TCP统计本身不计成功。

先协议单测和严格SDP负例，再无桌面采集的真实本机WebRTC/SCTP集成（正常/异常），最后两台真实Windows PC视频+100事件、主动停止/关窗/拔线。数据按自动与双机分别留证；未测保留未完成。旧视频回归、全量构建/lint/clippy/test、审查和包CRC/SHA完成后Git交付。回退关闭p005-control，旧入口仍可独立构建。

参考：[W3C RTCDataChannel](https://www.w3.org/TR/webrtc/#rtcdatachannel)；本地锁定webrtc0.21.1 src/data_channel/mod.rs、peer_connection/配置API。无新第三方依赖，现有锁文件不升级。
