# P0-06c-DIAG 连接诊断与独立最终统计报告

日期：2026-11-22（北京时间）。开工fetch确认main/origin/main=acd1910，工作树干净。先提交ADR0011及本地契约76dd4af，再修改代码。本轮只做离线与本地自动测试，没有登录/启动阿里云WSS/TURN，没有真实桌面/摄像头采集，也没有要求用户重做双机；既有云端停用状态没有被本轮操作改变。

## 结论与边界

已实现严格候选的分侧脱敏诊断、接收端独立两阶段落盘、异常证据保留、关窗通知/缺失回退和即时停止顺序；完整回归及本机真实合成视频测试通过。**历史两次故障仍不能确定具体被拒绝侧/端点/类型，不声称已经定位或修复现场兼容性。** 已提交最小在线复现方案供独立审查，未申请或执行云端操作，P006c历史问题不追填PASS，不进入P0-07或Phase1。

| 层次 | 结果 | 范围 |
|---|---|---|
| 旧证据/旧版本源码离线审计 | 完成，根因未定位 | 两次失败及发布候选类型对照；不是在线复现 |
| 协议/权限/文件/生命周期单元及安全负例 | PASS | 80 Rust、32前端、12 Python；私有MF样本1项ignored |
| 完整构建、类型、lint、fmt、clippy及旧模式 | PASS | 35项命令，3项Edge回归；旧桌面3项另测 |
| 本机真实TLS/WSS/MF/SRTP/SCTP/Edge | PASS | 五轮、实际fsync后的独立接收JSON；生成图案，不采集桌面 |
| 新版嵌入WebView2/OS关窗/物理双机/云故障重现 | 未执行 | Native桥已编译，前端通知/回收及原生journal逻辑有自动覆盖；不当作新现场实测 |

## 旧失败能证明什么

审计脚本读取acd1910的direct_policy.rs、session_direct.rs及保留的两份失败JSON，记录SHA，输出historical-audit.json：

1. 当时两个集合都已取得，两个实际候选已通过旧UDP/IPv4及支持类型转换，错误发生于至少一侧精确`(socket,type)`不在对应验证集合。不在该返回点归因为集合不存在、非UDP或未知类型；不代表实际地址范围必然正确。
2. 两次本端公开统计都只有srflx/relay，成功轮另有host。webrtc/rtc0.21.1缓存源码表明selected pair读取ICE agent的候选索引，转换复制其address/port/type；本工程拿该值与经过publish_sdp过滤的应答集合比较。
3. 若实际本地host没有发布而对端使用srflx到达它，原有严格匹配会拒绝；离线构造了这个组合且仍拒绝。这是有数据支持的**可验证假设**，不是历史根因证明：旧失败pair为null、未记录私有host发布批准布尔，不能断言当时必然选了host或用户漏填了地址。
4. 失败源0帧/0控制事件、回收完成；DCEP open事件不等于控制权限开放。浏览器srflx/srflx不能代替原生被拒绝pair，重试成功也不能证明兼容性问题已消失。

没有通过将host与srflx视为相同、接受同IP不同端口、加入prflx或忽略SDP集合来修复。原UDP运行白名单文件未修改；增加集合scope绑定检查，只收紧跨会话组合，不扩大授权。

## 新诊断

原生Stats.direct_pair_diagnostic version1对local/remote分别记录固定reason、类型/协议/地址类别、集合来源/大小，以及同IP、同端口、同类型、同端点、完全相等计数；记录本次私有host发布批准布尔。原因区分缺失集合、scope/过期、协议/分量/类型拒绝、范围拒绝、同端点不同类型、集合内缺类型、端口/地址变化及无匹配端点。后两者只表示相对于当前集合的关系，不是假定的网络历史变化。

实际控制放行仍执行原精确匹配；诊断不能安装端点或授权。首次失败在清理后仍保留。输出无原IP/端口值、完整SDP、凭据或可跨会话关联的端点哈希。离线fixture输出单独标DIAG_OFFLINE_FIXTURE，不是两台电脑新实测。

## 独立最终统计与异常处理

修复旧p006_save只保存控制快照/周期样本、遗漏最后浏览器getStats的问题。复用同一个受限IPC的receiverEvidence可选参数；无新网络帧、任意路径输入或Tauri权限。原生检查本地接收角色、既有owner、版本、严格字段、数值/容量、固定错误码及记录顺序；文件create_new+sync_all，不覆盖已有证据。

