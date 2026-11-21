# control.v1 实验协议契约

状态：P0-05实验版v1。单一可靠有序、DCEP协商的DataChannel；label=protocol=`control.v1`、ordered=true、maxRetransmits/maxPacketLifeTime为空、negotiated=false。一个既有视频PeerConnection，最多一个通道。仅支持模拟坐标，不含键盘、按键、实际输入或任意命令。

文本UTF-8 JSON，不接受binary、未知字段/重复字段、不合法JSON。每条最多1024 UTF-8字节，64位整数seq取1..1000；每方向分别从1严格递增，包含该方向所有种类。v必须是整数1。session_id是经HMAC信令双方核对的本轮controlSession UUID；不是配对密钥，不作为持久身份。每秒固定窗口最多60个入站请求，每轮最多1000个；发送端正常20Hz，最大未确认128条、缓冲16KiB，限流不重发。

| 方向 | kind | 精确字段（包括公共v/kind/seq/session_id） |
|---|---|---|
| WebView2→Rust | mouse_move_sim | x_norm、y_norm：有限数值，均在[0,1] |
| WebView2→Rust | ping | 无额外数据 |
| Rust→WebView2 | ack | ack_seq：刚接受的mouse_move_sim序号 |
| Rust→WebView2 | pong | reply_to：刚接受的ping序号 |

示例：`{"v":1,"kind":"mouse_move_sim","seq":1,"session_id":"00000000-0000-4000-8000-000000000001","x_norm":0.25,"y_norm":0.75}`；合法ACK方向独立seq：`{"v":1,"kind":"ack","seq":1,"session_id":"00000000-0000-4000-8000-000000000001","ack_seq":1}`。示例是fixture，不是实测。

Rust只在P005原生批准且HMAC配对成功建立的Session处理。每次入站、发送回复前检查stop/当前授权及原生期限。每个合法mouse恰好返回一个ACK；ping恰好一个pong，ACK/pong无额外ACK。Rust统计分别记录accepted_moves/acks_sent/pings_received/pongs_sent/last_rx_seq/last_tx_seq和拒绝数；不持久化坐标或消息体。发送回复成功仅表示库已入发送队列，必须结合B收到ACK计数，不能称已交付。

不合法类型、字段、坐标、会话、序号、大小或速率均不ACK、不执行副作用，记录固定错误码并关闭本轮；错误处理不回显输入。每轮全新原生对象/UUID，已停止/过期授权不可重新signal或收发；重连必须重新双端批准/配对。OnError、OnClose或控制任务失败停止本轮，自动清理PC/通道/采集线程，无自动恢复。

P005认证信令使用`offer.control.v1`/`answer.control.v1`，payload精确JSON `{sdp,controlSession}`，拒绝未知字段/无效UUID/应答上下文不相等。仍由原有HMAC帧绑定方向、nonce、序号、种类和payload；完整SDP/代码不记录。旧P004b的offer/answer视频-only路径不接受此信封，避免静默降权。

统计证据：B发送计数从DataChannel.send成功累加、ACK/pong从onmessage核验后累加；A接收从Rust DataChannel OnMessage累加，回传从真实send_text成功累加。TCP/IPC只传元数据统计。正常轮须A accepted_moves=B sent_moves=B ack_received≥100，序号连续、无异常，视频仍实播。中断轮的pending不冒充丢失；关闭后未收取的浏览器事件不补造。
