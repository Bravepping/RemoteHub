# P0-07-A：Phase 0 技术总评与退出条件审查

审查日期：2026-11-22（北京时间）。开工确认 GitHub main 与本地均为 `246dcea74f120897b47c2b6f00a7980f266afb69`，工作树干净。本轮交付的是技术评审与证据汇总，没有运行新媒体实验、启动云 WSS/TURN、采集桌面、要求用户重做双机或开发 Phase 1。

## 结论

**当前路线的核心可行性成立；建议继续保留 WebRTC 架构，但暂不签署正式 Phase 0 退出。** 两台真实 Windows PC 已有连续视频、同一 PeerConnection 上的模拟控制、跨网 TURN 的直接证据，不能再描述为“云端视频和物理双机均待测”。同时，技术可行性通过不自动消除原验收证据偏差或连接可靠性问题。

正式退出目前有两项待关闭：P004 原录屏缺失的证据例外尚未获明确接受；P006c 两次真实 `DIRECT_SELECTED_PAIR_NOT_APPROVED` 仍未归因，最新诊断和独立终态只完成本机自动验证。前者是验收材料偏差，后者是正常使用连接可靠性的未决风险；它们都不构成已证实的架构不可行，也不应被重试成功或单元测试数掩盖。

本轮 `P0-07-A` 总评完成，`P0-07` 父项仍待退出决议。原正式父任务计数保留4/7（P001/02/03/05）；P004/P006/P007不因新写一份总结而自动done。不进入Phase1。

## 证据口径与版本

原规范[§11 Phase 0](RemoteHub_完整开发文档_v1.0.md)最低退出条件是“两台真实Windows连续视频 + 控制消息 + TURN可用”；P004产物另外明确要求“录屏与stats”，P007要求成功/失败、性能与备选方案决议。以下按**原始实测范围、程序版本、独立证据能力**汇总，不把同机两个PC对象叫作两台物理机，不把合成源叫作桌面采集。

原始资料在各报告及其artifacts引用。新增[证据索引](../artifacts/p007a/evidence-index.json)从固定基线Git对象重新读取、校验关键计数、按原始云事件复算Allocation分类，并保存原文件SHA256。它验证保留材料的一致性，不是对历史物理环境的重新测量，也不是第三方见证。索引中的`reviewJudgments`是本轮评审判断，与`observations`实测字段分开。

