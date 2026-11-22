# P0-07-B 准备交付检查点（E04已批准，R06在线未授权）

- 用户明确接受P004归档真实统计+本人观看替代录屏，限定既有阶段范围；新增PHASE0_P007B_DECISION.md，P004父项按例外done，正式父项5/7。旧未录屏事实/旧报告/规范/统计不改。R06未豁免，P006/P007及Phase0退出仍HOLD，不进Phase1。
- 基线main dbea0e3干净，先ADR0013/契约9c0b87e。新p007b-diag专用Tauri gate固定8秒/DirectPreferred，复用load_operator_synthetic(false)、Session/MF/NV12/RelayProbe/认证WSS/DIAG。只能合成源，旧capture handler不注册，无桌面开关/故障开关/TLS或候选绕过。新r06-private.json最多1200秒，包不带有效配置；原生双向同意保留。
- 41项完整默认/全feature/专用feature安全回归全过：82 Rust/34前端/3Edge/12Python，私有MF1ignored；旧桌面及专用3项单测分别保留。9项PowerShell/打包/隐私/SHA正负例过。五轮8秒本机localhost TLS/Session/MF/Edge/control合成真实传输过；正常240全帧/100ACK，短停止与故障保持真实计数/last_known；它们不是打包原生WebView2 GUI/两物理机/新云验证。无实际桌面、无阿里云登录/启动。临时TLS/测试目录已清理。
- 原生报告专用目录artifacts/private/p007b-r06，时间/分侧原因，独立receiver-pre-close/final沿用受限journal。Export-R06Summary.ps1先验证本包程序hash，再按固定白名单导出；错误/未知/缺失不补真值。Verify-SHA256.ps1离线核对。package-p007b.py只复制白名单，验证专用EXE身份/当前验证SHA/测试/产品源码已提交。程序17,053,696字节 SHA61b6e4815e7339ca0823c6c304593368f2d2a28ba8bd1b666a0eea4169df54c5。
- docs/P007B_R06_RUNBOOK/ONLINE_PLAN及PHASE0_P007B_TEST_REPORT已准备；未来最多20分钟/4alloc/最多两轮8秒，重点候选原因与独立终态，不要求100条完整串流，不重复旧停止补测；临时专用WSS/TURN启动仍需用户新授权。先准备待激活ZIP及SHA后提交GitHub，等审查，不自行扩展或在线复现。包与最终源码SHA以delivery.json/本轮交付为准。

# P0-07-A 技术总评检查点（只做评审，不启动测试服务）

- 基线GitHub/main 246dcea干净；本轮汇总真实证据、比较WebRTC与Sunshine/Moonlight/EasyTier备选，更新README过时状态。没有改业务/依赖/权限/原报告/原统计，没有云登录/启动、桌面采集或新双机，旧授权不能复用。
- 新docs/PHASE0_P007A_REVIEW.md及ADR0012：最低双机视频+模拟控制+TURN条件已有实测；建议保留现有架构，正式Phase0退出暂缓。E04录屏缺失建议明确接受stats+现场确认替代，但尚无验收例外；R06两次严格候选拒绝未归因，新DIAG只本地验证。需诊断闭环或用户明确接受有限风险，不能重试成功就关闭，也不要求现在重做旧P006b停止补测。
- 当前路线从v1.0基线即为标准WebRTC，并非已完成GameStream/Moonlight互联；官方资料只核对公开能力/许可，不冒充A/B性能。记录输入至AU p95 13.395ms等编码基线，端到端延迟/连接成功率/Win11/长稳未证实。
- 新scripts/p007a-review-evidence.py离线从固定Git对象读取72源文件/13组证据，并复算6+9+8分配历史分类，不调用旧会改写summary的脚本；仅写artifacts/p007a。历史物理源900与B末周期893/880不追补；d805ecb物理成功不代替246dcea新版现场验收。
- P007A子项材料完成，P004/P006/P007父项仍待审查，正式父项4/7，不进Phase1。README已修正云视频/双机待测、当前回归和旧包版本的过时口径，旧报告原结论保持。验证/最终SHA以本轮记录和Git为准。完成Git交付后停止扩展。

