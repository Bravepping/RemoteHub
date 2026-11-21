# P006c 视频/模拟控制集成契约

实现顺序：Rust Session → WebView2 → WSS；未验证项不得标PASS。

1. 新模式通过p006-direct和显式new_direct开启；普通、Loopback/LAN/RelayOnly构建行为保持。
2. DirectSettings绑定grant、短票及可发布私有host策略。控制session UUID必须等于grant session，不能借其它会话重放。
3. video SDP复用原媒体/H264校验，仅候选scope独立扩展；大小64KiB、全份最多16候选、UDP4 component1，严格指纹和单BUNDLE；旧入口不接受DirectPreferred的host/srflx。
4. 完整SDP校验后原子安装remote集合，local集合来自真实gather并独立校验；实际pair两侧逐一匹配、过期/停止拒绝。控制授权与视频启动等待原生验证通过，后续定期复查。
5. Hello新增mode，RelayOnly缺省且序列化省略；双方必须一致。DirectPreferred会话双方先发送一次HMAC frame kind=path，payload为{v:1, mode:direct_preferred, private_peer_ipv4:[确切批准IP]}，最多512字节/2个RFC1918地址。非法/重复/错模式/未完成策略先offer都拒绝。清单记录的是允许连接的对方私有IP，不是自报身份。
6. 发送SDP保留private host仅当对方清单包含本机确切接口；接收SDP仅允许本地清单内private host。公网候选仍须可路由IPv4、高端口，relay仍限专用端点/范围；无DNS、CGNAT、元数据或SDP外prflx。
7. WSS应答生成并行于原心跳，四秒信令租约及既有证书/角色邀请/HMAC/序号不降低；有界协商和media任务统一停止/join。DirectPreferred总会话预算覆盖协商+30秒视频，不能无限重试。
8. 浏览器仅在真实video播放及真实pair通过候选集合检查后发送control.v1；原生ACK经同一SCTP返回。公开metrics可增加connectionPath=direct|relay|unknown，无地址/完整SDP；未知为unknown，不能冒充connected。
9. 合成测试源只在默认关闭的操作员probe feature中启用，运行真实MF/SRTP/解码，不读取桌面。统计sourceKind明确区分desktop/synthetic_nv12；正式UI仍须每次亲自原生同意。
10. 本地结果与云端/两台物理Windows各自报告。后两层需要新授权/用户实际观看，未执行标待测；不重复用户已跳过的P006b接收端停止补测。
