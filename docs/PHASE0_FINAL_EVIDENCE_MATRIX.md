# Phase 0 最终证据矩阵

审查日期：2026-11-22；冻结基线 `7fde54e6d94f2eb9ef2697e437cccfd42f4f67c9`。本轮是**归档证据离线复核**，没有新运行媒体、云端或双机实验。“通过”只覆盖各行当时的版本、环境与指标，不能把所有阶段相加成当前版本的完整测试集。

类型：**实机-1**为一台物理 Windows 的原生实验；**实机-2**为两台物理 Windows；**云-1**为真实云服务配合一台物理 Windows 上的多个端点；**自动**包含单元、fixture、浏览器或本地合成；**决议**为用户明确接受。合成源不等同桌面，Edge 不等同嵌入 WebView2；原生 PC 数量不等同物理电脑数量。

| ID / 类型 | 当时已通过的证据 | 主要归档来源 | 限制及本次判读 |
| --- | --- | --- | --- |
| E01 P001 / 实机-1 | Windows10 原生 Tauri v2/WebView2 窗口、IPC 刷新及本机构建 | [首轮报告](PHASE0_TEST_REPORT.md)、[窗口观察](../artifacts/native-window.txt)、[环境](../artifacts/environment.json) | Win11 未验收；没有据此宣称正式桌面产品完成。 |
| E02 P002 / 实机-1 | DXGI 主屏 2560×1440，30 帧/1059ms，0 超时 | [采集数据](../artifacts/capture-report.json)、[首轮报告](PHASE0_TEST_REPORT.md) | 28.315 为桌面更新观察 FPS；不是端到端视频 FPS。原私有画面不作公开证据。 |
| E03 P003 / 实机-1 | AMD 硬件 MF 720p30、10秒300帧；独立解码300帧、颜色/PTS/GOP通过；软件回退另有2秒60帧 | [报告](PHASE0_P003_TEST_REPORT.md)、[硬编](../artifacts/p003/hardware-encode-report.json)、[解码](../artifacts/p003/decoder-validation.json) | 单GPU/驱动、短时实验；无端到端输入延迟或全硬件覆盖结论。 |
| E04a P004a / 实机-1 | 原生 WebView2 同机视频：五轮450帧、一轮900帧完整收发/解码/呈现；停止/播放中关窗回收 | [报告](PHASE0_P004_TEST_REPORT.md)、[逐轮统计](../artifacts/p004a/native-session-summary.json) | loopback 不能充当双机；缺失的关闭瞬间 JS 计数继续缺失。 |
| E04b P004b / 实机-2 | 有线LAN两Windows10，30秒900帧全收发/解码/呈现，720p、host/host、丢包0；两端停止/关窗/拔线回收 | [报告](PHASE0_P004B_TEST_REPORT.md)、[完整轮](../artifacts/p004b/full-30s.json)、[回收轮](../artifacts/p004b/cleanup-runs.json) | **未录屏**；部分回收为源端统计+现场确认，不能补造B独立文件。 |
| E04 / 决议 | 已批准真实统计+现场观看替代原录屏，P004限定范围验收 | [用户决议归档](PHASE0_P007B_DECISION.md)、[ADR0013](ADR/0013-p007b-r06-diagnostic-package.md) | 例外已接受，不重开；保留原未录屏事实，无画面回放。P004当前应为done。 |
| E05 P005 / 实机-2 | 同一视频PC的可靠有序control.v1，100发送/接收/ACK写入/确认，序号连续、异常0；视频同时播放及回收轮通过 | [报告](PHASE0_P005_TEST_REPORT.md)、[完整轮](../artifacts/p005/dual-pc-full.json)、[重复轮](../artifacts/p005/dual-pc-repeat.json) | 只有模拟输入；完整源900、B末周期893不是B最终900。 |
| E06a G6 / 实机-1+云 | 11项真实UDP协议/危险peer拒绝，两次原生relay gather | [报告](PHASE0_G6_TEST_REPORT.md)、[协议统计](../artifacts/p006a/g6/wire-results.json) | Allocate或relay候选不单独证明媒体走TURN。 |
| E06b P006b / 实机-2+云 | 宽带与手机热点两PC，完整轮各900帧全收发/解码/呈现；双方selected pair为relay/relay，模拟控制一致；停止/关窗/断网回收 | [报告](PHASE0_P006B_TEST_REPORT.md)、[双机统计](../artifacts/p006b/two-pc/statistics.json) | 用户已接受接收端主动停止现场确认并跳过补测；独立提前停止日志限制保留，不重新阻塞。 |
| E06L TURN生命周期 / 实机-1+云 | 6例：5例Refresh(0)、1例实际600秒租约超时；同PID、到期+宽限无残留，先判断后停服 | [报告](TURN_ALLOCATION_LIFECYCLE_TEST_REPORT.md)、[逐例数据](../artifacts/turn-lifecycle/results.json) | 包含协议/Windows data-only用例；不代表所有WebView2视频/网络错误路径，也不倒推旧未归属socket。 |
| E06N P006c网络 / 云-1 | 两个原生PC：直连、受阻回退、TURN受阻仍直连；双路受阻真实失败并回收 | [报告](PHASE0_P006C_TEST_REPORT.md)、[回退](../artifacts/p006c/cloud/fallback.json)、[双路受阻](../artifacts/p006c/cloud/all-unavailable.json) | 一台物理机；无视频；丢包为有界实验注入，不是物理拔线。 |
| E06V P006c云视频 / 云-1 | MF合成源/Edge，正常直连及受阻relay，各300帧全链、100控制；9分配释放 | [报告](PHASE0_P006C_VIDEO_CLOUD_TEST_REPORT.md)、[直连](../artifacts/p006c-video/cloud/auto.json)、[回退](../artifacts/p006c-video/cloud/fallback.json) | 不是新双机桌面/WebView2测试；真实云服务与本机两端须分别标注。 |
| E06M P006c桌面 / 实机-2+云 | 真实桌面LAN直连与热点中继成功轮各900源帧、720p、100模拟控制；8分配先释放后停服 | [报告](PHASE0_P006C_MANUAL_TEST_REPORT.md)、[LAN](../artifacts/p006c-manual/lan/desktop-direct.json)、[热点](../artifacts/p006c-manual/hotspot/desktop-relay.json) | B末周期893/880，独立final缺失；同轮历史另有两次候选拒绝/0源帧，不改成全部PASS。 |
| E06D DIAG / 自动+实机-1 | 分侧固定原因、首次失败保留、独立pre-close/final；35项当轮回归、5轮本机TLS/MF/Edge合成，覆盖统计故障/中断 | [报告](PHASE0_P006C_DIAG_TEST_REPORT.md)、[摘要](../artifacts/p006c-diag/summary.json)、[历史核查](../artifacts/p006c-diag/historical-audit.json) | 80 Rust/32前端等为当轮测试；开发期100ACK超时未归因；不是新物理双机故障验收。 |
| E07P 隐私v2 / 自动+实机-1 | 扫描器前向修复、12隐私/9交付；41项当轮安全兼容及5轮本机合成；Release实下哈希核对 | [报告](PHASE0_P007B_PRIVACY_FIX_REPORT.md)、[回归](../artifacts/p007b-privacy/summary.json)、[下载](../artifacts/p007b-privacy/published.json) | 当前修复已接受；旧对象仍可达，未重写历史，未证明外部缓存清除。不是新双机。 |
| E07R R06 v2 / 实机-2+云 | 一轮8秒synthetic：240发送/接收/解码/呈现；Rust两侧matched，B独立pre-close/final均live；100背景控制一致；2分配Refresh(0) | [报告](P007B_R06_ONLINE_DIAGNOSTIC_REPORT.md)、[A](../artifacts/r06-online/sender-summary.json)、[B](../artifacts/r06-online/receiver-summary.json)、[释放](../artifacts/r06-online/allocation-verdict-before-stop.json) | 媒体本轮direct，TURN为候选收集分配；未复现旧拒绝、未确定根因；正常自动结束不能代替异常/OS关窗验收。 |
| E07O R06计时/收尾 / 云+人工 | 服务启动前真实3秒timer触发清理；正式timer验证后激活；约306秒提前停服/清凭据，基线未变，第二台删除由用户确认 | [guard](../artifacts/r06-online/guard-proof.json)、[收尾](../artifacts/r06-online/after-stop.json)、[确认](../artifacts/r06-online/operator-confirmation.json) | 未等运行中服务自然到第19分钟；无新租约超时用例。最后已知停止13:40:16，本轮没有再连接云端。 |