# P0-06c-DIAG 交付检查点（本轮仅离线/本机，无云授权）

- 用户本轮要求诊断两次DIRECT_SELECTED_PAIR_NOT_APPROVED、完善接收端独立getStats落盘/异常证据、完整回归及Git交付。main acd1910干净；ADR0011/契约先提交76dd4af。没有登录/启动云WSS/TURN，没有真实桌面/摄像头/新物理双机，不请求用户重测，不进入P0-07/P1；既有云停用授权结束，未来需新具体授权。
- 审计旧源码及保留JSON，能证明至少一侧实际(endpoint,type)不在验证集合；两次已发布本端集合缺host，成功多host。原生getSelected读取agent候选索引并复制字段，未发布host被选中的假设可离线复现拒绝，但旧失败pair/批准布尔缺失，具体侧/地址/端口/类型不能定位，不能宣称现场修复。historical-audit.json保留SHA/结论，最小20分钟4alloc在线方案仅文档待审查，未执行。
- 新direct_diagnostics按真实native pair和grant分别记录固定reason/类型/协议/地址类别/集合来源/大小与相等关系计数、private_local_publication_approved；首次失败不被清理覆盖，无原IP/端口/SDP/secret/端点hash。旧精确selected保持，scope交叉/协议/分量/范围/类型更严格；运行UDP白名单不改，未加入prflx或host-srflx别名放行。Session策略集成负例保持0帧/控制权限关闭。
- 新receiver_evidence本地journal严格schema/owner/接收角色/大小/固定错误码，p006_save可选receiverEvidence且browserClosed可选；pre-close及receiver-final各create_new/fsync，原生回收真值覆盖前端。视频/ICE同份getStats；读取失败保留原时间last_known或unavailable。Stop先同步control.revoke和发NativeStop，日志不能延后撤销；晚poll停后退出不改终态。各步独立catch，失败仍关track/PC/native；原生关窗立即cancel并emit、最多25秒等JS，没响应写unavailable。新OS嵌入WebView2/关窗未实测，不能夸大单机Edge覆盖。
- 最终35项完整回归全部达预期，80 Rust/32前端/3Edge/12Python，旧桌面3另测、私有MF1ignored；默认/全特性及生产特性clippy、fmt、所有旧feature兼容/构建通过。使用dev/test debug0+incremental0避免缓存膨胀。最终真实本机TLS/WSS/Session/MF/RTP/Edge/control五轮auto/sender-stop/receiver-stop/stats-failure/wss-disconnect通过，正常300全帧+100ACK；原生实际文件2阶段落盘，角色/外会话写入拒绝。stats故障为API注入/last_known，WSS断开真实kill本地TLSserver/exit1预期保留，不冒充正常零错。临时TLS私钥/配置目录均清理，无TURN/no cloud/no actual desktop。
- artifacts/p006c-diag分automatic/local/development/historical audit/offline fixture/summary，summary保留源码摘要/二进制SHA。开发保留Vite首opt重载失败和一次100ACK等待超时（未获完整失败快照、原因未归因）；runner已补提前异常失败JSON+退出码/完成时刻交叉验证防旧成功JSON残留冒充，最终五轮通过，不能改历史FAIL。报告PHASE0_P006C_DIAG_TEST_REPORT.md/最小在线计划/契约已更新。最终Git SHA以实际提交为准，完成后停止扩展待审查。

# P006c 真实双机手动验收收尾（历史服务已停）

