# RemoteHub 当前开发指引

更新日期：2026-11-22；本轮基线7fde54e。这里补充当前实施/审查入口；[v1.0完整规范](RemoteHub_完整开发文档_v1.0.md)作为需求和架构基线保持原文，各历轮报告按当时结论归档。当前用户决议优先于旧报告里“当时尚未批准”的状态。

## 当前事实与权限

- P001–P005限定范围正式验收，其中P004按已批准的统计+现场观看例外完成，保留未录屏事实。P006最低技术条件有证据，但R06风险接受/正式限定验收待定；P007材料已备，正式退出待审查。父项5/7，Phase0 HOLD。
- R06历史未复现、根因未知。新版一轮8秒真实双机正常终态已验证，不能补写旧缺失final或扩成全环境可靠性。见[最终证据矩阵](PHASE0_FINAL_EVIDENCE_MATRIX.md)、[风险登记](PHASE0_RISK_REGISTER.md)、[退出评审](PHASE0_FINAL_EXIT_REVIEW.md)。
- 本轮只准离线审查/规划及GitHub提交。不运行旧激活脚本、不启阿里云、不要求新双机测试、不采集桌面/输入、不执行第三方安装器，不进入P1。历史服务启动授权已经结束。

## 后续实施基线（未授权）

1. 栈保持Tauri v2 / Rust / Vue3 / TypeScript / Tailwind，复用现有workspace和媒体Session，不重新创建工程。
2. [P1方案](PHASE1_DESKTOP_IMPLEMENTATION_PLAN.md)是任务合同提案，六项均未开工。R06风险决定、Phase0退出、P1实施授权分别记录；没有决定时只可继续准备材料。
3. [兼容评估](COMPATIBILITY_INTEGRATION_ASSESSMENT.md)明确Foundation/V+ PC/EasyTier目标与现有WebRTC的差距；必须先固定具体目标、来源/许可、角色/单UI/进程合同，再决定原型。当前没有兼容实现或第三方运行依赖。
4. [ADR0014](ADR/0014-phase0-final-review-and-risk-gates.md)更新验收门槛；[ADR0015](ADR/0015-client-backend-and-compatibility-plan.md)记录后端提案。旧ADR0012/13保留当时事实，不据旧授权启动服务。

## 代码导航与边界

| 位置 | 现有用途 / 后续约束 |
| --- | --- |
| `apps/desktop/src/App.vue`、`bridge.ts` | 当前实验壳与真实能力IPC；P1将分离产品页面，不能把概念图示例当真数据。 |
| `apps/desktop/src/features/p004`、`features/p006` | 已验证实验UI/浏览器媒体控制与独立统计；保留授权、统计语义及默认隔离。 |
| `apps/desktop/src-tauri/src/main.rs`及p004/p006/p007b模块 | 原生权限/会话/采集入口；新增IPC/托盘前需最小capability及生命周期测试。 |
| `crates/hub-capture-win`、`hub-encode-win`、`hub-webrtc` | DXGI/MF、标准WebRTC集成、scope、控制及清理；新UI不重写底层。 |
| `hub-turn-credentials`、`hub-probe-wire`、`hub-rendezvous-probe` | 服务端短票/实验信令，不等于正式身份。长期密钥不可进桌面包。 |
| `docs/PHASE0_*`、`artifacts` | 历轮证据只读；私有画面/票据不进Git。不因刷新报告而回写旧PASS。 |

## 审查和变更流程

先看最新main/工作树、AGENTS/TASKS和当前决议；需求涉及协议、候选范围、后端或权限时先ADR与契约。明确区分“现有API”和“规划字段”；每项测试标版本、拓扑、源类型和原生/Edge差别。

本轮离线复核：`python scripts/p007-final-review.py`；它只更新新的最终评审索引和verification，不运行媒体/云脚本。历史P007A复核仍为 `python scripts/p007a-review-evidence.py --check`，其旧判断保留当时状态。不要运行旧生成脚本覆盖历史证据。

未来构建/回归遵循各具体任务脚本和锁文件；没有源码变动时不把旧41/33项回归说成本轮新结果。新增真实Windows/云验证须相应授权，服务限时、先判断Allocation再停服、清临时凭据及不自启；不以停止整个TURN伪造正常释放。

完成当前文档交付即等待正式审查；任何风险接受、退出批准或P1开工都要有用户新决议记录，不能由状态脚本推断。