## 最低退出条件与尚待决议

| 原条件 / 决议 | 证据判断 | 当前状态 |
| --- | --- | --- |
| 两台真实Windows持续视频 | E04b/E06b/E06M | 已在限定拓扑成立。 |
| 同PC真实模拟控制 | E05/E06b/E06M | 已成立，不含实际输入。 |
| 真正TURN媒体 | E06b及E06M热点轮 | 已成立，依据实际selected pair。 |
| P004录屏契约 | E04明确例外 | 已批准并有效；不是仍待补录。 |
| 当前新版正常终态可取得 | E07R | 已实测一轮；旧缺失文件不补造。 |
| R06历史可靠性剩余风险 | E06M失败与E07R未复现共同保留 | 风险接受PENDING；根因NOT_DETERMINED。 |
| 最终审查 / 正式退出 / Phase1实施 | 用户本轮要求审查准备，不含批准 | Phase0 HOLD，P006/P007不提前done，Phase1未授权。 |

性能边界：720p30短时计数、单机编码指标不等于1080p60、低延迟游戏体验、长稳成功率或成熟三件套性能。没有同条件A/B测试、Win11/多GPU、多NAT/多网卡覆盖，不发布连接成功率百分比。旧报告的缺口、ignored与失败均保留。

离线来源哈希/范围复核见 [evidence-index.json](../artifacts/p007-final/evidence-index.json) 及 [verification.json](../artifacts/p007-final/verification.json)。读取的是固定Git对象；新状态来自明确决议，不能用旧索引当时“例外未批准”覆盖后来批准。
