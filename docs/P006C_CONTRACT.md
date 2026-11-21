# P006c 首步契约

状态：开发中；生产视频入口尚未接入。ADR0009先行。

- 模式：DirectPreferred独立显式构造器，ICE All；旧IceConfig.validate/native_configuration仍仅RelayOnly。
- 授权：短时会话UUID、双方批准、指定本地接口/TURN端点/relay范围、确切私有peer允许清单。授权过期、停止、候选校验失败后不能继续收发。
- 候选：本步data-only仅UDP4/component1、host、srflx、已批准relay；每份最多16，拒绝任意其他类型/传输/端点。本端rtc多出的component2在发送本地SDP前过滤，外来component2仍拒绝。指纹/BUNDLE/m-line数量必检，原视频入口及其rtcp-mux兼容契约保持。
- 原子放行：完整SDP成功后才安装唯一对端端点集合，最多16；禁止后续任意扩张、禁止重复协商和重放；不支持trickle或ICE restart。这沿用P006b完整SDP交换，尚未实现产品基线将来的trickle/reconnect。
- 路径：真实双方selected pair，每一候选UDP且属于本地gather或已核验remote集合；任意一侧relay为relay，否则host/srflx为direct。无pair为unknown，不以配置/候选存在代替连接成功。
- 自动探针：两原生PC同机、单application、100条可靠有序固定序号+ACK，无媒体/输入。公开仅计数/类别/耗时/释放，私有配置不进Git。
- 故障：测试进程内部可丢弃direct或TURN数据，保留同份候选、同ICE All。必须同时报告注入开关与实际选中路径，不能宣称为真实公网NAT阻断或物理双机网络测试。
- 限时与清理：每例主流程最多60秒，之后每端close各最多3秒；先PC.close后撤销边界以允许Refresh0，失败也清理。云端资源须独立观察，关闭PC不直接记云释放PASS。

验收分层：离线策略负例；Windows真实host/host；授权云端直连/自动fallback/部分不可达/全失败及allocation清理；最后现有视频/WebView2接入和物理网络验收。每层独立报告，未执行不标PASS。
