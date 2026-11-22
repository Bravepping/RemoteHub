# RemoteHub 开发任务看板

> 初始均为 `todo`；实际开始后改为 `doing`，测试验证完成后改为 `done`，否则 `blocked` 并说明原因。不得仅因生成了源码就把硬件验收项标记完成。

## Phase 0 · 先验证核心能力

- [x] `P0-01` **done** — Cargo/pnpm 锁文件、Tauri v2 原生编译、真实 WebView2 窗口启动与 IPC 刷新通过（Windows 10 22H2）；Windows 11 尚未复测。
- [x] `P0-02` **done（首个主屏捕获实验）** — AMD 主屏 2560×1440、30 帧/1059ms、28.315 观察 FPS、0 超时；本地 BMP/JSON；拒绝无同意和无效帧数。旋转/光标合成/HDR/桌面切换仍未支持或未验收，详见报告。
- [x] `P0-03` **done（本机 720p30 编码实验）** — 实际 AMDh264Encoder 硬件 MFT、10 秒/300 帧/零丢帧、Baseline L3.1、颜色/PTS/GOP/完整解码通过；软件 MFT 短时回退验证通过。物理设备丢失和长稳测试未执行，详见 `docs/PHASE0_P003_TEST_REPORT.md`。
- [x] `P0-04` **done（限定范围内按已批准录屏替代证据例外验收）** — 本机及两台真实Windows10有线LAN的720p30视频、启停/关窗/拔线回收已有实测。用户已明确接受归档真实统计+现场观看替代原录屏，见docs/PHASE0_P007B_DECISION.md；本轮仅修正状态遗漏，无新测试。历史未录屏事实及覆盖限制保留，不扩大到Win11/长稳/新网络，不重开例外。
- [x] `P0-04a` **done（同机视频实验）** — Windows 原生 Tauri/WebView2，实时 DXGI/MF → webrtc-rs 0.21.1 → H.264 SRTP/解码；五轮 15 秒各 450 帧及一轮 30 秒 900 帧，接收丢包 0；主动停止与播放中关窗清理通过。默认关闭实验 feature，详见 `docs/PHASE0_P004_TEST_REPORT.md`。
- [x] `P0-04b` **done（有界局域网双机实验）** — 两台物理Windows10有线PC，v2的30秒900帧全部发送/接收/解码/呈现，1280×720、host/host指定对端UDP、接收丢包0；两端主动停止、接收端关窗与拔线提前回收通过。默认关闭LAN feature，详见 `docs/PHASE0_P004B_TEST_REPORT.md`。
- [x] `P0-05` **done（模拟控制，无实际输入）** — 复用P004b同一视频PC的真实可靠有序control.v1；两台Windows10电脑100条模拟事件发送/Rust接收/ACK写入/B确认全部100，ping/pong与序号连续、异常0，视频同时1280×720正常播放；两端停止、接收端关窗/拔线清理通过。43 Rust / 11前端 / 2Edge；协议与本机真实SCTP负例覆盖，详见 `docs/PHASE0_P005_TEST_REPORT.md`。
- [x] `P0-06` **done（用户批准限定媒体可行性验收，R06继续OPEN）** — 用户审查767ff44后已明确接受R06剩余风险并批准Phase0限定退出；G6/P006b真实relay媒体、P006c选路/回退成功轮、生命周期及新版正常终态按各自范围归档。历史拒绝根因未确定，不改旧失败/日志限制、不宣称全环境可靠；见docs/PHASE0_EXIT_DECISION.md。无本轮云端或双机新测试。
- [x] `P0-06a` **infrastructure-verified** — 原implementation-ready的G1–5/G7/G8基础上，用户另行授权G6：Ubuntu24.04/coturn4.6.1，两次Windows原生真实relay候选（81/92ms gather），11项实际UDP协议检查通过，缺/错/过期票401、错realm437专用原因、五类危险peer403、释放与非root服务收尾通过。详见`docs/PHASE0_G6_TEST_REPORT.md`和`artifacts/p006a/g6`。原准备完整回归54 Rust/13前端/3Edge/3Python，新增4协议工具离线测试；未进入P006b/c、实际输入或Phase1。
- [x] `P0-06b` **done（用户接受现场确认，保留历史日志限制）** — 两轮30秒各900帧全发送/接收/解码/呈现、720p、丢包0/冻结0；5轮真实relay/relay UDP、每轮发送/Rust接收/ACK/确认100、连续1..101、异常0。发送端停止194帧、接收端关窗196帧、热点断网244帧提前回收；接收端主动停止用户已现场验证并明确跳过补测；900帧独立提前停止证据限制继续保留，不追填PASS日志。60 Rust/16前端/3Edge及完整构建/lint/clippy通过，云端服务已停止、不自启、socket清零；原逐会话TURN疑点由独立6例生命周期验证补齐（历史未归属socket不追溯定性）。见docs/PHASE0_P006B_TEST_REPORT.md、docs/TURN_ALLOCATION_LIFECYCLE_TEST_REPORT.md。
- [x] `TURN-LIFECYCLE` **done** — 6例真实云端/Windows自动测试：5例Refresh0即时释放，1例600秒租约超时回收；同PID、最多2分配、原租约+15秒无残留，停服务前socket已0。19组自动检查、61 Rust/11 Python通过。独立报告与Git交付先行，不重复双机串流。
- [x] `P0-06c` **done（限定范围风险接受；历史R06未修复）** — 既有真实直连与热点relay成功轮及后续R06正常终态证据已由最终评审汇总，用户现接受历史可靠性剩余风险作为限定退出条件。旧源900/B末周期893/880、旧B独立final缺失、两次候选拒绝仍原样保留，不追填全链900或全尝试PASS；旧8分配Refresh0释放/收尾事实见docs/PHASE0_P006C_MANUAL_TEST_REPORT.md。R06技术问题仍OPEN_NOT_REPRODUCED/根因NOT_DETERMINED，见风险台账与退出决议，不扩大网络/硬件保证。
- [x] `P0-07` **done（用户正式批准Phase0限定退出）** — 767ff44最终矩阵/风险/技术路线与P1方案经用户审查，用户已明确接受R06尚未关闭的剩余风险并批准限定退出。E04批准、历史失败/未测试项保持；Phase0正式父项7/7，不等于产品发布。P1仅P1-01本轮获批；具体兼容引擎及后续任务不自动授权，见docs/PHASE0_EXIT_DECISION.md。

