# ADR 0009：P006c 直连优先与自动中继回退

状态：接受，分步实现。前置交付88f1edc已推送main；不改变ADR0003/4/7的旧入口。

## 决定与兼容性

复用webrtc-rs/rtc 0.21.1，新增默认关闭的p006-direct-probe。第一步实现可复用的DirectGrant、候选策略、套接字边界与无画面Windows双PC自动探针；先实证路径选择，再接入现有视频Session及WebView2入口。旧relay_only、Loopback、LAN PinnedRuntime仍严格保留；不得把direct_preferred塞入旧relay构造器以绕过约束。

DirectPreferred使用ICE All，同轮收集host/srflx/relay，不人为延迟TURN收集；由ICE候选优先级及连通性检查选择。relay任意一侧被选中即为中继，不能要求双方都是relay才承认中继。没有真实selected pair即unknown；协商成功必须有真实SCTP响应，不能用gather完成替代。

只允许UDP4、指定本地绑定接口、指定公网TURN/STUN端点与短时票据。经认证且双方原生批准的会话才能构造业务DirectGrant；私有peer IP须另列确切允许值，最多2个，不能开放整个LAN。公网host/srflx限制为可路由IPv4、高端口；拒绝回环、元数据、CGNAT、保留地址、DNS/TCP/IPv6。每份SDP最多16个候选，实际对端UDP目的地由完整验证后的SDP一次性安装，不能在验证中逐步放行；不接受SDP外peer-reflexive端点。这会减少部分NAT下直连成功率，但有界回退可用，后续须单独审查更广的prflx支持。

生产SDP继续一video一application、同BUNDLE、SHA256指纹及原H264限制；data-only探针的单application契约独立，不可作为视频校验捷径。现有一次性码/HMAC/超时/主动停止不变；第一步探针仅本机私有broker交换两个真实PC的SDP，不声称已接通跨机产品信令。

## 自动验证

无采集/无输入/无WSS的Windows原生两个真实PC，可靠有序DataChannel发送100条有序固定实验消息并逐条确认，保留双方真实selected pair及候选类别数量，不保存SDP/地址/票据/载荷。四种测试：正常direct；测试套接字丢弃全部对端直连数据但保留TURN；丢弃TURN但保留direct；两者全丢弃。测试故障开关仅探针进程可设置，不改系统配置、不改生产策略。fallback测试仍ICE All、仍交换直连候选，不能通过改成relay-only冒充自动回退。

无云授权时只运行本地host/host与负面策略测试。真实云fallback及分配回收需新的具体启动授权；不能继承生命周期轮的已结束窗口。首步完成不将P006c或父项标done，也不标记物理双机视频/P2P实测通过。

## 风险

本阶段不是切网后ICE restart/无缝重连；ICE All并不保证每种NAT直连成功。TURN不可达可能延长gather，探针整体有界60秒；每条路径若不成功必须显式失败并关闭PC。PC close与云分配释放仍分别记录；不能以停止服务掩盖残留。下步视频入口接入需要单独真实兼容验证，Phase1及输入注入不在范围内。
