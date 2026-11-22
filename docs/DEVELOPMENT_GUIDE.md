# RemoteHub 当前开发指引

更新日期：2026-11-22；P2-01实施基线d7ce2e0。这里补充当前实施/审查入口；[v1.0完整规范](RemoteHub_完整开发文档_v1.0.md)保持原文，各历轮报告按当时结论归档。当前用户决议优先于旧报告的待批准状态。

## 当前事实与权限

- 用户已审查767ff44并接受R06未关闭剩余风险、批准Phase0限定退出，正式父项7/7；P004已批准例外及未录屏事实保留。最新决议见[批准记录](PHASE0_EXIT_DECISION.md)，不是产品发布或R06修复结论。
- R06历史未复现、根因未知。新版一轮8秒真实双机正常终态已验证，不能补写旧缺失final或扩成全环境可靠性。见[最终证据矩阵](PHASE0_FINAL_EVIDENCE_MATRIX.md)、[风险登记](PHASE0_RISK_REGISTER.md)、[退出评审](PHASE0_FINAL_EXIT_REVIEW.md)。
- 用户已批准Phase 1限定退出，见[追加决议](PHASE1_EXIT_DECISION.md)；本轮只批准P2-01本机身份。生产初始化只在普通版启动进行，不开放登录/配对/信令/远控。无云/真实采集/实际输入/双机/第三方后端，提交后等待审查，不进P2-02。

## 实施基线与后续门槛

1. 栈保持Tauri v2 / Rust / Vue3 / TypeScript / Tailwind，复用现有workspace和媒体Session，不重新创建工程。
2. [P1方案](PHASE1_DESKTOP_IMPLEMENTATION_PLAN.md)中P1-01至05已交付，Phase 1已获限定退出，本轮仅P2-01身份模块；真实设备/连接/会话动作和兼容运行仍待后续授权。Pinia/只读IPC边界见[P1-04合同](P104_IPC_CONTRACT.md)及[ADR0019](ADR/0019-p104-pinia-typed-readonly-ipc.md)；导航与实验隔离见[ADR0017](ADR/0017-p102-local-navigation-shell.md)，弹窗/工具栏边界见[ADR0018](ADR/0018-p103-inert-connection-and-session-ui.md)。
3. [兼容评估](COMPATIBILITY_INTEGRATION_ASSESSMENT.md)明确Foundation/V+ PC/EasyTier目标与现有WebRTC的差距；必须先固定具体目标、来源/许可、角色/单UI/进程合同，再决定原型。最新[C0合同草案](C0_COMPATIBILITY_TARGET_CONTRACT_DRAFT.md)仅待审决议，公开来源核对不等于C1完成；当前没有兼容实现或第三方运行依赖。
4. [ADR0014](ADR/0014-phase0-final-review-and-risk-gates.md)更新验收门槛；[ADR0015](ADR/0015-client-backend-and-compatibility-plan.md)记录后端提案。旧ADR0012/13保留当时事实，不据旧授权启动服务。

## 当前P2-01入口

[身份契约](P201_IDENTITY_CONTRACT.md)/[ADR0022](ADR/0022-local-ed25519-dpapi-identity.md)/[开发说明](P201_IDENTITY_GUIDE.md)/[测试报告](P201_TEST_REPORT.md)。`hub-identity`是私钥所有者，桌面缓存公开状态；不在Pinia/浏览器存私钥，不添加通用签名IPC。`pnpm verify:p201`、`python scripts/p201-native.py`和`python scripts/p201-evidence.py`使用新artifacts/p201，不刷新历史目录。实际DPAPI需要正常Windows用户加密profile；工具受限失败按不可用记录，不降级。

以下P1入口保留作冻结版本的历史说明，当前授权/阶段状态以本段和TASKS为准。

## 代码导航与边界