- [x] `P0-06c-DIAG` **done（诊断与最终统计实现/本地自动验证；历史具体根因仍待授权复现）** — 原生精确候选校验保持，新增分侧固定原因/计数/集合来源/私有host批准布尔，首失败保留，不输出端点或凭据；新增scope负例。接收端独立pre-close/final落盘，保留live/last_known/unavailable、原采样时间及原生回收真值；停止先撤销发送/请求原生停止，异常不漏关PC。35项完整回归/80 Rust/32前端/3Edge/12Python、5轮本机真实TLS/MF/RTP/Edge/control通过；1私有样本ignored。旧两次仅能定位到精确集合匹配失败，失败集合缺host是线索而非根因；新云端/物理双机/WebView2 OS关窗未测。无阿里云启动或桌面采集，最小在线方案提交待授权；该DIAG轮未进入P0-07，当前总评另列下项。见docs/PHASE0_P006C_DIAG_TEST_REPORT.md。

- [x] `P0-07-A` **done（技术总评与退出审查；非正式退出批准）** — 基于main246dcea，汇总原生/物理双机/云端/合成与fixture的证据边界；ADR0012建议维持DXGI/MF + WebRTC/WebView2，未承诺GameStream/Moonlight兼容或同级性能。E04录屏偏差建议按明确例外处理，R06连接可靠性保持未闭环；旧P006b停止补测豁免不重开。新增可复算证据索引、修正README当前状态；未改业务源码、旧报告或旧统计，无新云测试/桌面采集/双机操作。见docs/PHASE0_P007A_REVIEW.md及artifacts/p007a。

