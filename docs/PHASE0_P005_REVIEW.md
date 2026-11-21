# P0-05 代码与协议复核

2026-11-21；自审，不是独立渗透或供应链安全审计。

需求映射：ADR0005与CONTROL_V1先行；新feature默认关闭；原LAN模块创建的同一视频PeerConnection承载SCTP；B createDataChannel和A OnMessage/send_text是真实字节路径，IPC/TCP仅传信令与统计；没有实际输入API。默认、P004a及P004b独立构建不开放application协商。只修改媒体会话集成/授权/协议/UI/测试，无MF、色彩转换、RTP包化或H264参数实现变更。

已检查：原生双端确认和窗口来源、feature、单轮互斥；控制上下文由HMAC信令认证、不接受跨轮消息；DTLS指纹/候选检查保留；SDP最多一video一application且同BUNDLE/独立mid、有界SCTP参数；label/protocol/可靠有序/in-band/单通道检查；文本/1024字节、serde精确字段、范围/方向/版本/序号/速率/总量/当前权限/期限；每个ACK实际由DataChannel写出，B再核验ack_seq及连续回复序号；未把排队写入等同收到ACK。

并发与资源：on_data_channel回调只交付容量1的句柄，不在driver回调阻塞轮询；控制任务由源worker拥有，stop token串联停止源，PC关闭及控制任务join参与resources_released；回复前再次检查stop；重复/迟到授权、窗口退出仍沿用已有原生清理。100条确定性坐标不持久化，不采集全局鼠标；B停止先关闭通道与发送计时器。所有统计固定字段、无坐标/消息体/配对码/完整SDP；发送缓冲16KiB、接收SCTP缓存64KiB/消息16KiB，应用层仍1024字节；每轮上限防内存无界增长。

验证反例：旧入口收到application拒绝；多媒体、错误BUNDLE、SCTP参数、外IP/非UDP、坏DTLS指纹拒绝；真实本机SCTP重放/错会话/1025字节/非法/61突发均拒绝，关闭后send失败；纯协议权限false/到期及坐标越界/版本/字段/binary/总量拒绝；B收到错误响应、error事件、迟到open停止发送。前端fixture的error不声称是真实物理OnError；本机SCTP不声称双机。

复核中修正：H264格式查找限定video节，避免application中伪造rtpmap污染视频判断；每个媒体节独立mid；数据通道集成测试过滤buffered事件后核对真实OnMessage，不把任意事件当ACK。旧库fixture add_track只声明轨道，无采集/像素，测试明确断言frames_input=0。无依赖升级，Cargo.lock无差异。

最终10项自动命令均exit0（43 Rust、11前端、2Edge），双机两轮视频+100/100以及两端停止/关窗/拔线清理通过，证据见测试报告与artifacts/p005；未发现当前实验范围内的阻塞代码问题。残余风险：临时配对/会话授权不是正式身份与ACL；可靠20Hz短测不代表高频指针、弱网/长稳能力；库OnClose可能在PC销毁时只表现为事件流终止，close_requested、close_events及task_joined分别记录；B终态本地统计未收取，未伪造。无Win11/硬件丢失/独立安全审计；本轮不部署云/不改安全配置/不扩展P006或P1。回退关闭p005-control，保留P004b原入口。