| 关卡 | 已通过的真实证据 | 主要来源与边界 |
|---|---|---|
| P001 / P002 | 原生Tauri/WebView2窗口及IPC；DXGI主屏2560×1440、30帧/1059ms、0超时 | [首轮报告](PHASE0_TEST_REPORT.md)、[窗口观察](../artifacts/native-window.txt)、[采集数据](../artifacts/capture-report.json)。28.315是桌面更新观察FPS，不是视频FPS；Windows10实测，Windows11未验收 |
| P003 | AMD硬件MFT真实桌面720p30、10秒300AU、0丢tick；独立完整解码300帧、色条/PTS/GOP通过；软件MFT另有2秒60帧 | [报告](PHASE0_P003_TEST_REPORT.md)、[硬编记录](../artifacts/p003/hardware-encode-report.json)、[解码验证](../artifacts/p003/decoder-validation.json)。不代表所有GPU、驱动或长稳 |
| P004a | 同机原生WebView2，五轮450帧及一轮900帧全收发/解码/呈现；主动停止及播放中关窗回收 | [报告](PHASE0_P004_TEST_REPORT.md)、[逐轮汇总](../artifacts/p004a/native-session-summary.json)。本机loopback不是双机；关窗后的JS数字未取得则保持缺失 |
| P004b | 两台有线Windows10，v2完整30秒900输入/编码/发送/接收/解码/呈现，1280×720、host/host、丢包0；两端停止/关窗/拔线回收 | [报告](PHASE0_P004B_TEST_REPORT.md)、[完整轮](../artifacts/p004b/full-30s.json)、[清理轮](../artifacts/p004b/cleanup-runs.json)。无录屏；B独立关窗文件未收取，部分清理由源端统计+用户确认支持 |
| P005 | 两机同一视频PC的真实control.v1，两个完整轮及四个清理轮每轮100发送/Rust接收/ACK写入/B确认，序号1..101、异常0，视频持续变化 | [报告](PHASE0_P005_TEST_REPORT.md)、[完整轮](../artifacts/p005/dual-pc-full.json)、[重复轮](../artifacts/p005/dual-pc-repeat.json)、[清理轮](../artifacts/p005/dual-pc-cleanup.json)。完整源900、B末周期893，非B最终900；仅模拟输入 |
| G6 / P006a | 真实云STUN/Allocate与两次Windows relay gather；11项协议/危险peer拒绝验证 | [G6报告](PHASE0_G6_TEST_REPORT.md)、[实际协议结果](../artifacts/p006a/g6/wire-results.json)。基础分配不能替代视频或selected pair |
| P006b | 宽带与手机热点两物理机，两个完整轮各900帧全收发/解码/呈现；5轮真实双方relay/relay，每轮100条模拟控制一致、异常0 | [报告](PHASE0_P006B_TEST_REPORT.md)、[统计](../artifacts/p006b/two-pc/statistics.json)。源端停止194帧、B关窗196帧、断网244帧提前回收。B主动停止已由用户接受、补测跳过，独立日志缺口不追填 |
| TURN生命周期 | 六例真实云/Windows：5例Refresh(0)即时释放、1例实际600秒租约超时；同PID下判断，原租约+15秒无残留 | [独立报告](TURN_ALLOCATION_LIFECYCLE_TEST_REPORT.md)、[逐例结果](../artifacts/turn-lifecycle/results.json)。不是靠停服务清零；Edge data-only不等于所有WebView2视频负载 |
| P006c首步网络 | 同一物理Windows上两个原生PC：直连、直连受阻回退、TURN受阻直连三例100/100/100；双路受阻0消息、明确超时并回收 | [报告](PHASE0_P006C_TEST_REPORT.md)、[回退](../artifacts/p006c/cloud/fallback.json)、[双路断开](../artifacts/p006c/cloud/all-unavailable.json)。实际云TURN，但无视频、无双机；故障为进程内真实丢包 |
| P006c云视频 | 真实云WSS/TURN + 单物理Windows/MF合成源/Edge，ICE All的host/host与受阻后relay/relay，两轮各300帧全链、100控制一致 | [报告](PHASE0_P006C_VIDEO_CLOUD_TEST_REPORT.md)、[正常](../artifacts/p006c-video/cloud/auto.json)、[回退](../artifacts/p006c-video/cloud/fallback.json)。9分配即时释放、原租约+宽限无残留；不是新物理双机/桌面/WebView2实验 |
| P006c物理双机 | d805ecb程序，真实桌面直连及热点中继成功轮：各900源帧、720p、100/100/100/100，用户看到变化画面；8分配即时释放后才停服 | [报告](PHASE0_P006C_MANUAL_TEST_REPORT.md)、[直连](../artifacts/p006c-manual/lan/desktop-direct.json)、[热点](../artifacts/p006c-manual/hotspot/desktop-relay.json)。原生直连host/srflx、浏览器srflx/srflx；B末周期893/880，独立final缺失。另两次拒绝必须保留 |
| P006c-DIAG | 246dcea代码的35项完整回归：80 Rust/32前端/3Edge/12Python，私有MF1项ignored；5轮本机真实TLS/MF/RTP/SCTP/Edge及两阶段独立文件 | [报告](PHASE0_P006C_DIAG_TEST_REPORT.md)、[摘要](../artifacts/p006c-diag/summary.json)。正常完整轮300全帧/100ACK，停止/统计故障/WSS断线符合各自预期；无新版云/物理双机或嵌入WebView2关窗实测 |

