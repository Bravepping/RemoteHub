# RemoteHub 当前开发指引

更新日期：2026-11-22；P1-03实施基线d0fd876。这里补充当前实施/审查入口；[v1.0完整规范](RemoteHub_完整开发文档_v1.0.md)保持原文，各历轮报告按当时结论归档。当前用户决议优先于旧报告的待批准状态。

## 当前事实与权限

- 用户已审查767ff44并接受R06未关闭剩余风险、批准Phase0限定退出，正式父项7/7；P004已批准例外及未录屏事实保留。最新决议见[批准记录](PHASE0_EXIT_DECISION.md)，不是产品发布或R06修复结论。
- R06历史未复现、根因未知。新版一轮8秒真实双机正常终态已验证，不能补写旧缺失final或扩成全环境可靠性。见[最终证据矩阵](PHASE0_FINAL_EVIDENCE_MATRIX.md)、[风险登记](PHASE0_RISK_REGISTER.md)、[退出评审](PHASE0_FINAL_EXIT_REVIEW.md)。
- 用户审查d0fd876后本轮仅批准P1-03，UI卡片/连接准备弹窗/无会话工具栏完成后等待审查。允许本机UI/离线回归，不运行旧激活脚本、不启阿里云、不双机/应用真实桌面采集/远控输入，不执行第三方安装器；P1-04及后续不自动开工。

## 实施基线与后续门槛

1. 栈保持Tauri v2 / Rust / Vue3 / TypeScript / Tailwind，复用现有workspace和媒体Session，不重新创建工程。
2. [P1方案](PHASE1_DESKTOP_IMPLEMENTATION_PLAN.md)中P1-01/02已审查，当前只交付P1-03；新产品IPC/Pinia、真实设备/连接/会话动作和兼容运行仍待后续授权。IPC/状态边界沿用P1-01合同；导航与实验隔离见[ADR0017](ADR/0017-p102-local-navigation-shell.md)，弹窗/工具栏边界见[ADR0018](ADR/0018-p103-inert-connection-and-session-ui.md)。
3. [兼容评估](COMPATIBILITY_INTEGRATION_ASSESSMENT.md)明确Foundation/V+ PC/EasyTier目标与现有WebRTC的差距；必须先固定具体目标、来源/许可、角色/单UI/进程合同，再决定原型。当前没有兼容实现或第三方运行依赖。
4. [ADR0014](ADR/0014-phase0-final-review-and-risk-gates.md)更新验收门槛；[ADR0015](ADR/0015-client-backend-and-compatibility-plan.md)记录后端提案。旧ADR0012/13保留当时事实，不据旧授权启动服务。

## 代码导航与边界

| 位置 | 现有用途 / 后续约束 |
| --- | --- |
| `apps/desktop/src/App.vue`、`features/desktop`、`bridge.ts` | 普通五页产品壳、卡片/准备弹窗/无会话工具栏、固定本地hash导航及只读能力；未知/未接入用空态，不能把概念图示例当真数据。 |
| `apps/desktop/src/features/experiments/ExperimentWorkspace.vue` | 原实验壳保留；显式实验构建继续原生同意/feature路径，普通壳不渲染采集或远控控件。 |
| `apps/desktop/src/features/p004`、`features/p006` | 已验证实验UI/浏览器媒体控制与独立统计；保留授权、统计语义及默认隔离。 |
| `apps/desktop/src-tauri/src/main.rs`及p004/p006/p007b模块 | 原生权限/会话/采集入口；新增IPC/托盘前需最小capability及生命周期测试。 |
| `crates/hub-capture-win`、`hub-encode-win`、`hub-webrtc` | DXGI/MF、标准WebRTC集成、scope、控制及清理；新UI不重写底层。 |
| `hub-turn-credentials`、`hub-probe-wire`、`hub-rendezvous-probe` | 服务端短票/实验信令，不等于正式身份。长期密钥不可进桌面包。 |
| `docs/PHASE0_*`、`artifacts` | 历轮证据只读；私有画面/票据不进Git。不因刷新报告而回写旧PASS。 |

## 审查和变更流程

先看最新main/工作树、AGENTS/TASKS和当前决议；需求涉及协议、候选范围、后端或权限时先ADR与契约。明确区分“现有API”和“规划字段”；每项测试标版本、拓扑、源类型和原生/Edge差别。

当前入口：`pnpm verify:p103`（Windows，scripts/p103.ps1），复用基础函数并固定写artifacts/p103；`python scripts/p103-evidence.py`作本轮脱敏/范围复核。旧verify:p1/p102会写历史目录，新工作轮应使用独立输出。浏览器截图使用TestInfo路径再另存白名单图片；元数据与单元fixture不能冒充原生媒体。本轮结果和原生UI未测限制见[P1-03报告](P103_TEST_REPORT.md)。

历史最终评审的 `scripts/p007-final-review.py` 固定校验当时5/7/HOLD状态，只能在该冻结版本复算，不能在已经批准退出的新工作树重新生成旧verification。历史P007A只读复核仍为 `python scripts/p007a-review-evidence.py --check`；不要运行旧生成器覆写旧证据。

未来构建/回归遵循各具体任务脚本和锁文件；没有源码变动时不把旧41/33项回归说成本轮新结果。新增真实Windows/云验证须相应授权，服务限时、先判断Allocation再停服、清临时凭据及不自启；不以停止整个TURN伪造正常释放。

P1-03提交后等待审查，不继续P1-04至06。任何新云/实际采集或输入、兼容运行及后续任务都须相应范围指令；状态脚本不能代用户授权。
