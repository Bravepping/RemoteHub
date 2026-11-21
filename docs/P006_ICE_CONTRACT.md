# P006a ICE/信令逻辑契约

IceConfig={mode,ice_servers,expires_at,session_id}，使用snake_case字段；实验配置另有批准期限及target{local,turn,relay,port_min,port_max}，以probe-input.json.example为准。短期username/credential不写应用长期配置或日志；显式操作者CLI仅为未来获授权的G6读取短期私有JSON（Unix0600/Windows限制ACL），使用后撤除访问并销毁，不能放TURN共享密钥。自有Rust秘密缓冲区释放时清理；SDK内部副本与JavaScript字符串不承诺完全清零。config.validate检查当前期限、60..600秒剩余TTL、到期不得续用，最多2个服务器/每个2个URL。IceConfig::parse最多8192字节；CLI整个文件最多16384字节，并校验同一有限字段集。

| mode | 数据面策略 | 当前执行状态 |
|---|---|---|
| Loopback | 原127范围、无ICE server | 旧P004a不改 |
| LanPinned | 原指定RFC1918对端，PinnedRuntime | 旧P004b/P005不改 |
| RelayOnly | 原生Relay/WebView2 relay；物理端点与逻辑relay范围分别校验 | P006a默认关闭框架，无桌面采集入口 |
| DirectPreferred | 未来All+host/srflx/relay与动态对端边界 | 仅契约，执行返回未启用 |

0.21.1原生当前只支持TURN UDP。地址输入严格turn:批准数字IPv4:合法端口?transport=udp；stun可在DirectPreferred未来单独评审，当前探测框架不自动访问任何公网STUN。TCP、turns、DNS、不批准服务器、永久配置或未知mode失败关闭。无测量时selected pair、RTT、丢包为null。

## rendezvous-probe.v1草案（没有上线服务）

双方人工授权前由受信任实验管理员建立一轮随机session UUID/256-bit临时码、独立于TURN共享密钥的会话MAC密钥。WSS必须有效证书且固定可信origin；不允许信令服务器直接替换已认证SDP。连接端点与部署权限另行确认，不因本文件而授权云操作。

帧字段version=1、session、direction、nonce、seq、expires_at、kind、payload、mac，拒绝未知字段。direction为controller-to-host或host-to-controller；kind仅offer/answer。MAC=HMAC-SHA256(key, UTF8(JSON紧凑数组["rendezvous-probe.v1",version,session,direction,nonce,seq,expires_at,kind,payload]))，编码以Rust serde_json为准，无额外空格；mac为64位十六进制，常量时间比较。离线Verifier已实现；WSS传输、随机码交换和网络端审批尚未实现。方向独立严格seq从1、每轮新随机非nil UUID nonce/会话码、最大帧96KiB/SDP64KiB、最多16候选/媒体节、信令10消息/秒和256消息/轮、无自动恢复。原生双方批准角色及权限video+control_sim，HMAC证明持码，不冒充永久设备身份。

服务端签发TURN票据必须已经完成会话认证、双方批准、绑定session/realm、未撤销且未到期，另限制每角色最多2次签发、序号防重放。服务端拒绝未认证/单方批准/过期/错realm/超TTL；无公开“输入设备ID取票据/SDP”的API。纯issuer的trusted session参数是认证层边界，不是接受浏览器任意布尔值的HTTP接口；本轮未接入WSS，无法宣称其远程认证已实现。

SDP/ICE交由独立策略验证后才交库：媒体数量/方向/codec、BUNDLE/mid、SHA256指纹继承、candidate组件/UDP/类型/端口/IP及长度均核对。RelayOnly逻辑peer必须在批准relay地址/端口范围；物理socket只允许批准TURN服务端端点。保留消息完整性保护，不能关闭DTLS/TLS证书验证来通过测试。

后续P006b必须两个不同网络的Windows端都实际选择relay，视频/control真实通过并记录实际pair/getStats；仅Allocate成功、配置包含TURN或策略字符串为relay都不是媒体验收。
