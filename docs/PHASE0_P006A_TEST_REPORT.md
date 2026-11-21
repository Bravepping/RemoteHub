# P0-06a TURN/ICE 准备测试报告

后续记录：以下为5e05c09准备交付时的历史结果；用户之后提供目标并另行授权，G6已真实通过，当前状态升为infrastructure-verified，见[独立G6实测报告](PHASE0_G6_TEST_REPORT.md)。不将后续实测倒填到历史离线结果。

2026-11-21，状态：**implementation-ready**。G1–G5/G7/G8完成，G6 **blocked / 未执行**。不是infrastructure-verified，不是P006父任务完成。没有访问阿里云主机、安装coturn、修改安全组/防火墙或进行公网端口探测，没有进入P006b/c、Phase1或实际输入开发。

## 基线与交付范围

开工核对main=079275d1875e78bfbc811a429a52e4dbb539cb5a、工作树干净，读取AGENTS/TASKS/原设计、ADR0004/0005、CONTROL_V1和P005报告/真实双机证据。新任务确认P005阶段性通过，原P004录屏差异继续保留。ADR0006、ICE/rendezvous契约和实施计划先提交3a8f67f3b3099468f1924f49608c3bb8e6d4c834，再修改源码；最终复核fetch的origin/main仍为079275d。最终交付SHA以Git记录和包内BUILD_INFO为准，避免文档自引用。

复用webrtc/rtc **0.21.1**，逐项核对本机锁定库实际源码的RTCConfigurationBuilder/RTCIceServer/RTCIceTransportPolicy::Relay、candidate事件、sctp.transport().ice_transport().get_selected_candidate_pair和pair.local()/remote()。原生TURN gather明确跳过secure及非UDP URL，因此本轮只允许批准的数字IPv4 TURN UDP。未升级既有锁定包；新增workspace issuer和hub-webrtc可选zeroize依赖，见source-audit.json。

新增server-only hub-turn-credentials：从受限服务端环境载入共享密钥，HMAC-SHA1/Base64仅用于coturn REST兼容。username=expiry:session_uuid，默认TTL300、允许60..600秒、时钟偏差≤30秒且不超过批准会话；检查受认证的可信上下文、双方批准、realm、撤销、会话/角色/连续序号、每角色两次签发上限。没有HTTP/WSS签发端点；trusted context不是远端传auth=true。自有秘密缓冲区清理、Debug脱敏；桌面all-feature依赖闭包不含此crate。

hub-webrtc新增独立RelayGrant/IceConfig与RelayRuntime，区分物理TURN服务端IP:port与逻辑relay候选IP/端口范围。物理UDP双向精确端点、短期限和停止守卫，禁DNS/TCP；Relay SDP保留视频+application数量、BUNDLE/mid、H264、SHA256指纹、UDP/relay/地址范围、大小和候选数量检查。rendezvous-probe离线Verifier对完整SDP及方向/nonce/seq/期限做HMAC-SHA256验证、限流/大小并失败关闭。WSS传输、远端审批和长期设备身份尚未实现，不能用此模块宣称公网认证完成。

默认关闭p006-turn-probe；TurnProbe/CLI不采集、不加视频轨、不接收远端SDP、不发control事件。只有明确批准目标并提供新鲜短期票据才会触发真实库gather/Allocate，最多10秒gather+3秒close；本轮**没有执行可连网配置**。配置和诊断使用真实原生Relay及浏览器relay属性，未连接selected pair/RTT/丢包保持null。DirectPreferred只保留契约并明确拒绝执行。TS工厂未接入公网页面/IPC，普通构建不开放远程会话。

## 实际执行的自动检查

最终原始UTF8输出和退出码：`artifacts/p006a/final-checks/results.json`及同目录各txt；默认基线细项：`artifacts/p006a/baseline/verification.json`/txt。`scripts/p006a-checks.mjs`可复现17组检查，15组exit0，两个预期拒绝分别exit2/1，全部达到预期。

| 检查 | 实际结果与边界 |
|---|---|
| pnpm verify | lint、typecheck、13前端测试、Vite生产构建、fmt、默认clippy及Rust测试通过；默认45 Rust，未启用桌面实验测试 |
| all-feature clippy | workspace/all-targets/all-features，-D warnings，locked/offline，通过 |
| all-feature Rust test | **54通过、0失败、1 ignored**；6 capture +12 encode +4 issuer +30 webrtc +2 desktop；私有MF AU测试未重新采集或执行，不算PASS |
| pnpm test:e2e | **3项Edge通过**；旧预览、真实video/control SDP及真实RTCPeerConnection relay配置/关闭；不采集、不gather、无云连接 |
| feature独立check | p004-probe、p004-lan、p005-control、p006-turn-probe各自通过 |
| 原生默认桌面build | cargo build -p remotehub-desktop --locked --offline通过；没有新增UI人工验收，不替代旧硬件播放证据 |
| coturn离线策略检查 | 模板、3 Python测试、bash语法与wrapper通过；不是coturn官方解析器或Linux服务运行结果 |
| Allocate CLI优化构建/帮助 | profile probe构建成功，--help exit0、未发网络请求 |
| CLI默认/占位拒绝 | 无参数exit2；占位JSON在IP反序列化前置失败exit1/PROBE_CONFIG_SCHEMA，未建PC、未连云 |
| 源码/依赖检查 | source-audit.json：既有锁包版本/checksum不变、issuer不进desktop、CLI需feature、保护路径diff为空、实际输入API调用0 |

最初一轮静态检查发现新增限流测试使用unwrap_err隐含要求Negotiation:Debug，已改为直接匹配错误，不给SDP相关类型增加Debug；最终重新执行完整17组检查通过。Windows pnpm固定命令shim的Node DEP0190、Edge NO_COLOR/FORCE_COLOR提示是工具输出，未隐藏，也不作为云功能错误。没有完整RustSec/SBOM审计。

