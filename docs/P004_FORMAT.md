# P0-04a 实时媒体与生命周期契约

只用于当前窗口受授权的 same-machine 实验，不是公网会话授权。

## 线程与数据

`hub-encode-win::stream::run` 由专用 Windows 线程调用，线程内初始化、使用、释放 DXGI/COM/MF。它复用现有 CPU BGRA→NV12、MFT 选择与 AU 结构，不读写视频文件。旧 `probe::run` 保留文件实验；仅共享跨进程互斥可见性，不改变旧 CLI 参数或行为。Tokio 侧只拥有编码后 AU，COM 对象不会越过线程边界。

源为真实 2560×1440 主屏，CPU 缩放为 NV12 1280×720，30Hz、4Mbps、Baseline，目标 GOP 60。静态桌面复用最近真实帧，轻微落后复用已转换 NV12，严重落后记录 missed ticks；不是把桌面更新计数当作视频帧数。新报告分别统计 input、encoded、sent、received、decoded、rendered。

AU 通道固定容量 3。满或发送失败时中止整轮，报告背压错误，关闭 PeerConnection、停止并 join 源线程；不在丢 P 帧后继续宣称正常。MF 输入/输出与 drain 继承已有上限，单 AU≤8MiB、总输出≤128MiB、≤900 帧、≤30 秒。停止丢弃整段剩余参考链，不继续会话。

## H.264 与 RTP

每个 `EncodedVideoFrame` 必须包含单个完整 Annex B AU。先校验 NAL，首 AU 要有 SPS/PPS/IDR，再调用锁定版本 `rtc::rtp::codec::h264::H264Payloader`。它实际处理单 NAL、STAP-A 与 FU-A，删除起始码和无媒体作用的 AUD/filler；不能把起始码直接当 RTP payload。负载 MTU 1188 字节，加基本 RTP 头约 1200；额外 SRTP/UDP/IP 开销另计。每 AU 仅最后一包 marker=1，sequence 连续，SSRC 属于本轮单轨。

RTP 时间戳为 `round(pts_100ns*9/1000)`，30Hz 正常间隔 3000；使用原 MFT PTS，不从磁盘、浮点 duration 或 ffprobe 帧率重建。当前限时实验以 0 起始；未来公网产品应补充随机初值等生产约定。

SDP offer/answer 为 H264/90000、payload 102、packetization-mode=1、level-asymmetry-allowed=1、profile-level-id=42001f。WebView2 codec stats 也独立返回该协商结果。AMD 本机真实 SPS 是 42041f，constraint_set5_flag 对 profile 66 不适用；ITU-T H.264 7.4.2.1.1 要求解码器忽略此位。只在兼容性判断忽略该位，保留并披露原 SPS；该驱动的置位不符合编码端规范，不能称原 SPS 逐字节为 42001f。其他 profile/level/约束不匹配仍拒绝。无 SPS 改写或重编码。

RTCP 经过默认 NACK/report 拦截链；额外观察 PLI/FIR 并通知 Windows 线程，用 `CODECAPI_AVEncVideoForceKeyFrame` 的 VT_UI4=1 请求 IDR。驱动拒绝计数可见，周期 GOP 仍保留。本轮实际 PLI/FIR 均为 0，没有把代码存在当作端到端丢包恢复已验收。RTT 仅在库报告有效测量次数时给值，未测不显示成 0ms。

## 同机信令

Rust webrtc 0.21.1 只绑定 127.0.0.1 UDP，不开 TCP listener；无 ICE server，mDNS disabled。JS recvonly offer 完成 host 收集后，在送入 IPC 前删去 LAN/mDNS/relay 候选并归零 c= 地址；原生还独立拒绝非 loopback/非 UDP 候选、额外媒体/data m-line、sendonly/sendrecv、remote-candidates 与超长 SDP。

实测浏览器会通过 Rust 答案中的 127 候选完成 ICE 检查，并产生 peer-reflexive 本地候选。最终浏览器 selected pair 是 local prflx / remote host，remote address=127.0.0.1。不能推广为其他 WebView2/网络配置必定可连。浏览器自身 host gathering 行为由 Chromium 管理，本轮不修改浏览器或 Windows 安全设置。

SDP 含临时凭据，只经当前窗口 IPC 交换，不写公共日志。视频走 DTLS/SRTP，不经 IPC、WebSocket、base64、Canvas 或 MP4 文件播放。

## 授权、关闭与报告

`p004-probe` Cargo feature 默认关闭。新增命令通过 Tauri ACL 限定 main 本地窗口，原生再次检查实际 URL/label。UI 勾选之外，还必须通过原生 Windows 对话框同意本轮；后端维护生成的 sessionId、并发门禁与过期，stop 幂等。CSP 只为视频加 media-src blob，不加任意网络 fetch。

关闭窗口时先取消、关闭 PeerConnection、join 媒体线程，再销毁窗口。同步驱动调用仍不提供硬实时可中断保证；超时保持失败/不可重启状态，不把尚未释放的线程标作成功。无磁盘视频样本；结束时只在程序目录 artifacts/private/p004 写新统计文件，拒绝覆盖，不接受输出路径参数。关窗快照只有原生最终状态，浏览器已卸载后的帧数保持缺失，不补零或猜测。

原生 connection/ICE 字段是最后一次回调观察，close 后可能仍保留 connected；释放以 close 返回、join 与 resources_released/cleanupSucceeded 为准。浏览器关闭另由实际 connectionState=closed 和 track ended 判断。快照中开始到首帧含人工确认及约 350ms 轮询粒度；不是编码或端到端延迟。RTP 字节为媒体 payload，不含所有线缆开销。

参考：[官方 webrtc 0.21.1 API](https://docs.rs/webrtc/0.21.1/webrtc/)、[RFC 6184](https://www.rfc-editor.org/rfc/rfc6184)、[ITU-T H.264 2024](https://www.itu.int/rec/T-REC-H.264-202408-I/en)、[ForceKeyFrame 类型](https://learn.microsoft.com/en-us/windows/win32/medfound/codecapi-avencvideoforcekeyframe)。