当前交付：用户已审查767ff44并批准Phase0限定退出，正式父项7/7；P004按批准证据例外，R06接受为未关闭剩余风险。历史两次拒绝、根因未知、旧终态缺失及所有覆盖限制继续保留，不把风险接受写成修复。本轮开始Phase1且仅P1-01获批，不启动阿里云、不采集真实桌面、不注入键鼠；最后已知云收尾来自既有13:40:16记录，本轮不重新连接核对。

当前总评：旧最终评审保留当时待签结论，最新批准由docs/PHASE0_EXIT_DECISION.md追加记录。R06风险接受=ACCEPTED_FOR_LIMITED_EXIT，技术状态仍OPEN_NOT_REPRODUCED；本轮工程实施与验证另见P1-01，不回写旧测试或重写Git历史。现有Tauri/Rust/Vue/TS及严格WebRTC校验保留，Foundation/V+ PC/EasyTier运行集成仍未实现/未授权。

- [x] P0-07-B **prepared（离线决议与R06诊断包准备；在线NOT_RUN）** — E04例外追加决议，专用synthetic-only Tauri构建/短轮协议、程序SHA/待激活包及白名单统计工具交付；安全与本机合成证据见docs/PHASE0_P007B_TEST_REPORT.md。无云启动、真实桌面、新物理双机或Phase1；R06未关闭，等待审查及单独最多20分钟授权。

- [x] `P0-07-B-PRIVACY` **prepared（扫描器修复与v2交付；R06仍OPEN）** — 两处疑似现场端点硬编码移除，通用地址/载荷检查+扫描器源码自检+固定错误输出，12隐私/9交付正负例通过。只读核查公开仓库可达Git历史/当前跟踪文件/旧ZIP，当前疑似值匹配0，旧对象仍可达，不重写历史。41项安全兼容及5轮本机合成重新通过；新版ZIP/SHA及GitHub实际下载证据独立归档。无产品/依赖/ICE边界变更，无云启动或双机操作，不进入Phase1；20分钟申请等待另行批准。见docs/PHASE0_P007B_PRIVACY_FIX_REPORT.md。

- [x] `R06-v2-PREFLIGHT` **prepared（只读核对与执行清单；启动授权PENDING）** — 基于最新main01bddac，2026-11-22 13:05:17只读核对专用服务停止/disabled、WSS实际SHA、配置/权限/旧材料清理及端口；10项基线与3辅助文件SHA符合。旧90分钟清理器不可复用，新19分钟guard/固定20分钟截止、8分钟后禁新分配的清单及脚本只在本地准备，云端未安装/执行。coturn全局8/用户名4如实披露，本次4分配依单会话/逐轮归零/监测约束，不改配置。11运维+12隐私+9交付离线回归，未重跑产品完整41项，无新云媒体/物理双机/凭据签发。见docs/P007B_R06_EXECUTION_CHECKLIST.md、docs/P007B_R06_PREFLIGHT_REPORT.md及artifacts/r06-preflight；当前不要求两机操作，等待独立20分钟授权。R06 OPEN、Phase0 HOLD、不进入Phase1。

- [x] `R06-v2-ONLINE` **done（限定诊断执行与收尾；历史未复现，待最终审查）** — 单独授权后13:35:10开始；真实3秒timer先触发清理入口，正式timer核对后才启动专用WSS/TURN。两物理PC同局域网、用户原生确认、一轮8秒synthetic：Rust严格local host/remote srflx两侧matched、B直连srflx/srflx；240发送/接收/解码/呈现，背景模拟控制100/100/100，B独立pre-close/final均live且最终回收字段true。2分配同PID、Refresh0成功+socket消失，先分类后13:40:16停服、无自启/端口/凭据残留，基线未变；第二台副本由用户确认删除。33离线回归通过；未执行第二轮、30秒、真实桌面、实际输入、19分钟运行中自然到期或租约超时新用例。历史拒绝NOT_REPRODUCED、根因NOT_DETERMINED，旧报告不改，R06未关闭、Phase0 HOLD。见docs/P007B_R06_ONLINE_DIAGNOSTIC_REPORT.md及artifacts/r06-online。

