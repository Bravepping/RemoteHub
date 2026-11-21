# P006a ICE/信令逻辑契约

IceConfig={mode,ice_servers,expires_at}；实验配置另有session_uuid、批准期限、local_ipv4、approved_turn_endpoint、relay_ipv4、relay_port_min/max。服务器/客户端均仅在内存持有短期username/credential，不写配置或日志，不包含TURN共享密钥。所有构造器检查有效期、60..600秒TTL、到期不得续用，最多2个服务器/每个2个URL，总字符串和SDP有界。

| mode | 数据面策略 | 当前执行状态 |
|---|---|---|
| Loopback | 原127范围、无ICE server | 旧P004a不改 |
| LanPinned | 原指定RFC1918对端，PinnedRuntime | 旧P004b/P005不改 |
| RelayOnly | 原生Relay/WebView2 relay；物理端点与逻辑relay范围分别校验 | P006a默认关闭框架，无桌面采集入口 |
| DirectPreferred | 未来All+host/srflx/relay与动态对端边界 | 仅契约，执行返回未启用 |

0.21.1原生当前只支持TURN UDP。地址输入严格turn:批准数字IPv4:合法端口?transport=udp；stun可在DirectPreferred未来单独评审，当前探测框架不自动访问任何公网STUN。TCP、turns、DNS、不批准服务器、永久配置或未知mode失败关闭。无测量时selected pair、RTT、丢包为null。

## rendezvous-probe.v1草案（没有上线服务）

双方人工授权前由受信任实验管理员建立一轮随机session UUID/256-bit临时码、独立于TURN共享密钥的会话MAC密钥。WSS必须有效证书且固定可信origin；不允许信令服务器直接替换已认证SDP。连接端点与部署权限另行确认，不因本文件而授权云操作。

帧字段version=1、session、role、nonce、seq、expires_at、kind、payload、mac。MAC=HMAC-SHA256(域分离rendezvous-probe.v1、方向、全体字段和完整payload的确定性编码)，常量时间比较；方向独立严格seq从1、每轮新随机nonce/会话码、最大帧96KiB/SDP64KiB、最多16候选/媒体节、信令10消息/秒和256消息/轮、无自动恢复。原生双方批准角色及权限video+control_sim，HMAC证明持码，不冒充永久设备身份。

服务端签发TURN票据必须已经完成会话认证、双方批准、绑定session/realm、未撤销且未到期，另限制每角色最多2次签发、序号防重放。服务端拒绝未认证/单方批准/过期/错realm/超TTL；无公开“输入设备ID取票据/SDP”的API。纯issuer的trusted session参数是认证层边界，不是接受浏览器任意布尔值的HTTP接口；本轮未接入WSS，无法宣称其远程认证已实现。

SDP/ICE交由独立策略验证后才交库：媒体数量/方向/codec、BUNDLE/mid、SHA256指纹继承、candidate组件/UDP/类型/端口/IP及长度均核对。RelayOnly逻辑peer必须在批准relay地址/端口范围；物理socket只允许批准TURN服务端端点。保留消息完整性保护，不能关闭DTLS/TLS证书验证来通过测试。

后续P006b必须两个不同网络的Windows端都实际选择relay，视频/control真实通过并记录实际pair/getStats；仅Allocate成功、配置包含TURN或策略字符串为relay都不是媒体验收。