- 用户单独授权最多90分钟，本轮亲自批准两端采集；main基线65993e1，沿用d805ecb程序，未改产品源码/不代点。09:39:31启动独立低权transient WSS及既有专用TURN，09:53:20提前停服，expiry timer已取消，授权结束不得沿用重开。
- 两真实Windows成功轮：LAN直连native host/srflx、浏览器认证周期srflx/srflx；B手机热点轮双方relay/relay。ICE All、desktop源DXGI/MF各900输入/编码/发送，720p，100发送/原生接收/ACK/确认，pingpong1/序号1..101/异常0、源控制join。用户观看变化画面并确认双方资源回收无错。
- 接收端认证末周期分别893/880接收/解码/呈现，lost0/freeze0；B独立final未收取，不能写全链最终900或用源browserClosed冒充B关闭证明。更早两次DIRECT_SELECTED_PAIR_NOT_APPROVED失败，0帧/0控制，资源已回收，保留development；原因未归因，未改安全匹配或声称已修复。P006c保持doing、成功轮与本次操作收尾完成、待审查上述问题；P006/Phase0不提前全done，不进输入或Phase1，不要求重复旧P006b接收端停止补测。
- 同coturn PID337919最多2分配，四次尝试8实际Allocation均Refresh0请求/成功回应+1.0615–1.4659秒首次socket消失；停服务前socket0。原租约600秒，本轮不声称所有deadline+15再测完，历史自动云9例完整期限及更早timeout单独保留。新旧WSS/coturn均停，旧disabled/新无持久unit、timer取消、端口0；原配置/unit/旧binary/FWhash/26服务集合/业务容器启动时间不变，云key/invitations/labels/rawpcap删、SSH关闭。
- 独立报告docs/PHASE0_P006C_MANUAL_TEST_REPORT.md；artifacts/p006c-manual分LAN/hotspot/development/cloud/summary/local-cleanup与verification。scripts/p006c-manual-evidence.py离线从实际记录复算，无网络/采集。操作脚本归档deploy/rendezvous-probe/p006c-manual-ops，仅审查不用旧授权执行。
- 本机已删除artifacts/private/p006c-manual-client-private.json、父目录已激活解压包的p006c-private.json及带凭据ZIP，程序和stats保留。用户随后明确确认第二台p006c-private.json及带配置“已激活”ZIP已删除、程序和统计保留，清理闭环；证据为用户陈述，actualSecondPcDeletionVerified=false仍保留，不能称Agent已独立核验B文件或补写B视频终态。旧邀请服务端已清理。无凭据旧交付ZIP仍保留，未来需新授权/短期配置。
- 本轮无产品代码变动，不重跑不冒称74Rust/24前端全套；核对日志/新离线脚本/凭据扫描完成。完成GitHub提交和运维记录后停止扩展等待审查，最终Git SHA以对话/提交为准，避免自引用。

# P006c 真实云视频自动验证交付检查点（历史已收尾）

- 用户明确授权90分钟/4alloc/自动合成WSS/TURN，明确禁止自动真实双机桌面采集；结束必须全停不自启、删临时凭据。开工fetch main f57674c干净，沿用ADR0010，无Rust/Tauri源码改动。新目录/opt/remotehub-p006c-video-20261122、旧工具chain/vendor离线编译WSS6tests/release过，低权transient WSS沿用8443，coturn配置/旧WSS/业务FW SG不改。
- 2026-11-22 09:07:51启动；真实Windows单机两原生WSS角色+Session/MF/前端Edge合成图案。09:10:17正常host/host，09:10:59进程内直连阻断40包自动relay/relay；ICE All，两轮各300帧输入/编码/发送/接收/解码/呈现、720p30、100事件发送/原生接收/ACK/确认、连续1..101+pingpong1、异常0/lost0/freeze0。首帧观察2364.5/3040.2ms不能当纯RTT/任意NAT结论。测试浏览器fake设备权限只为VPN下物理接口曝光，不读实际设备，生产WebView2无该开关。
- 保留首云轮passed=false：原生300帧/100ACK，前端finish用旧周期样本280覆盖新stats，不能判真实20丢帧。现finish在PC/track关闭前读取真实最终snapshot并保留候选校验；两云轮均全300。前端新增2项最终快照与安全拒绝回归、当前24tests/type/lint过；现有build/3Edge/12Python/Tauri构建过，日志cloud-automatic。Rust源码未变，历史74/clippy不冒称本轮重跑。最终原生回传样本仍可能早于浏览器终态，本轮不扩原生save协议。
- 同coturn PID332698最多3分配，三轮9实际Allocation（含首失败），全部Refresh0请求/成功回应+1.1097–1.3967秒首次socket消失，实际600秒租约及+15秒均无残留。observer显式split-shared-credentials按真实UDP tuple私有关联，公开别名，不把WSS共享username混为一个。9:21:08提前finish保护拒绝未变更；9:28:18重连核对9项/同PID/0socket后停止，shutdown完整。26业务集合/容器启动时间、原config/unit/binary及FW策略不变；新旧WSS/coturn停，旧disabled、新无持久unit、timer取消、端口0，临时key/邀请/labels/rawpcap云端和本机已删，SSH关闭。
- docs/PHASE0_P006C_VIDEO_CLOUD_TEST_REPORT.md、P006C_MANUAL_ACCEPTANCE.md，artifacts/p006c-video/cloud含真实pair/rawstats/9classifications/脱敏事件/shutdown；历史本地/首步报告保留。新scripts/p006c-cloud-evidence.py从真实事件复算，package-p006c-video.py仅全停后生成待激活包，无有效配置/secret，CRC+SHA。最终SHA/包见delivery.json与会话，避免自引用。
- 当前只剩用户后续另行批准并操作物理双机/WebView2/DXGI，P006c保持doing/Phase0 4of7；不要求现在重复操作或旧接收端停止补测。后续启服务/签新配置必须新授权，本次窗口结束，不进SendInput或Phase1。
- 云自动代码/证据/独立报告已提交d805ecb；无凭据双机包RemoteHub_P006c_双机实验_v1_20261122.zip在工作区父目录，CRC6成员和当前程序SHA核对，具体delivery.json。运维清单及历史已通过正式审批写入本轮已核实结果/停用状态，无凭据。接下来仅上传提交和交付信息，不再启动云服务或采集。