- [x] `P0-07-C` **prepared（最终离线评审与P1规划完成；正式决议待用户）** — 同步P004已批准例外为done，旧报告不改；最终矩阵/风险与R06修复条件、退出报告、ADR0014/15及具体Foundation/V+ PC/EasyTier集成方案交付。只读核对本地候选版本/哈希，不执行安装器、不联网核验上游；当前没有三件套互操作证据。仅新增文档/离线审核工具和新证据索引，无产品源码/依赖/服务变更、不重写历史。P006/P007父项不提前done，R06接受/退出/P1实施分别PENDING；提交后等正式审查。

## Phase 1 · 工程与 UI（本轮仅P1-01获批）

实施合同见docs/PHASE1_DESKTOP_IMPLEMENTATION_PLAN.md及docs/P101_DESKTOP_CONTRACT.md；本轮只复用现有WebRTC工程基础，不包含三件套运行原型。P1-02至P1-06保持未开工，P1-01完成后提交等待审查。

- [x] `P1-01` **done（工程基础与质量门禁；等待审查）** — 复用Cargo/pnpm及锁定栈；8种既有构建配置真实Cargo解析、9隔离测试、verify:p1统一入口与IPC/状态合同完成。47质量命令通过，Rust全特性82/默认56（各1私有MF ignored）、34前端/3Edge/12Python；截图目录隔离后Edge再测通过，旧原图/报告/证据不改。默认Windows原生构建通过；无产品Rust/Vue/权限/依赖锁变化，无云/桌面采集/实际输入/新双机。见docs/P101_TEST_REPORT.md及artifacts/p101。P1-02至06未开工，提交后停止等审查。
- [ ] `P1-02` 首页/设备/连接/会话/设置导航
- [ ] `P1-03` UI 卡片、连接弹窗和会话工具栏
- [ ] `P1-04` Pinia + Tauri IPC
- [ ] `P1-05` 空状态、加载、错误中文提示
- [ ] `P1-06` 托盘常驻与退出

## Phase 2 · 认证与信令

- [ ] `P2-01` Ed25519/DPAPI 设备身份
- [ ] `P2-02` Axum/SQLite/WS 服务端
- [ ] `P2-03` presence、心跳与限流
- [ ] `P2-04` 首次配对/批准/撤销
- [ ] `P2-05` 签名 SDP 信令、篡改/重放检查
- [ ] `P2-06` Session ACL/主动断开/超时清理

## Phase 3 · 远控闭环

- [ ] `P3-01` 集成 DXGI+MF+webrtc-rs
- [ ] `P3-02` 会话窗口收流与播放
- [ ] `P3-03` DataChannel 键盘鼠标注入
- [ ] `P3-04` 多 DPI/窗口缩放处理
- [ ] `P3-05` 断开与资源清理
- [ ] `P3-06` 被控端提示/一键停止

## Phase 4 · 公网 P2P/中继

- [ ] `P4-01` STUN/ICE 直连实测
- [ ] `P4-02` TURN 短时凭据与服务器配置
- [ ] `P4-03` P2P 失败使用 TURN 发送视频/控制
- [ ] `P4-04` 显示真实 selected candidate pair
- [ ] `P4-05` 断网/切网/有界重连
- [ ] `P4-06` 中继配额与滥用防护

## Phase 5 · 优化

- [ ] `P5-01` H.264 优先硬件编码
- [ ] `P5-02` 1080p60 真实性能测试
- [ ] `P5-03` 可生效画质/码率档位
- [ ] `P5-04` 弱网、关键帧、限流与积压丢帧
- [ ] `P5-05` 用户级登录后自启

## Phase 6 · 发布

- [ ] `P6-01` 安全负向测试
- [ ] `P6-02` Windows 安装包测试
- [ ] `P6-03` Linux 部署/备份/健康检测
- [ ] `P6-04` 隐私、日志脱敏和开源许可
- [ ] `P6-05` 最终 E2E 测试与已知限制
