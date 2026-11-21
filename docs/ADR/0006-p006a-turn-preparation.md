# ADR0006：P0-06a TURN准备与隔离的ICE策略

2026-11-21，基线079275d1875e78bfbc811a429a52e4dbb539cb5a；开工fetch=main、工作树干净。范围仅P006a implementation-ready。无已确认云主机/系统/权限，未获SSH/安全组/部署授权，不访问真实主机、不运行公网探测，G6保留blocked。P006父项不完成，不进入P006b/c/P1。

## 策略与兼容性

保留现有NetworkScope::Loopback/Lan及PinnedRuntime原语义。新增隔离ICE逻辑契约：Loopback、LanPinned、RelayOnly、DirectPreferred。旧三种实验默认构造器/feature不获得公网配置；P006默认关闭p006-turn-probe，只提供有界、短期、受批准配置的无采集诊断框架，不注册公网桌面会话IPC。DirectPreferred仅定义契约，执行路径明确拒绝，留P006c。

RelayOnly区别物理TURN服务器端点与逻辑对端relay候选：物理UDP仅允许批准的服务器IP:3478，逻辑SDP只允许指定relay公网IP和批准端口范围、relay类型/UDP，仍限制video/control m-line、BUNDLE、DTLS指纹和消息长度。不是把LAN peer改成服务器。新策略单独使用真实RTCIceTransportPolicy::Relay；WebView2工厂用iceTransportPolicy:'relay'。DirectPreferred未来用All并另设计动态对端权限，不能直接删除socket/SDP校验。

本地锁定webrtc/rtc0.21.1源码核对：RTCIceServer{urls,username,credential}；RTCConfigurationBuilder.with_ice_servers/with_ice_transport_policy；PeerConnectionEventHandler.on_ice_candidate；pc.sctp()->transport()->ice_transport()->get_selected_candidate_pair或get_stats的Transport.selectedCandidatePairId；RTCIceCandidatePair.local()/remote()。无有效选择时诊断null，不因配置了TURN显示relay。

重要限制：该版本webrtc/src/peer_connection/transport/turn_relayer.rs gather明确跳过is_secure()与非udp。P006a只允许turn:IPv4:port?transport=udp，拒绝turns/TCP/域名自动解析。coturn模板UDP起步，TCP/TLS仅说明后续能力验证与证书条件，不能假称客户端支持。WebRTC本身DTLS/SRTP不关闭验证。

## 凭据与信令信任边界

新增server-only纯模块（不成为desktop或hub-webrtc依赖），TURN共享密钥仅通过服务端受限环境加载、无Debug/Serialize、释放清理内存。依托已有锁定的RustCrypto hmac/sha1/sha2、base64、zeroize，固定版本，不升级现有WebRTC。算法按coturn REST兼容：username=expiry_unix:session_uuid、credential=base64(HMAC-SHA1(secret,username))；TTL默认300秒，允许60..600秒，不超过批准会话期限。SHA1仅此兼容HMAC，不用作登录签名。

实验签发入口要求受认证的会话上下文与双方手动批准、到期/撤销守卫；无任何公开HTTP端点。会话随机码/HMAC与TURN静态密钥分离。rendezvous-probe.v1契约定义经WSS传送的方向/随机session/nonce/seq/期限/大小/限流与完整SDP/指纹MAC保护，未实施可公网服务；后续需完成正式Ed25519/DPAPI身份/ACL。

明确限制：coturn REST票据是有效期内可复用的bearer能力，username含session不代表coturn执行应用ACL，也不具备单票即时撤销/单次使用；会话层防跨轮与重放，云端配额/危险peer拒绝/短TTL限制滥用。未知realm、未授权会话不得由issuer签发；实际错realm/密码/到期Allocate拒绝必须未来对真实coturn验证，算法单测不能代替。

## coturn与验证

交付需渲染后才可运行的安全模板、server-only .env.example、配置静态检查/只读inventory、Linux包+systemd手工部署/回退步骤。限定监听/relay接口、3478/udp和49160..49259 UDP、明确EIP私网映射决策；use-auth-secret、独立realm、拒绝匿名Allocate/危险peer、配额/带宽/最小用户、无verbose/无界日志。日志不保证无IP，使用受限短期私有日志及脱敏摘要，不上传原日志。

先离线测试向量、TTL/授权/重放/配置/SDP/秘密Debug负例；真实库配置与未连接diagnostic；旧P004a/P004b/P005回归、全构建/lint/clippy/test和配置验证。真实Allocate/STUN需要明确目标及授权，未测记blocked，不用模拟成功填G6。交付源码/文档/脱敏自动证据、Git SHA与校验包；完成后停止等待审查。

来源：[coturn当前配置说明](https://github.com/coturn/coturn/blob/master/examples/etc/turnserver.conf)、[TURN REST草案及撤销限制](https://datatracker.ietf.org/doc/html/draft-uberti-behave-turn-rest-00)。coturn wiki自身声明过时，以目标安装包help/man和当前源核对；服务端版本尚未知。