# P006c 视频/control/WSS集成检查点（历史）

- 用户要求按Rust Session→WebView2→认证WSS顺序接入，优先本地，再单独申请云范围。main e020051干净，先ADR0010/契约提交a9fb146；本轮尚未启动或变更云服务，旧授权不继承。
- 默认关闭p006-direct，复用旧视频PC/MF/RTP/control.v1。DirectGrant绑定控制UUID，完整SDP后安装精确端点，原生及前端真实pair两侧匹配；媒体/控制等验证再开放。新HMAC path交换明确对方私有IP范围；旧RelayOnly wire缺省兼容，模式不匹配拒绝。WSS应答任务不阻塞4秒心跳。停止立即撤销生产/处理，通知并2秒close grace、5秒join后关PC/边界；旧范围预算保留。
- 新p006-session-probe只操作员合成NV12/MF，不采集桌面，不可通过正式JSON开启。2026-11-22 08:28本机Session四轮、08:34真实本地TLS/WSS/前端三轮均通过；完整轮300帧全链/720p30、100条发送/原生接收/ACK/确认、host/host、序号1..101、异常0及清理。异常销毁Edge页真实PEER_CONNECTION_FAILED/exit1、411源帧/关闭前153接收帧，记录为预期断线回收，非正常零错或Tauri关窗实测。停止快照差异保留。
- 35项最终检查全部达预期，74 Rust/22前端/3Edge/11Python，旧桌面3另测，私有MF样本1ignored；实验Tauri已编译。development保留5失败：PC/channel关闭顺序、RPC串行stop及正常停止统计竞争。后者只在明确停止码且原生无错/已回收才消除误报，安全错误继续失败，最后WSS三轮全过。
- docs/PHASE0_P006C_VIDEO_TEST_REPORT.md、P006C_VIDEO_CONTRACT/RUNBOOK/CLOUD_PLAN、artifacts/p006c-video分层证据及源码/二进制SHA。测试浏览器因VPN使用进程内合成设备权限暴露物理接口，随即停止合成流，无相机/麦克风/桌面；local WSS完整TLS，无TURN，未用占位ICE在测试工厂移除。生产前端默认PC工厂不变，Tauri只能desktop且每次原生亲自批准。
- 新版云视频回退、物理双机、嵌入WebView2、DXGI未验收，P006c仍doing/Phase0正式4of7。先提交并上传当前代码/报告，再请求新具体云授权（计划新受限目录+离线WSS构建，低权临时8443、复用原coturn不改配置FW SG业务，90分钟4alloc上限，自动合成正常/回退后物理LAN+热点，最终全停不自启、资源清零先判）。授权前不能部署或启动。用户明确跳过旧接收端停止补测，保留原日志限制，不要重复要求。