- UUID-receiver-pre-close.json：浏览器PC/track关闭前落盘。视频及ICE从同一getStats报告读取，控制与呈现数在采样时读取，记录真实采样/记录时间。
- UUID-receiver-final.json：独立清理结果，保留最终可用快照及控制、track、PC关闭状态；原生nativeResourcesReleased在写入时由实际会话覆盖前端值。旧final仍兼容，不回填旧893/880帧。
- 采样失败时明确last_known并保留原采样时间；无旧样本则unavailable/空值。失败候选可以作为peerMatches=false的诊断落盘，但实时metrics及控制权限不放宽。安全错误不被正常停止码吞掉。
- 各清理步骤独立捕错，统计或写盘失败也继续关track/PC和停止原生任务。停止先同步revoke控制发送并立即发起原生停止，不让写盘等待延后撤销；晚返回的周期轮询不能覆盖终态或重新开始发送。测试证明通道尚保留供采样时发送已经停止。
- 原生关窗立即cancel并通知前端，限时等待其记录；无响应以原生unavailable替代，未知浏览器状态不写true。强杀/断电、持续磁盘故障不保证文件写成，必须按缺失处理。窗口消息桥未在本轮新做实际嵌入WebView2/OS关闭验收。

## 最终本机真实测试

同一物理Windows，localhost新临时IP-SAN TLS证书、两个真实原生WSS角色、现有Session/AMDh264Encoder/720p30/RTP、共享前端RelayProbe、Edge实际解码、可靠有序control.v1。未联系fixture里未使用的TURN占位地址：原生无TURN grant、测试PC工厂移除ICE服务器；实际源synthetic_nv12、ICE All、host/host并逐端精确验证。浏览器仅用强制合成设备与临时媒体权限公布物理接口，随即停掉合成流，不访问真实设备。生产WebView2未引入这些测试开关。

最终五轮actual计数、采样时间、两份实际磁盘JSON和资源状态在local及summary.json。正常完整轮300帧输入/编码/发送/接收/解码/呈现、100事件发送/原生接收/ACK/确认；两端角色/owner越权报告写入真实拒绝，所有回收通过。

发送端停止与接收端停止在100ACK后执行，统计保留实际短帧数，不写成完整300。getStats故障轮在真实100ACK后仅注入浏览器API拒绝，保留last_known原时间，不能当作真实物理设备故障。WSS断开轮真实终止本地TLS服务器连接，保留P006_WSS_CLOSED及原生exit1，再核对独立终态/回收；runner通过表示符合预期异常，不把它改成零错误正常退出。这两轮仍是本机真实传输加故障条件，不是新增双机或阿里云试验。

临时TLS配置/私钥与测试目录均已清理；未创建云端临时凭据。开发阶段一轮Edge测试因新event依赖首次优化触发Vite重载而失败，保留development/browser-optimization-reload.txt；显式预优化两个Tauri API后最终3项通过，不改WebRTC校验。

停止顺序修正后的本机复核中，一次故障用例在注入故障之前等待100ACK超时，未取得完整失败快照；runner日志保留development/wait-for-ack-timeout.txt，不能推定是网络或产品哪一项原因。后续runner在提前结束/异常时也写失败JSON，并将每轮退出码和结果完成时间一起验证，避免把残留旧成功JSON当成新失败轮结果。独立重跑及最终五轮均通过，超时日志不被改为PASS，也不据此宣称已解决所有时序问题。

## 回归与复现

原始命令/退出码在automatic/results.json，各命令日志保留；包含全workspace/all-features单元、默认/全特性clippy -D warnings、fmt、旧P004a/LAN/P005/RelayOnly兼容、新DirectPreferred生产特性clippy/优化构建、两个操作员probe及默认/占位/缺批准拒绝、前端全部检查和Python回归。80 Rust通过、私有MF1 ignored；旧桌面3项另测、32前端、3 Edge、12 Python。为避免重新产生大量debug缓存，本轮明确禁用增量并关闭dev/test调试符号，没有删测试来省空间。

```powershell
# 完整离线回归；不启动任何云服务或采集
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p006c-diag-checks.ps1
# 显式本机合成传输，替换为本机实际RFC1918接口
node scripts/p006c-diag-local.mjs --local PRIVATE_INTERFACE_IPV4
# 保留文件的离线一致性/隐私/清理核对
python scripts/p006c-diag-evidence.py
```

Python可用REMOTEHUB_PYTHON指定；本地TLS fixture需要已有cryptography包，细节沿用P006C_VIDEO_RUNBOOK.md。summary.json保留源码摘要和Windows程序/操作员probe SHA，区分编译结果与本机实测，不把当前构建称为已执行新双机验收。

## 待审查

历史具体故障仍未定位；最小在线方案见P006C_DIAG_MINIMUM_ONLINE_PLAN.md，只是待授权材料。本轮不请求用户立即重测，不自动启动阿里云。旧现场证据及本轮新诊断并列保留，下一步是否另行在线复现、是否进入P0-07由用户审查决定；本轮完成后停止扩展。
