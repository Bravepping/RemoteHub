# P1-01 工程基础与质量检查交付报告

日期：2026-11-22；GitHub main实施基线 `767ff44cba1690fce69c417f82c6531ce3fcee14`，开工工作树干净且已fetch核对。用户已明确接受R06未关闭剩余风险并批准Phase0限定退出，本轮仅授权P1-01。批准事实另记[退出决议](PHASE0_EXIT_DECISION.md)，旧评审/测试报告不回写。

## 实际交付

复用现有Cargo/pnpm工程、Tauri v2/Rust/Vue3/TypeScript/Tailwind及依赖锁，没有重建项目或新增依赖。新增 `scripts/quality/desktop-builds.json`列出default/loopback/LAN/control/TURN-probe/relay/direct/R06八种已有配置；用固定锁/离线的真正Cargo metadata解析核对，不模拟特性解析结果。默认desktop不加载可选WebRTC依赖，R06必须解析出synthetic-stream；解析结果不是媒体运行或授权证明。

根 `pnpm verify:p1`调用scripts/p101.ps1，固定写artifacts/p101；复用已有41项离线安全检查，增加构建清单单元/真实解析、默认workspace check/test/原生构建及TURN desktop feature检查，共47项命令。结果逐步写入，失败会保留非成功状态，避免旧成功摘要被误读为本次结果。旧检查入口保留兼容，不改云/媒体工具。

新增[IPC/状态合同](P101_DESKTOP_CONTRACT.md)及[ADR0016](ADR/0016-p101-foundation-and-quality-gates.md)：区分编译能力、实际handler、本轮授权、路径批准、live/last_known/unavailable和最终清理；R06拒绝不能映射成连接成功或校验绕过。本轮没有新增产品IPC、Pinia、导航、托盘、真实输入或兼容后端。

浏览器测试原先固定写早期artifacts/browser-preview.png。本轮改为按REMOTEHUB_VERIFY_DIR落本轮截图：首次运行生成的新图已保留到p101，并从冻结基线恢复旧图；修正后再次运行真实Edge测试，独立目录生成截图，旧图字节相同。这是测试证据输出隔离，不是桌面采集或产品UI变更。

## 实际执行与证据

主入口：`pnpm verify:p1`。本机使用现有工具链，PowerShell仅当前进程以ExecutionPolicy Bypass运行现有配置脚本，没有修改系统策略/全局PATH。曾直接dot-source被当前策略阻止，改用既有脚本入口后完成；不把该环境尝试当产品测试通过。

| 项目 | 本轮结果 | 证据 / 边界 |
| --- | --- | --- |
| 质量命令 | 47/47符合预期退出码（41复用+6基础） | [quality-results.json](../artifacts/p101/quality-results.json)，有些安全负例预期非零，不等同所有命令返回0。 |
| 构建配置 | 8/8真实Cargo解析通过 | [build-matrix.json](../artifacts/p101/build-matrix.json)，无应用/PeerConnection启动。 |
| 新隔离测试 | 9/9通过 | 默认误启relay/依赖、实验默认、loopback串LAN、direct认证基础丢失、R06合成丢失、缺resolve、非法清单请求等生成型负例；fixture不冒充真实Cargo。 |
| Rust全特性 | 82通过、1 ignored | 本轮真实cargo test --workspace --all-features --locked --offline；不把ignored私有MF样本标PASS。 |
| Rust默认 | 56通过、1 ignored | 本轮真实默认workspace test；与全特性是两种模式，不累计为138个独立测试。 |
| 前端 | 34通过 | 真实Vitest；typecheck/build/Prettier检查同时通过。 |
| Edge预览 | 3通过；截图隔离改动后又复测3通过 | [复核结果](../artifacts/p101/browser-isolation/result.json)、[截图](../artifacts/p101/browser-isolation/browser-preview.png)。均为浏览器预览/明确fixture，不能当新原生/双机媒体验收。 |
| Python协议/生命周期工具 | 12通过 | 继承的离线用例，不连接云服务。 |
| fmt / clippy | 默认和全特性、既有direct/diag组合通过 | 使用-D warnings，命令与日志独立保存。 |
| 原生构建 | 默认、relay/direct/diag组合实际编译通过，补TURN feature检查 | 默认probe EXE实际SHA/大小在[verification.json](../artifacts/p101/verification.json)；不生成新用户实验ZIP或发布Release。 |
| 诊断CLI拒绝 | 原有self-check及桌面源/故障覆盖参数拒绝通过 | self-check无GUI/媒体；R06既有限制保持。 |

额外只重跑了受截图路径修改影响的Edge测试及相关格式检查；没有为了抬高数字再跑不相关媒体。该补测可复现为设置`REMOTEHUB_VERIFY_DIR=artifacts/p101/browser-isolation`后运行`pnpm test:e2e`。主检查日志放 [automatic](../artifacts/p101/automatic/)；归纳/隐私/历史范围复核入口为 `python scripts/p101-evidence.py`。实际结果以verification及退出码为准，运行日志中Node对旧固定pnpm shell shim的DEP0190提示是继承工具实现的提醒，不是新增网络/输入能力；这些调用参数均是源码固定值，没有对端输入。

## 范围和已知限制

- Rust/Vue媒体源码、Tauri配置/权限、候选/SDP/指纹/授权、依赖锁与原生handler均未修改；只调整工程脚本、浏览器测试输出及当前文档。旧报告、证据和原图与冻结基线保持。
- 没有启动阿里云、加载有效票据、执行新双机或媒体实验，没有真实桌面采集/键鼠注入。原生GUI未新启动；本轮UI代码没变，现有窗口仍为Phase0实验壳，产品导航/标识将在P1-02等任务处理，不宣称新桌面产品界面完成。
- 构建清单核对metadata，不能代替真实运行时授权和网络验收；真实条件保证来自原安全校验，本轮没有改动它们。`--all-features`不是普通分发配置。
- R06风险接受=ACCEPTED_FOR_LIMITED_EXIT；技术OPEN_NOT_REPRODUCED、历史根因NOT_DETERMINED。原负例、诊断/终态要求和未来修复/关闭门槛继续有效。
- 本轮C0仅实现现有WebRTC工程基础；Foundation/V+ PC/EasyTier仍未核验具体来源/许可/API、未实现互操作，不部署第三方运行依赖。

## 收尾

P1-01工程基础和检查合同完成，提交GitHub后等待审查；P1-02至P1-06未开工，不自动继续。Phase0父项7/7为已批准限定退出，既非软件完成百分比，也不是R06已修复或发布/无人值守许可。