# P006c 首步交付检查点（历史）

- 生命周期独立代码/报告已推main 88f1edc，用户随后明确授权P006c自动云测试（最多40分钟/2分配/只既有coturn/WSS关闭/无配置FW SG业务改动）。ADR0009先提交b8b0214，新增默认关闭p006-direct-probe；尚未接现有视频Session/WebView2/业务信令，不标P006c或P006父项done。
- 2026-11-22北京时间07:13:41至07:17:31实际启动专用coturn，单PID324151；同一物理Windows机两个真实原生PC、无媒体/输入，ICE All：正常host/host 100/100/100、直连进程内阻断自动relay/relay 100/100/100（34.873秒）、TURN进程内阻断仍host/host 100/100/100（22.051秒）、两路阻断35.014秒明确DIRECT_CONNECT_TIMEOUT、0消息/no pair。双方正常/回退均保留host/srflx/relay各1，不改relay-only。故障真实丢包仅进程内，不是物理NAT/防火墙试验。
- 4实际云分配各授予600秒，Refresh0成功回应+1.140至1.394秒首次观察socket消失；同PID最多2，逐轮0再继续。后两轮4票无成功Allocate；本轮即时释放后提前停止，原租约deadline检查点null（上一轮6例完整期限证据独立保留）。最终shutdown证实停服务前socket0、coturn inactive/disabled、WSS始终停、timer取消、原配置/unit/FW策略/业务容器与服务集合不变，原始控制捕获/短票清理，SSH关闭。
- 报告docs/PHASE0_P006C_TEST_REPORT.md，脱敏artifacts/p006c（真实云、local与offline/development分开）；24项最终检查/66 Rust、旧桌面3、16前端、3Edge、11Python；MF1ignored。最终审查后加EOF失败保护与真实闭PC回归，保留云前二进制/sourcehash及最终差异，未为此重启服务。首个component2拒绝及闭通道fixture类型/handler错误留development，修复后重跑。
- 约束：用户明确跳过接收端主动停止补测，保留原独立日志限制。P006c完整视频/control.v1与WebView2接入、真实物理双机/NAT、trickle/restart/切网恢复未做，SDP外prflx仍拒绝。下一步复用旧视频链路接入，不进入输入或P1。新云启动仍需新范围授权。
- 运维资料外部清单写入首次因自动审批工作区额度不足未执行，未绕过；后续Git上传审批恢复后，通过同一审批通道成功更新生命周期及P006c的真实结论/最终停用状态，未保存凭据。仓库报告/交接证据完整保留，Git最终SHA以提交/对话为准，避免自引用。

# TURN生命周期 / P006c开工检查点（历史）

- 用户明确接受接收端主动停止现场确认并要求跳过补测，保留原900帧独立提前停止证据限制，不再要求重复双机操作。P006b据此收尾，P006父项仍doing，P006c需后续开发。
- 开工main ed6ff20干净，先提交ADR0008 dde4649。用户单独批准既有coturn最长60分钟/最多2分配/WSS关闭/无配置防火墙安全组业务修改。2026-11-21北京时间22:39至23:06真实完成6例：协议主动释放、停止续租、Windows原生/Edge gather后close、已连接Rust/Edge data-only双方close。5例Refresh0请求/成功回应+1.110至1.464秒首次观察socket消失；实际租约均600秒，不续租到期后0.318秒消失。原始deadline+15秒均无残留，同PID286888、最多2分配；判定结束前socket已0，未用服务停止冒充单例释放。
- docs/TURN_ALLOCATION_LIFECYCLE_TEST_REPORT.md、artifacts/turn-lifecycle为真实证据；19组检查符合预期，61 Rust/16前端/3Edge/11Python，私有MF1ignored。Rust与Edge在同一物理Windows机器真实SCTP ping/pong、双方relay/relay；无采集，无新双机/WebView2视频负载实测，不能扩大解释。本轮未归因上轮遗留1socket。
- 23:05:59已停专用coturn且disabled，WSS始终停用，timer取消，原始控制捕获和短票清理；shutdown.json证明配置/unit/防火墙hash、业务容器启动时间及服务集合不变。后续云端再次启动或变更需提交新具体范围取得授权。
- 下一步：先独立提交并推送生命周期结果，再写P006c ADR/契约、实现P2P优先及TURN自动回退，尽可能无画面自动Windows测试；不能将尚未执行的真实双机验收标PASS。不进入输入注入或Phase1。

