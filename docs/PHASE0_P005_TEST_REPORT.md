# P0-05 DataChannel测试报告

2026-11-21；状态：P0-05通过，已停止扩展并等待审查。开工fetch确认main=b914942、工作树干净；ADR0005/control.v1先提交f68e42e，再修改源码。未重建工程、未升级依赖。

## 实现

默认关闭p005-control，依赖旧p004-lan；原P004a与P004b video-only构造器继续拒绝application m-line。复用同一个H264视频PeerConnection，WebView2 createDataChannel(control.v1，可靠有序)，Rust on_data_channel交付一个有界句柄，poll真实open/message/error/close。控制任务关闭后join，才将会话资源标释放。

每轮双端原生确认明确视频+模拟坐标，保留临时码HMAC/指定IP双向UDP过滤/3秒心跳租约/30秒媒体。P005 HMAC信令信封绑定本轮controlSession，旧video-only不可混用。消息逐项检查类型、字段、UTF8大小、版本、会话、权限/到期、连续序号、归一化坐标、60/s及1000/轮上限。拒绝后无ACK并终止本轮；没有Windows输入API。TCP/IPC只交换认证SDP和统计，不转发鼠标事件或ACK。

## 自动测试（与双机分开）

- 纯协议：100条连续模拟事件/ACK、ping/pong、缺权限/到期、错会话、重复序号、版本/类型/字段/坐标/binary/大小、固定窗口和总量限制。
- 本机真实WebRTC/SCTP：两个实际库PeerConnection、video+application真实协商（视频fixture不采集也不发帧），100条消息实际发送/OnMessage/ACK回传、ping/pong；随后重复序号导致停止。另开连接验证错会话、1025字节、非法消息和61条突发限流，关闭后继续send失败。不是普通函数/IPC/TCP模拟通道。
- SDP：P005精确两m-line/同BUNDLE/独立mid/SCTP参数/DTLS指纹；错误媒体/额外m-line/错IP/非UDP/无bundle/无mux/参数非法拒绝；原P004a/04b拒绝DataChannel。真实库offer/answer均过策略检查。
- 前端11项测试（含3项控制器fixture测试），验证open/message/close/error回调、100发送与ACK/pong核对、错响应停止、迟到open不发送、原视频/预览回归。这里的error回调是受控fixture触发，不冒充物理网络OnError事件。
- Edge实浏览器2项通过：旧预览不伪装核心可用；实际创建video+application、BUNDLE和可靠有序control.v1，未采集屏幕。不将其当双机DataChannel通过。
- 完整verify的lint/typecheck/build/fmt/clippy/default测试通过，all-feature clippy和43项Rust测试通过；原P004a/04b feature独立check通过，Cargo.lock无变化。最终10项命令全部exit0，见artifacts/p005/final-checks/results.json及UTF8原始输出，早期preflight保留。实测二进制SHA见artifacts/p005/preflight-summary.json。私有真实MF AU测试保持显式ignored，无额外桌面采集。

## 双机实测

两台原P004b物理Windows10电脑，同一有线局域网；用户亲自确认、使用P005-v1新配对。用户确认变化画面与B面板100/100、待确认0/异常0。A原生实际OnMessage接收100个mouse_move_sim、写入100个ACK；B真实DataChannel.onmessage收到100个ACK。一次ping/pong，双向总序号1..101连续，open各1次、message各101次，拒绝/异常均0。A控制任务已join，源与会话资源释放；证据为artifacts/p005/dual-pc-full.json的A原生统计及经认证TCP回传的B真实浏览器计数，TCP只传统计，没有传控制事件。

视频源输入/编码/发送900帧；A最后保存的B视频样本为893帧接收/解码/呈现，1280×720、host/host UDP指定对端、接收丢包0。此值是停止前最后回传样本，不冒充B最终900帧，也不将与发送数之差直接称丢帧。用户确认控制运行时画面持续变化，未将旧P004b900帧作为本轮证据。

第二轮完整30秒同样100/100、连续101序号、拒绝/异常0，输入/编码/发送900，最后回传视频893、丢包0，作为独立重复记录见dual-pc-repeat.json。完整轮A原生close_requested=true、task_joined=true、PC/源释放成功；native close_events=0，B最后回传时仍open/closeEvents=0，未把关闭请求伪写成收到close事件。

随后用户按顺序进行四个短轮次，均在100/100后操作，统计见dual-pc-cleanup.json：

| 场景 | 视频发送帧 | A接收 / ACK写入 / B确认 | A真实close事件 | 结束/清理 |
|---|---:|---|---:|---|
| 接收端主动停止 | 188 | 100 / 100 / 100 | 1 | stopped，控制任务join、源/会话释放 |
| 发送端主动停止 | 207 | 100 / 100 / 100 | 0 | stopped，控制任务join、源/会话释放 |
| 接收端直接关窗 | 210 | 100 / 100 / 100 | 1 | stopped，控制任务join、源/会话释放 |
| 接收端拔线 | 279 | 100 / 100 / 100 | 1 | LAN_LEASE_EXPIRED，控制任务join、源/会话释放 |

六轮都只有一次ping/pong、序号连续1..101、协议拒绝0；主动停止/关窗提前结束而非等待900帧。拔线后依据原生3秒租约撤销，不自动恢复，用户确认四项都回收；拔线物理瞬间未单独打时戳，不声称精确清理延迟。A接收端停止/关窗轮的OnClose计数1是直接通道事件证据；PC被本地直接销毁的轮计数0如实保留。

B本地final及其关闭后统计未收取，B界面操作/终态回收由用户确认；B正常运行中的ACK计数是已保存的真实浏览器回传，不是用户手填制造。物理网络未触发DataChannel OnError（计数0），error回调覆盖是自动fixture测试，网络故障实际表现为租约到期/close。未收取的记录不补造。

## 风险、限制与下一阶段

临时实验权限不是正式设备身份/ACL；只做模拟坐标，不含实际鼠标或键盘注入。静态调用扫描SendInput/mouse_event/keybd_event/SetCursorPos均0；没有像素或控制事件IPC代理，不保存坐标/桌面。固定可靠有序通道和20Hz有界实验不代表将来高频指针最佳方案。没有长稳、Win11、硬件丢失、独立渗透/RustSec、TURN或公网验收；P004原基线录屏差异保留，没有擅自更改其验收。本轮不动P006和Phase1、不改系统安全设置。下一阶段建议仅供审查：先评审本协议和临时权限，再独立安排TURN节点与P006，不在本轮实现。
