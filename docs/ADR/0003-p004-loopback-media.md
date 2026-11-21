# ADR 0003：P0-04a 有界同机 WebRTC 视频实验

日期：2026-11-21。状态：实施中，是否通过以原生 WebView2 实测为准。

## 基线与范围

本机 HEAD 与远端 main 均为 `7fcbb519709a4655aa77aa4b8eca945c413891c2`。当前 README/TASKS 已正确说明 P0-03 完成，与审查文档所见不同；无法仅凭此断定是缓存还是审查时间差，不做无意义更改。P0-04a 只验证 same-machine 真实 DXGI → MF H.264 → Rust WebRTC → WebView2，不实现 DataChannel、双机、STUN/TURN、公网入口或输入注入。原 P0-04 仍需双机，不可标记 done。

## 版本与封包

编码前核对官方 docs.rs 与 crates.io 下载源码，采用 `webrtc = =0.21.1`（MIT/Apache-2.0，默认 Tokio/ring），不照搬旧 RTCPeerConnection API。新版本是异步 handle + background driver。依赖固定到 Cargo.lock，开发期下载不随交付打包；未知漏洞状态不得冒充审计通过。

优先使用该版本公开 RTP track API 及库自带 H264 payloader。完整 Annex B AU 经已验证解析去起始码并按 RFC 6184 的单 NAL / FU-A 规则封包，MTU 1200。SPS/PPS/IDR 与实际 Baseline level 3.1（42001f）检查后才发送。采用 90kHz 时间戳，`round(pts_100ns * 9 / 1000)`，正常相邻帧 3000 ticks；保留实际 PTS 的间隔，不从磁盘或 duration 浮点累加重建时间轴。Payload type 来自 SDP 交集。

## 媒体与背压

新增 stream 生产者，独立 Windows OS 线程持有 DXGI/COM/MF，复用颜色转换、MfEncoder 与 EncodedVideoFrame；P0-03 文件 probe 保持原行为。Tokio 侧只接收拥有字节数据的 AU，禁止移动 COM 对象。队列容量 3 AU，满时明确失败并停止整轮，不能随机丢 P 帧。初始化、协商、输入等待、总时长与停止有期限。正常周期 IDR（60 帧）以及 RTCP PLI/FIR 触发下一输入强制 IDR；若驱动不支持请求则明确记录并有界停止/周期恢复，不宣称任意丢包可恢复。

## 同机与授权边界

Rust 只绑定 `127.0.0.1:0` UDP，不创建 TCP listener，不配置 ICE server，关闭 mDNS。浏览器不配置 ICE server，候选由 IPC 交换；拒绝外机、relay/srflx、非 UDP 及额外媒体/data m-line，原生端候选只接受 loopback 或本机接口地址。WebView2 可能不公布 loopback candidate，必须实测；若需本机接口兼容，仍保持原生 socket 只绑定 loopback，不能通过开放公网地址绕过。证据仅记录地址分类与端口，完整 SDP/私有地址不进入 Git。

Tauri `p004-probe` Cargo feature 默认关闭；仅显式构建的实验程序提供入口。所有新增命令核对预期 main 窗口、local app URL、配置及 sessionId。开始前要求 UI 勾选本次同意，并由原生 Windows 确认框批准；后端维护窗口绑定的本轮状态，JS 布尔值本身不能创建授权。并发返回 BUSY，停止幂等，窗口关闭触发取消并等待资源清理。CSP 仅增加视频所需 blob media，不开启任意网络 fetch 或远程 IPC。

## 接口与观察

`start(config)` → sessionId；`signal(sessionId, browserOffer)` → answer（含 ICE）；`stats(sessionId)` → 有界的状态、发送/编码统计；`stop(sessionId)` 幂等。先创建 recvonly browser offer 并完成本机 host candidate 收集，单次 IPC 交换有界完整 SDP，无远程信令服务。只传 SDP/统计，视频通过 SRTP。状态 idle/starting/negotiating/connected/playing/stopping/stopped/failed，播放状态和接收指标由真实 video/getStats 测得，原生发送 connected 不能冒充 playing。

失败后保留最后一轮非画面统计；再次开始生成新 sessionId。至少 10 秒真实播放、5 次启停、拒绝授权/并发/非法参数/关闭/编码与连接失败、旧 P0-03 回归和全 workspace 检查共同构成验收，未执行的项目必须列明。测试截图和完整 SDP 只保留本机忽略目录。

参考：[webrtc 0.21.1](https://docs.rs/webrtc/0.21.1/webrtc/)、[PeerConnection API](https://docs.rs/webrtc/0.21.1/webrtc/peer_connection/trait.PeerConnection.html)、[RFC 6184](https://www.rfc-editor.org/rfc/rfc6184)。