# P0-06b 双机收尾检查点（历史）

- 实现提交977f97fe3aac287283fa73da93cf60b9261e965b已推GitHub；源码/二进制本轮收尾不再修改。ADR先6f694de，基线4507fa9，沿用同一视频PC与control.v1，未做P006c/输入/P1。
- 真实宽带+蜂窝热点双机5轮：两轮30秒各900帧完整发送/接收/解码/呈现，720p、lost0/freeze0；每轮真实双方relay/relay UDP、100条发送/接收/ACK/确认、1..101连续序号、异常0。三轮提前194/196/244帧，按用户给定顺序对应发送端停止/接收端关窗/热点断网，源/控制join回收。首个接收端停止尝试仍900帧；补测请求后用户称正常，无新增记录，只记现场确认，不标独立PASS。B本地final/window-close未收取，A browserClosed不冒充B证明。
- 自动13组PASS：60 Rust+旧桌面3、16前端、3Edge；私有MF1 ignored。Ubuntu WSS4测试/build、公网TLS/错误CA/邀请拒绝与本机两分配双向TURN通过，和双机结果分开。artifacts/p006b/two-pc为脱敏实测/原报告SHA/用户观察；cloud/shutdown为收尾。
- 北京时间2026-11-21 22:17:25已停remotehub-p006b与remotehub-turn-g6、均inactive/disabled/MainPID0；8443/3478/relay范围/实验进程socket均0，TURN运行目录回收，取消临时timer，SSH关闭。基线26服务仍运行，两个容器保持；另fwupd运行（未操作/原因未查），不称集合完全相同。停服务前尚1个relay范围socket，未归属每会话，逐PC即时Refresh(0)未证实。秘密仅云端受限/本机忽略目录；临时TURN票已从本机删除。
- 状态：双机主链路及主要清理通过，P006b/P006完整验收doing（上述独立证据限制）；P004录屏差异保留，Phase0正式仍4/7。交付报告/脱敏证据，停止扩展等待审查。原私有实验包限时邀请不再用于在线实验；最终Git/归档SHA见BUILD_INFO。

# P0-06a / G6 交付检查点

- 已完成带证据：准备基线079275d、ADR先行3a8f67f、实现5e05c09。用户后续单独授权指定ECS的G6，开工fetch仍5e05c09干净；实际Ubuntu24.04/coturn4.6.1、非root专用unit与EIP映射，Windows原生两轮各1 relay候选/gather81和92ms/close成功，真实UDP11项全部通过（无/错/过期401、错realm437专用原因、五危险peer403），Refresh释放。P006a现infrastructure-verified。完整17组回归达预期，54 Rust/13前端/3Edge/3配置Python+4协议Python，私有MF1ignored。apps/crates/锁和实测CLI二进制与5e05c09不变。
- 相关文件：docs/PHASE0_G6_TEST_REPORT/PLAN、历史P006a报告/ADR0006；scripts/g6_turn_wire.py及4项离线测试/RFC许可；deploy/coturn单元pidfile/userdb专用/run路径；artifacts/p006a/g6保留真实结果/首轮断言差异/本轮全回归/收尾摘要。服务器本次新增remotehub-turn-g6.service、/etc/remotehub-turn-g6和/opt/remotehub-turn-g6-tools，实际地址/秘密仅服务端私有或本地忽略目录，登录密码未保存。最终SHA/包以BUILD_INFO为准。
- 未完成：P006父项仍待不同网络双机视频/control真实relay与pair证据，不能以Allocate代替；Phase0仍4/7、P004录屏差异不改。测试专用及默认coturn均inactive/disabled，短票本机文件已移除，运行时socket与/run目录回收；保留已安装包/受限配置，后续需另轮授权启动。既有26服务/2容器仍运行，INPUT/OUTPUT和归一化nft策略不变，没有改安全组/防火墙。完成交付后停等审查，不进P006b/c/P1。
- 风险/发现：0.21.1原生TURN仅UDP；P006b/WSS/长期身份未实现，REST票据可复用且无单票即时撤销。G6第一次错realm返回437而断言期望401/441，保留首轮；核对coturn4.6.1源码Allocate分支确为437+专用原因，新增严格回归后真实重测11/11。nft原始hash因40处计数变化，去counter数值后结构完全相同。STUN/Allocate事务耗时不是ICE RTT；未建PC对端故pair/RTT/loss仍null。目标4.6.1 help已验证pidfile/userdb，单元不写受保护默认目录；其它目标仍须独立核对。

