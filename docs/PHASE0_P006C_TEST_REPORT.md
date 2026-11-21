# P0-06c 首步：自动路径选择与真实云端回退报告

状态：**首步核心/无画面自动验证通过；P006c完整视频接入仍doing**。生命周期独立交付88f1edc后，先提交ADR0009（b8b0214），再开发默认关闭的p006-direct-probe。本轮不修改旧Loopback/LAN/relay-only入口，不采集屏幕、不调用输入API、不进入Phase1。

## 授权与实际环境

用户2026-11-22明确授权P006c四例自动云测试，范围见P006C_CLOUD_OPERATION_PLAN.md。专用coturn从北京时间2026-11-22T07:13:41.812+08:00运行至2026-11-22T07:17:31.271+08:00，约229.5秒，低于40分钟上限。WSS始终关闭，仅复用原UDP端口、短票和观察器，最多2个并发Allocation。

**同一台物理Windows电脑的两个真实原生PeerConnection**，ICE All、UDP4、可靠有序DataChannel、固定100条序号及逐条ACK。SDP仅在本机内存broker中交换，实际载荷必须经ICE/DTLS/SCTP；无IPC/TCP转发控制载荷。每例双方selected pair均由原生ICE transport实际查询并在收发后复查。不是新的两物理机/NAT穿透/跨ISP/WebView2或视频负载验收；100条为探针固定消息，不冒称新增control.v1物理双机测试。

## 真实路径结果

| 场景 | 实际选中pair（双方一致） | 发送/接收/ACK | 总耗时 | 错误 |
|---|---|---|---|---|
| TURN可用、直连正常 | host/host UDP | 100/100/100 | 1.953秒 | 无 |
| 进程内直连受阻、TURN可达 | relay/relay UDP | 100/100/100 | 34.873秒 | 无 |
| 进程内TURN受阻、直连可达 | host/host UDP | 100/100/100 | 22.051秒 | 无 |
| 进程内两路均受阻 | unknown（无selected pair） | 0/0/0 | 35.014秒 | DIRECT_CONNECT_TIMEOUT |

正常与fallback两轮，双方均保留host=1、srflx=1、relay=1，ICE policy始终all；fallback没有改成relay-only或删除直连候选。故障层真实丢弃非指定TURN端点UDP（包含直发远端relay端口的混合路径），保留经服务器的relay/relay，实际丢弃96包。因此这是进程内受控丢包下的ICE自动回退，不是物理防火墙或公网NAT失败实测。

TURN受阻轮实际丢弃30包，无srflx/relay，约10秒/端的收集观察上限后用已收集host继续，gatherComplete=false如实保留；直连仍成功。全受阻轮丢弃46包、0消息、pair为null，CLI退出1及DIRECT_CONNECT_TIMEOUT是预期失败，runner才将“符合负例预期”记通过，不将连接记成功。主流程最多60秒，再每端close最多3秒。

本机无云版本另验证host/host的100/100/100及全断15秒明确失败，见artifacts/p006c/local；与本表真实云数据分别保存。首轮严格拒绝库自动生成的component2，保留initial-component-rejection.json；随后本端SDP只传component1，远端component2仍拒绝，没有放宽外来候选范围。

## Allocation与收尾

正常/回退两轮共4个实际云端分配，均授予600秒；每个都观察到Refresh(0)请求、对应成功lifetime0回应、独立relay端口消失，首次观察消失为回应后1.140至1.394秒。同一服务PID贯穿、0.5秒socket采样，逐例确认0再发新票。TURN受阻的两轮4个票据标签无成功Allocate，与本地实际丢包计数和空socket一致。

本轮即时释放后提前结束，**未再次等待4个原租约deadline**，allocation-results.json中的该检查点明确为null；上一轮六例完整租约到期/宽限证据见TURN_ALLOCATION_LIFECYCLE_TEST_REPORT.md，二者不混写。所有本轮分配判定先于服务停止；停止前relay socket=0，不能用最终服务清零冒充单例释放。

最终coturn inactive/disabled，WSS未启动、到期timer取消，配置/unit/防火墙策略hash、业务容器启动时间及运行服务集合与本轮基线一致；原始控制捕获及服务端短票删除，保留SHA和脱敏事件。观察器退出0、无异常残留，见shutdown.json。客户端/服务器保留各自时钟，一次有界SSH测得server-minus-Windows约[-5.041,-3.494]秒；所有释放延迟仅用云端同一时钟，不能跨钟相减伪造耗时。

## 源码与自动检查

DirectGrant绑定短票会话/TTL及确切私有peer清单；独立DirectRuntime只允许指定服务器和已完整校验SDP的确切UDP端点，集合仅安装一次，跨会话或重复安装即撤销。拒绝危险IP、DNS/TCP/IPv6、错误指纹/媒体/大小、过期或未批准配置。新模式通过独立构造器开启，旧IceConfig.validate仍拒绝DirectPreferred，原视频H264校验及业务HMAC未放宽。

最终24项离线检查符合预期：66 Rust（较生命周期轮新增5）、旧桌面3、16前端、3 Edge、11 Python；fmt/lint/default及all-feature clippy、桌面与两种probe构建、旧feature独立检查和CLI默认拒绝。私有MF样本1项保持ignored。新增真实PC-close测试先有类型推断与缺handler的fixture错误，修复后通过；另一次回归暴露旧PinnedSocket测试在好包先到时拒绝计数仍0，其依赖两个UDP发送者到达顺序。三处socket测试均改为先独立确认坏包不被交付，再发好包；运行时策略未改，原错误保留development，最后完整重跑。

四例云端运行二进制SHA256为9eaebadf01722cd37be7ce71ac2b422d9c15de8c666bd38c5cc0abb59543da30，源文件hash见build-under-test.json。云测试后审查发现等待open时None可能空转，最终direct_probe.rs加入EOF失败返回及真实PC-close测试；其余三处文件仅修改socket测试顺序。最终构建/本机检查与云前二进制区分，见final-build-provenance.json，未为此擅自重启云端。本修正不改变已实测的候选、网络边界策略、传输或故障注入路径。

## 未完成与后续

P006c完整验收不标done：现有视频Session、WebView2接收和认证信令尚未接入DirectPreferred，故此首步不证明720p30与自动回退同时工作。34.873秒回退和22.051秒TURN不可达场景的等待仍偏长，当前批量SDP/收集等待需后续诊断；不承诺低延迟切换。未做trickle、ICE restart、切网恢复、长期身份、输入注入或新的物理双机验收；SDP外prflx端点仍拒绝，某些NAT直连可能因此回退。

下一步复用现有Session/control.v1和WSS认证接入视频路径，完善真实路径展示，再按独立范围验证WebView2/媒体兼容性；云端再次启动仍需新具体授权，不继承本轮已结束窗口。接收端主动停止补测按用户决定跳过，历史独立日志限制保持。