表中不同阶段/版本的用例数不相加成“当前累计覆盖率”。d805ecb的物理双机成功不能代替246dcea新增终态桥的现场验收；246dcea的本机自动通过不能追补旧893/880。P006b历史未归属socket仍未追溯定性，后续独立生命周期证明其测试环境的行为，不能倒推旧socket来源。

## 性能与可靠性可作何判断

P003建立了单机编码性能基线：输入提交至AU取出p50/p95/max为4.915/13.395/36.320ms，含异步等待；CPU归一16线程约2.872%、峰值Working Set约113.285MiB。CPU缩放/转换p95为12.051ms，是后续优化候选。这些只来自10秒编码实验，不能当作双机全链长期CPU/内存或用户操作延迟。

720p30由源时间戳/时长与接收解码呈现计数共同支持；完整轮观测丢包/冻结0是该轮事实，不代表任意网络零丢包。不用视频`currentTime`、首帧发现时间或无有效RTT测量的0推算延迟。真正的屏幕变化到呈现、输入到可见反馈p50/p95/p99未测；没有Sunshine/Moonlight同机同场景对照、1080p60或游戏体验结论。

网络首步回退34.873秒、TURN不可达但直连可用22.051秒，说明收集/等待可能影响连接体验；后来云视频首次发现解码2364.5/3040.2ms包含另一套协商/采样和拓扑，不能作同条件性能提升对照。没有多NAT/多网卡/跨运营商覆盖或有统计意义的成功率分母。P006c物理双机保留两次拒绝和两次成功，是这几次操作的样本，不据此发布“连接成功率50%”。

## 技术路线与兼容路线的取舍

“最初兼容路线”需分清使用背景与交付契约：原规范§1.2已写明**不复刻GameStream、不实现Moonlight兼容协议、不创建TUN通用VPN**；§1.3直接选择WebRTC。当前没有Sunshine/Moonlight互联兼容性承诺或测试，不应把实现标准WebRTC称作已完成它们的协议兼容。