## P0-05 基线

- 已完成带证据：fetch确认main=b914942且开工工作树干净；先提交ADR0005/CONTROL_V1（f68e42e），再实现。默认关闭p005-control，复用同一视频PC。两台物理Windows10有线电脑两轮30秒视频+100事件，A接收/ACK写入/B发送/确认均100，连续1..101序号、ping/pong1、异常0；源900帧，B最后回传视频893帧、720p、丢包0（不伪称最终900）。四种清理：B停止188帧/A停止207/B关窗210/B拔线279（LAN_LEASE_EXPIRED），全部控制join/源/PC回收。B停止/关窗/拔线有A真实OnClose1。43 Rust、11前端、2Edge和全部10最终命令exit0；本机真实SCTP负例与双机分开，私有MF1项ignored。
- 相关文件：control.rs/control_channel.rs；signaling/session/lan；Tauri p004_lan/feature/main；Vue control.ts/lan.ts/LanVideoProbe；scripts/p005.ps1。旧P004a/04b构造器仍video-only，P005才接受一video一application的同BUNDLE SDP。TCP只运认证上下文、心跳和统计，无控制事件/ACK代理。源码不含SendInput调用。
- 未完成/限制：P005范围已验收，收尾Git与归档标识见交付BUILD_INFO/会话结果，完成后停止等待审查，不进P006/P1。B终态本地final未收取，其界面/回收由用户确认；B运行中真实ACK统计由A认证连接保存。没有物理DataChannel OnError，error回调覆盖是自动fixture，不伪造。P004基线录屏差异继续保留；无实际输入、长稳、Win11或独立安全审计。
- 发现/修正：webrtc fixture需add_track（不采集）才能sendonly；fixture候选先模拟前端component1过滤；真实DataChannel poll还产生buffered事件，集成测试需等待OnMessage而非把任意事件当ACK。H264匹配限video节、每节独立mid。前端测试RTCErrorEvent类型已修。PowerShell把stderr包装NativeCommandError，最终用Node捕获UTF8原始stdout/stderr和退出码避免误判；实际最终10命令全0。真实P004b基线保留，不把旧记录当P005新实测。

## P0-04b 基线

