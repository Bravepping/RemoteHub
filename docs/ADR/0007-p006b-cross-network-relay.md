# ADR0007：P0-06b 跨网络强制双端 TURN 实验

2026-11-21；开工fetch确认HEAD/origin/main=4507fa97d0184fbe8fd54702dfadaf8afbe9ce82，工作树干净。用户已准备两台Windows（本机有线宽带、接收端手机蜂窝热点），无域名、指定公网IPv4+端口，并授权启动同一ECS的服务。本轮只P006b，不进入自动直连回退、长期设备身份、实际输入或Phase1。

## 决策与替代

沿用Tauri/Vue、现有DXGI/MF/H264/RTP以及control.v1，同一个视频PeerConnection承载可靠有序模拟事件。保留Loopback/LanPinned构造器与PinnedRuntime不变；在Session增加显式Relay配置入口，复用P006a RelayRuntime/候选范围验证/实际库RTCIceTransportPolicy::Relay。浏览器同样relay-only；开始媒体前与运行中校验两端真实selected pair，缺值保持null，路径不符终止。所有物理TURN UDP只允许现有指定端点；逻辑对端仅批准服务器relay地址与端口范围，不能删IP/指纹限制。

新增隔离、默认关闭p006-relay。新hub-probe-wire共享实验契约，新hub-rendezvous-probe用Rust/Axum与内存房间（不推进正式SQLite身份/配对）；客户端用rustls+WebSocket。服务TLS证书含IP SAN，由本次专用CA签发；CA仅应用内信任并与测试包绑定，不安装Windows全局根证书、不关闭证书/有效期/IP匹配验证。未选择明文ws、全接受证书或复用旧LAN TCP；Python自制WSS替代方案不采纳，保持云端Rust架构。云端构建工具隔离于专用目录，单任务限制内存，不升级既有业务软件。

## 信任与凭据

实验包具有操作者本次签发的限时接入邀请（角色分离、期限最多2小时），仅用于此实验服务，不是TURN共享secret或长期设备身份；实际邀请文件与CA/目标配置不入Git，测试后撤销。WSS升级前验证邀请；客户端每轮原生确认后才连接。发送端产生256-bit临时配对密钥，只显示给用户，接收端人工输入；房间ID是其SHA256，配对密钥不发云端。双方已授权的实验进程入房后，服务端以受认证上下文/双角色批准触发现有server-only issuer，签发绑定随机session UUID、短期限的TURN票据。该实验授权不是对操作系统的身份远程证明。

转发SDP/统计/心跳/stop的信封以配对密钥做端到端HMAC-SHA256，域分离RemoteHub.Relay.v1，覆盖session、nonce、方向、严格递增序号、期限、kind和完整payload；云端不能替换有效SDP/DTLS指纹。帧96KiB/payload64KiB、每秒10条/方向、每轮256条；房间最多2个、每角色一连接、全局连接有上限、等待120秒、已配对期限有界、断开/停止即终止，失效房间不得复用。只转发信令与脱敏统计，视频/鼠标事件及ACK必须经真实WebRTC，不能用WSS代传冒充成功。

保留REST bearer可复用/无法单票即时撤销、realm不在REST HMAC的限制；云端secret仅受限env，客户端只有短票、释放自有秘密缓冲区。日志不记邀请、配对码、完整SDP/候选/nonce、坐标或画面。私有实验配置与可审计源码交付分开，不把临时接入票写入长期客户端配置。

## 验收与回退

先协议/权限/重放/限流/大小/TLS正负例与本机真实WSS集成，再所有旧feature独立回归及完整build/lint/clippy/tests。原生和浏览器都拿真实pair，不能把配置为relay当实测。双机必须用户每轮亲自确认，至少30秒720p30与100事件/100Rust接收/100ACK，序号连续；主动停止、关窗、热点断网清理留真实脱敏证据。未执行项标未测，不填PASS，不改P004录屏证据差异。

复用现有coturn专用unit，新增信令专用非root单元与独立配置/8443端口；先核对端口/业务服务。没有修改防火墙/安全组的既有授权，不关闭安全控制；若实际连通性需要规则变更，先给具体最小规则再处理。完成后停止本次服务、不自启，原业务服务/容器保持；恢复关闭p006-relay即可回到旧客户端，Git旧基线及配置备份保留。若实际库/浏览器不能互通，不以放宽旧scope规避，回到网络策略诊断。