外部能力核对只使用项目官方资料，核对日期2026-11-22：Sunshine是Moonlight服务端并提供GPU/软件编码与配对；Moonlight Qt是GameStream/Sunshine客户端，含硬件解码及更多编解码/音频/手柄功能；EasyTier提供mesh VPN、NAT穿透、选路与P2P失败转发。这些是各项目公开能力，**不是RemoteHub在本轮跑出的对比测量**。[Sunshine官方说明](https://github.com/SunshineStream/Sunshine#readme)、[Moonlight Qt官方说明](https://github.com/moonlight-stream/moonlight-qt#readme)、[EasyTier官方说明](https://github.com/EasyTier/EasyTier#readme)

| 路线 | 价值 | 代价与尚无证据部分 | 本轮建议 |
|---|---|---|---|
| 当前DXGI/MF + 标准WebRTC库 + WebView2 | 单个Tauri主进程承载产品功能；同PC视频/控制、短票和授权边界已有实测；与既定规范一致 | 自己维护媒体集成、候选策略、统计和生命周期；目前UDP4、H264/有限硬件，严格候选策略可能拒绝本应可连的组合；延迟与大规模兼容性无保证 | 保留，先关闭现有可靠性/退出差异；不重新造ICE/SRTP/SCTP |
| 一个安装器包装Sunshine + Moonlight + EasyTier | 可复用其既有串流、客户端和组网能力；减少部分媒体功能重写 | 仍需管理多进程、配置/信任/权限、升级与退出；虚拟组网要另评估驱动/路由影响。单安装包技术上可实现，不等于当前单主进程约束已满足；没有本项目集成样机或A/B性能测试 | 可作为重新定范围的备选，不能偷偷添加为运行依赖 |
| 嵌入/派生Sunshine/Moonlight核心 | 可更深复用既有媒体/输入实现，理论上统一外观 | 需处理C++/Qt/FFmpeg等构建和接口边界、生命周期、后续上游合并及许可；维护成本取决于具体方案，当前未验证 | 先独立原型/许可评审和ADR，再谈迁移，不能宣称成本已更低 |
| 自行重写GameStream兼容协议及通用组网 | 可能满足与原生态互联的新增目标 | 目前无需求合同、无兼容用例、无互操作证据；显著扩大协议/安全/驱动维护面，不能用它直接解释或修复候选拒绝 | 本阶段不采用 |
| 保持WebRTC、改原生D3D呈现 | 若未来证实WebView2持续呈现开销不可接受，可局部替换渲染 | 新解码/纹理/窗口路径仍需证明；当前未建立端到端延迟对照 | 沿用原规范降级方向，只在测量触发时另行设计 |

上述成本/迁移判断是基于现有代码边界的工程推论，未进行工期或性能基准。“单主进程”指产品原生主程序，不代表WebView2没有自身子进程。webrtc 0.21.1的[官方manifest](https://github.com/webrtc-rs/webrtc/blob/v0.21.1/Cargo.toml)声明MIT/Apache-2.0；Sunshine/Moonlight Qt仓库声明GPL-3.0，EasyTier声明LGPL-3.0，分别见上述官方仓库许可栏。只能据此说明依赖和分发审查范围不同，不能作“当前方案完整合规”或“GPL不能自用/不能整合”的法律结论。现有[依赖说明](../THIRD_PARTY_NOTICES.md)仍不是完整SBOM或最终分发许可审计；本轮没有复制上述项目代码或引入二进制。

## 两项关键退出差异

### E04：原录屏证据偏差

功能已通过；所缺的是原规范明确要求的录屏产物。程序和用户当时明确选择不保存画面，不能追认成已录屏，也不能以P003私有编码文件代替“双机局域网持续播放录屏”。现有stats、真实原生计数和用户现场观看足以支撑限定环境的视频可行性，对画面内容/主观流畅度的事后独立复查能力较弱。

**建议接受替代证据，而非要求默认补录。** 但这只是本轮审查建议，不代替用户批准验收例外。若用户明确接受，新增一条“按统计+现场确认替代录屏”的有日期决议，可将P004按例外收尾；旧报告的“未留存录屏”永远保留。若坚持原产物契约，则这项继续阻塞正式签字，只在新授权下补做最小必要材料。本轮不请求用户现在采集或重测。

### R06：P006c连接可靠性与独立终态

两次实际拒绝发生在至少一侧原生selected `(endpoint,type)`不在已批准SDP集合，0源帧/0控制，回收完成；是安全失败，不能用放宽UDP白名单、候选类型别名或绕过授权来“修好”。失败时本端发布集合仅srflx/relay，成功多host，是线索；实际失败pair未保存，不能证明“未发布host被选中”就是历史原因。

DIAG补上分侧原因/相等计数、首次失败保留及接收端独立pre-close/final，**提高了下一次诊断能力，没有证明旧故障已修复**。五轮本机合成成功是限定本机范围；开发期另一次100ACK等待超时仍未归因，保留[日志](../artifacts/p006c-diag/development/wait-for-ack-timeout.txt)，不据此编造同一根因或已解决结论。

**默认建议：在正式Phase0退出前关闭这一可靠性决议。** 可接受的关闭方式有二：

1. 经单独授权按[最小在线方案](P006C_DIAG_MINIMUM_ONLINE_PLAN.md)取得可解释的新诊断，定位是正确安全拒绝/配置适配还是合法候选兼容性缺陷；如需改码，先设计再回归，证明授权内成功和授权外拒绝，收取新版接收端独立终态。不能要求一定复现每次旧历史，只能如实限定新结论。
2. 若仍未复现或用户选择先结束Spike，由用户明确接受“最低可行性通过、连接可靠性未闭环”的有限范围退出，保留独立问题、影响范围/回退方式及后续关闭责任。它是风险例外，不能把P006c全环境可靠性、旧失败或新双机终态改成PASS。

本轮两种方式都尚未发生。不会为取得例外擅自标done、默认禁用直连或开放未批准候选。原P006b接收端主动停止补测已经用户明确免除，**不重新作为退出阻塞或要求重复操作**。

## 退出判定矩阵与后续边界

| 条件 | 审查判定 | 对退出的影响 |
|---|---|---|
| 两台真实Windows连续视频 | 已有P004b/P006b实测 | 最低可行性满足；不是任意硬件/网络保证 |
| 同一视频PC的真实模拟控制 | P005/P006b/P006c物理轮100条及ACK证明 | 最低可行性满足；实际SendInput不属于Phase0完成内容 |
| 真正TURN媒体路径 | P006b双方relay/relay、P006c热点成功 | 最低可行性满足；不是只看配置 |
| 分配释放与停止安全性 | 独立6例生命周期、后续云9/物理8分配观察；旧操作缺口保留 | 测试范围有证据；不承诺所有丢包都即时释放，不归因旧未关联socket |
| P004录屏产物 | 缺失，替代证据未正式豁免 | E04待明确接受或另行补证；当前阻塞形式签字 |
| P006c合法连接兼容/可靠性 | 两次真实拒绝未归因；最新桥尚无新双机终态 | R06待诊断闭环或明确有限风险接受；当前不签完整通过 |
| P007技术取舍与成功/失败汇总 | 本报告+ADR0012完成本轮评审 | 评审材料完成，最终退出决议仍待审查 |
| Win11、多GPU/驱动、HDR/旋转、20分钟长稳、1080p60、端到端延迟、TCP/TLS TURN/IPv6、ICE restart/切网恢复 | 未验收或未实现 | 不凭空增加原最低退出条件；列为后续兼容/性能目标，不作当前产品能力承诺 |
| 持久身份/DPAPI/Ed25519/SQLite、正式ACL、实际输入/DPI、安装/签名/SBOM/完整安全审计 | 未完成 | 属于后续产品阶段；Phase0退出也不等于可发布或可无人值守 |

当前源码不会实际注入键鼠。`control.v1`以可靠有序20Hz实验链路运行，不代表未来高频移动控制的最终最优策略。现有WSS是隔离短时实验服务，不是正式设备身份服务；UDP/TURN受阻的企业网络仍可能无法连接，不承诺TCP/TLS替代路径。

云端最后一次被实际核对的停止状态为11月22日09:53:20；后续DIAG及本轮没有重新登录检查，不能把历史停用证据称作“本轮实时云审计”。旧邀请/激活配置已清理，未来测试须新授权、新限时配置；本轮不改业务服务、安全组、防火墙或自启。

## 本轮检查与交付

先记录[ADR0012](ADR/0012-p007a-phase0-exit-review.md)，再生成本报告、更新README/TASKS和交接。执行`scripts/p007a-review-evidence.py`及`--check`，读取固定Git基线的72份材料、汇总13组证据、复算关键计数/Allocation并生成本轮新索引；旧报告/统计不写回。另做Python语法、相对文档链接、历史与业务源码无变化、Git空白检查；实际输出保存在[verification](../artifacts/p007a/verification.json)。

复核命令：`python scripts/p007a-review-evidence.py --check`；本轮文档/范围检查入口：`python scripts/p007a-review-checks.py`。后者只更新本轮verification，不触碰旧材料，也不运行任何云端操作脚本。

本轮没有修改业务代码、依赖、协议或权限，因此没有重跑编译、lint、clippy、媒体/网络完整回归；最新完整回归仍是DIAG的35项历史结果，不把它冒称本轮新测试。没有产生新版实验包或有效凭据。完成提交后停止扩展，等待对E04、R06及正式退出范围的审查；是否安排P0-07后续关卡或进入Phase1由之后明确决议决定。