## 新增测试覆盖及证据类型

- issuer 4项：RFC2202 HMAC-SHA1已知向量、独立Python标准库coturn向量；未认证/未批准/单方批准/过期/撤销/跨会话、TTL/时钟/realm/序号/签发配额与Debug脱敏。
- ICE配置2项：真实库配置getter为Relay；禁用模式、过期、超TTL、缺/畸形凭据、端点替换、DNS/TCP/TLS、私网/保留relay、未批准、loopback本机接口和超长JSON拒绝。公开IP仅作为**不连网数据fixture**，不探测第三方服务器。
- socket集成1项：真实127 UDP socket收发验证批准端点可通、同IP错端口RX丢弃/TX拒绝、撤销后RX/TX拒绝、到期拒绝、DNS禁止。这是本机socket边界，不是TURN Allocate。
- native probe生命周期1项：实际库创建与关闭，不调用gather；relay候选0、selected pair/RTT/loss/耗时null、close成功，不填伪实测值。
- relay SDP1项：有效fixture、错误IP/端口/候选类型/指纹、额外音频m-line、过量候选/超长/过期拒绝，原Loopback/LAN仍拒绝该relay SDP。
- 离线信令2项：完整MAC、重复/跳序/方向反射、指纹/端口被信令方替换、过期/跨会话/nonce/类型/超限TTL/超大payload、10/s限流和失败后关闭。是直接调用验证器，不是上线WSS连接。
- 前端新增2项：真实配置工厂relay属性，未启用/未批准/过期/无效批准期限、TCP/TLS/端点替换/未实现模式拒绝，空stats保持null。Edge另加1项实际浏览器属性检查，未产生ICE候选。
- Python3项：空env不能渲染、正例EIP映射及拒绝覆盖、危险peer覆盖/no-auth/缺安全标志/无配额/端口错误/错映射/短密钥/日志配置/命令注入负例，以及错误输出不泄露秘密fixture。

旧真实本机SCTP100事件/ACK和协议/授权/限流/重复负例随workspace重新执行通过。DXGI、MF、RTP/H264、scope、PinnedRuntime、session/lan/control实现及Tauri旧入口、P004前端代码均与基线无diff。没有在本轮重新采集桌面或重复两台PC实播；P004/P005历史实测是兼容性参考，不能填成新TURN证据。

## G1–G8验收矩阵

| Gate | 状态 | 证据与限制 |
|---|---|---|
| G1 HEAD/工作树/版本 | implementation-ready | 开工079275d、ADR先行3a8f67f、最终fetch复核、source-audit；0.21.1锁定 |
| G2 ADR/策略契约 | implementation-ready | ADR0006/P006_ICE_CONTRACT及隔离实现；DirectPreferred未启用 |
| G3 coturn准备 | implementation-ready | deploy/coturn模板/env/校验/渲染/systemd示例、人工部署/回滚；目标包解析/运行未测 |
| G4 短期票据 | implementation-ready | 已知向量和负例自动日志；无公网签发服务 |
| G5 旧能力回归 | implementation-ready | 默认完整verify、all-feature54项、独立feature、原生build与保护路径diff为空；无新双机播放 |
| G6 真实云功能 | **blocked / 未执行** | 未提供核实的ECS目标/发行版/映射/权限，未获登录和部署/测试授权；正确/缺失/错误/过期票据Allocate、STUN Binding、目标coturn日志均未取得 |
| G7 默认无公网入口 | implementation-ready | CLI required-feature、无新Tauri命令/页面/监听，默认和占位运行前置拒绝 |
| G8 文档/任务/证据 | implementation-ready | README/TASKS/本报告/runbook、脱敏自动证据及Git交付；未测内容明确 |

## 已知限制、风险与下一步

没有实际Linux环境检查、coturn官方配置加载、systemd启动、ECS路由/EIP/防火墙验证、匿名/错票/过期票拒绝和带宽/配额效果实测。模板故意不含真实IP/域名/秘密，必须取得独立授权后核实目标包版本、现有服务与具体变更，再手工部署。没有Docker不阻塞Linux官方包方案。

TURN REST凭据有效期内可复用，不能单票即时撤销；session UUID不会让coturn执行应用ACL，票据持有者仍可能请求服务器允许的其他公网UDP目标。realm不在REST HMAC输入内、原生库接受challenge realm而非指定expected realm；issuer拒绝错realm请求不能冒充云端跨realm负例。应隔离secret/realm并在真实G6记录限制。coturn max-bps/bps-capacity按**字节/秒、入出分别**，不是账单硬封顶；模板配额需实测。

自有Rust缓冲区会清理，但SDK副本/JS字符串不保证清零；CLI短期私有文件须受限ACL并及时销毁，不记录秘密/完整SDP/画面。无新长期凭据配置、无人值守、键鼠注入或系统安全设置改动。日志默认不持久保存流量信息；排障日志需独立批准、短期私有、限额并脱敏。

下一步先审查本轮准备实现；取得真实目标资料和明确授权后仅完成G6，再单独安排P006b不同网络双机relay视频+100模拟事件。P004录屏证据差异未替用户更改，Phase0正式父项仍4/7。本轮停止扩展。

依据：[coturn当前配置](https://github.com/coturn/coturn/blob/master/examples/etc/turnserver.conf)、[TURN REST兼容与撤销限制](https://datatracker.ietf.org/doc/html/draft-uberti-behave-turn-rest-00)、本机锁定webrtc/rtc0.21.1源码。coturn Wiki自述过时，未来以目标包help/man和实际运行验证为准。