- 已完成带证据：基线5fb0867，ADR先提交dabb5fa。两台真实Windows10有线PC，v2完整30秒900帧输入/编码/发送/接收/解码/呈现全部一致，1280×720，8365包、接收丢包0，实际selected host/host UDP指定对端匹配。两端停止114/144帧、接收端关窗125帧、拔线187帧（LAN_LEASE_EXPIRED）提前停止，发送端源/会话释放。用户亲自确认并操作，回报四项均回收。36 Rust(all features)、8前端、1Edge、verify/clippy通过，1私有MF测试保持ignored。
- 相关文件：ADR0004、hub-webrtc scope/pinned_runtime/lan_wire/lan、Tauri p004_lan/feature/命令ACL、Vue LanVideoProbe/lan.ts、p004b脚本/限定规则脚本、P004B_RUNBOOK和PHASE0_P004B_TEST_REPORT。脱敏实际统计artifacts/p004b。私有目录不得打包；具体IP/配对码/原SDP不进Git。
- 未完成：P004b功能通过，P004基线所列录屏未留存（本轮原生授权明确不保存画面），父任务保留doing/正式计数3/7。接收端本地final/window-close未收取，接收端关闭/回收由用户确认；发送端持有真实接收端指标回传，不能把A的browserClosed当B证明。P005 DataChannel、P006 TURN、公网、长稳、Win11未做。Git/归档信息以交付BUILD_INFO与会话结果为准，避免将最终提交哈希自引用写入Git。
- 风险/复盘：首轮配对成功但0帧，B拒绝库真实应答component2，A SIGNAL_DISCONNECTED。v2仅在应答有rtcp-mux时接受同指定IP/UDP的component2，真实库应答回归先失败后通过；B实际错误直接传出，避免LAN_SIGNAL_FAILED掩盖根因。停止/断线最后回传计数落后源停止，不当作丢帧；RTT无有效测量为null。防火墙脚本只解析/Inspect，未找到其专属规则组，不声称安装/移除已测；没有替用户修改防火墙。

## P0-04a 基线

- 已完成带证据：基线 7fcbb51，ADR 先行 6d38af3。用户亲自批准并操作原生 WebView2，五轮15秒450帧 + 30秒900帧全接收/解码/呈现、丢包0；主动停止47帧、关窗33帧清理通过，最终窗口/进程消失。artifacts/p004a 原始脱敏计数、pnpm verify、25 Rust(all features)、5前端、1Edge、显式1项私有真实MF AU测试均通过。
- 相关文件：stream.rs（Windows线程、不写视频）、hub-webrtc/session.rs/video.rs/signaling.rs、Tauri p004.rs/feature/ACL、Vue features/p004、scripts/p004.ps1、P004_FORMAT.md、PHASE0_P004_TEST_REPORT.md、ADR0003。默认 feature 关闭；显式脚本优化构建到 target/probe。视频统计仅存程序目录 artifacts/private/p004。
- 未完成：P0-04a done，原P0-04仍doing；P0-05/06未做，不进入P1。下一步双机范围与授权设计。剩余本轮流程是最终 Git 提交/上传及交付归档核验，最终SHA/包见后续会话记录。
- 已知风险/限制：AMD真实SPS42041f，按标准忽略对Baseline不适用constraint5做兼容，不改字节。browser无candidate offer由127答案建立prflx已实测；不能推广所有环境。原生同意需用户亲自操作。无Win11/长稳/物理GPU丢失/PLI端到端/RustSec；GUI OS退出码与播放截图未单独留存，报告明确。私有MF样本环境变量必须绝对路径，Cargo测试cwd是crate目录。

## P0-03 基线

- 已完成（带证据）：P0-01、P0-02 保留；P0-03 AMDh264Encoder 10 秒/300 帧/720p30/零丢 tick，标准解码、八色条、PTS、GOP 通过；软件 2 秒/60 帧通过。17 Rust / 3 前端 / 1 浏览器测试。证据 artifacts/p003/ 和 docs/PHASE0_P003_TEST_REPORT.md。
- 相关文件：crates/hub-encode-win/{src/mf.rs,src/probe.rs,src/h264.rs,src/color.rs}；docs/P003_FORMAT.md；ADR 0002；TASKS.md。视频样本仅在 artifacts/private/p003-accepted，不能提交/上传。
- 未完成：P0-04 WebRTC 视频、P0-05 DataChannel、P0-06 TURN、P0-07 全阶段结论。下一轮先做有界本机受授权 WebRTC -> WebView2；不进入 P1。恢复验证：设置 REMOTEHUB_VERIFY_DIR 到新证据目录，然后 pnpm verify。
- 阻塞/限制：真实硬件丢失与 Windows 11 未测；raw ffprobe 速率估计不代表实际 FPS，MF PTS 和 MP4 已验证 30Hz。远程身份/配对/签名/ACL 尚未实现，不要把本机 consent 当作远程授权；两机/TURN 出口仍缺第二台 PC 与节点。