| 位置 | 现有用途 / 后续约束 |
| --- | --- |
| `apps/desktop/src/App.vue`、`state`、`ipc`、`features/desktop`、`bridge.ts` | Pinia拥有本机只读能力状态，DesktopApi调用固定Tauri命令/校验返回值；五页/卡片/弹窗/工具栏未知能力保持空态。旧bridge兼容导出，测试替身不进默认路径。 |
| `apps/desktop/src/features/experiments/ExperimentWorkspace.vue` | 原实验壳保留；显式实验构建继续原生同意/feature路径，普通壳不渲染采集或远控控件。 |
| `apps/desktop/src/features/p004`、`features/p006` | 已验证实验UI/浏览器媒体控制与独立统计；保留授权、统计语义及默认隔离。 |
| `apps/desktop/src-tauri/src/main.rs`及p004/p006/p007b模块 | 原生权限/会话/采集入口；新增IPC/托盘前需最小capability及生命周期测试。 |
| `crates/hub-capture-win`、`hub-encode-win`、`hub-webrtc` | DXGI/MF、标准WebRTC集成、scope、控制及清理；新UI不重写底层。 |
| `hub-turn-credentials`、`hub-probe-wire`、`hub-rendezvous-probe` | 服务端短票/实验信令，不等于正式身份。长期密钥不可进桌面包。 |
| `docs/PHASE0_*`、`artifacts` | 历轮证据只读；私有画面/票据不进Git。不因刷新报告而回写旧PASS。 |

## 审查和变更流程

先看最新main/工作树、AGENTS/TASKS和当前决议；需求涉及协议、候选范围、后端或权限时先ADR与契约。明确区分“现有API”和“规划字段”；每项测试标版本、拓扑、源类型和原生/Edge差别。

本轮人工证据入口：`scripts/p106-manual-build.ps1`只编译和复制普通版，`p106-manual-observe.ps1`仅在用户打开后绑定/观察指定PID，不启动/控制/结束应用；步骤见[P106人工计划](P106_MANUAL_ACCEPTANCE_PLAN.md)，结果见[报告](P106_MANUAL_TEST_REPORT.md)。`python scripts/p106-manual-evidence.py`复核本轮证据；必须保留相同的本地EXE，不能拿不同构建替代其SHA。普通UI基本验收已通过，完整原生错误恢复/媒体关闭等限制见最终评审。

以下是上一轮冻结回归的入口说明，不在新工作树覆盖旧artifacts：

当前入口：`pnpm verify:p106`（Windows，scripts/p106.ps1），复用基础函数并固定写artifacts/p106；`python scripts/p106-evidence.py`作本轮脱敏/范围/决策复核。原生探针`powershell -File scripts/p106-native.ps1`仅操作自有测试窗口/托盘API，无采集或输入；真实执行失败仍必须保留，不能用纯策略测试代替。旧入口会写历史目录，不用于刷新归档。新读取清旧runtime、cancelRead显示取消、reset用于卸载静默清理，原生失败不显示浏览器原因；固定接口/5秒/代次不变。P1-05窗口显示按用户确认追加USER_CONFIRMED_WINDOW_VISIBLE，旧日志不改，五页交互仍未完整验证；本轮托盘原生探针启动错误，未产生验收记录，见[P1-06报告](P106_TEST_REPORT.md)。Edge仅自动浏览器证据。

历史最终评审的 `scripts/p007-final-review.py` 固定校验当时5/7/HOLD状态，只能在该冻结版本复算，不能在已经批准退出的新工作树重新生成旧verification。历史P007A只读复核仍为 `python scripts/p007a-review-evidence.py --check`；不要运行旧生成器覆写旧证据。

未来构建/回归遵循各具体任务脚本和锁文件；没有源码变动时不把旧41/33项回归说成本轮新结果。新增真实Windows/云验证须相应授权，服务限时、先判断Allocation再停服、清临时凭据及不自启；不以停止整个TURN伪造正常释放。

P1-06本轮普通UI人工验收通过，最终退出见[评审](PHASE1_FINAL_EXIT_REVIEW.md)，提交后等待用户批准，不继续Phase 2或第三方兼容原型。退出准备与D1–D6见新文档，未答项目不默认批准。任何新云/实际采集或输入、兼容运行及后续任务都须相应范围指令；状态脚本不能代用户授权。
