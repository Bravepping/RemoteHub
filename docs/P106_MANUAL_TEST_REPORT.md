# P1-06普通Windows客户端人工验收报告

日期2026-11-22；测试应用源码main为`ddbe191424c2a7379b43d81ac58f746ea32a6997`，开工工作区干净，fetch后与origin/main一致。用户阶段性接受该版实现/权限/离线回归，本轮另行明确授权本机普通UI人工测试；未授权Phase 1正式退出或Phase 2。应用源码、权限、媒体与依赖锁本轮均未修改，新增内容仅人工步骤、编译/只读进程观察工具、证据与评审文档。

## 被测程序与方法

本机Windows，.NET只读版本及系统位数见[环境记录](../artifacts/p106-manual/environment.json)。CIM查询拒绝访问，初次空结果保留，未据此核验具体系统版次、未修改系统权限。普通桌面编译已执行前端类型检查/Vite构建及Rust优化构建：`cargo build -p remotehub-desktop --bin remotehub-desktop --profile probe --no-default-features --features custom-protocol --locked --offline`。实际Cargo metadata仅解析到桌面feature `custom-protocol`，不含p004/p005/p006/诊断feature；profile名probe只表示优化配置，不意味着启用实验功能。

本机EXE相对仓库根路径：`artifacts/private/p106-manual/RemoteHub-P106-Manual.exe`。这是本地文件，Git忽略，不是GitHub下载链接；其内容与本轮编译的`target/probe/remotehub-desktop.exe`一致，仅重命名以隔离测试进程。无媒体/云配置或凭据。

SHA256：`be56951a4c79a0749fd7688c6c9a55ba9a9ffed777cb6ce78daeae9fbb3be315`。构建和每次绑定均核对同一哈希，测试结束后再次核对；见[build.json](../artifacts/p106-manual/build.json)。没有自动启动目标程序、再次运行旧原生探针或自动操作任何窗口/菜单。

用户亲自逐步启动、点击X/托盘/五页、隐藏/恢复/退出，Agent在每个退出动作前绑定唯一PID、启动时间、精确程序路径和SHA，并持有该进程句柄等待退出信号。只读观察器不启动、隐藏、注入、截屏或结束目标进程。视觉/菜单结果记`USER_RESPONSE`，进程证据独立记录，不用Edge或模拟结果替代；[计划](P106_MANUAL_ACCEPTANCE_PLAN.md)和[用户原话](../artifacts/p106-manual/user-observations.jsonl)可逐步核对。

## 人工步骤与进程结果

| 步骤 | 用户本轮确认 | Agent独立核对 | 结论 |
| --- | --- | --- | --- |
| A1/A2 默认X | 打开指定EXE，未启驻留，X后窗口关闭且未重开 | 唯一PID13036/SHA匹配；WaitForExit返回，后续枚举同名进程0 | 普通默认关闭退出通过 |
| B1 重开默认 | 驻留默认未勾选 | 新唯一PID20660，与A不同 | 本进程默认未继承驻留通过 |
| B2 主动驻留 | 勾选后X隐藏，托盘图标仍在 | 同一PID20660仍运行且实例数1 | 主动驻留隐藏通过 |
| B3 显示/重复显示 | 恢复正常，重复显示仍只有一个窗口 | 同一PID20660/启动时间匹配，无新实例 | 恢复与重复显示通过 |
| B4 取消驻留 | 再隐藏后取消勾选，窗口自动恢复、菜单未勾选 | 同一PID20660仍运行且唯一 | 取消驻留恢复通过 |
| B5 取消后X | X后窗口关闭，未重新打开 | 绑定PID20660退出信号，后续枚举0 | 恢复默认退出通过 |
| C1 五页/只读检测 | 五页正常、重新检测成功、空态真实，保持打开 | 新唯一PID22668/SHA匹配，页面确认后仍运行 | 五页基本导航及本机只读刷新人工通过 |
| C2 真正退出 | 启用驻留并X隐藏后，通过托盘真正退出，窗口和图标均消失 | PID22668退出信号，后续枚举0 | 托盘真正退出通过 |

三次进程：A启动18:32:05，观察退出18:32:54；B启动18:33:52，观察退出18:37:14；C启动18:37:53，观察退出18:39:02，均为Asia/Shanghai。完整UTC启动时间和带时区观察时间在[实例A](../artifacts/p106-manual/case-A-process.jsonl)、[实例B](../artifacts/p106-manual/case-B-process.jsonl)、[实例C](../artifacts/p106-manual/case-C-process.jsonl)。观察时间不等于用户点击时间，用户原话的agentRecordedAt只是归档时间。最终[清理记录](../artifacts/p106-manual/cleanup.json)为目标进程0、三个观察会话均完成，未强杀目标或其他用户应用。

## 证据能力与保留限制

本次进程接口仍未提供可用主窗口句柄，数值退出码也均为null；没有记“退出码0”或独立自动视觉PASS。退出信号/进程清零可核对，窗口显隐、菜单勾选、图标消失、页内容由用户现场确认。这些来源分别标注，不以无句柄推断启动失败或已退出。A初版观察器尚未显式输出exitCodeAvailable/processWaitSignaled字段；其EXIT_OBSERVED同样只在WaitForExit返回true后生成，原日志保留，未追填字段。

本轮七项生命周期场景和五页基本导航/重新检测完成，不等于取消/超时/故障恢复等所有原生状态已逐项测完，也没有原生截图或逐页自动文本轨迹。重复显示有实际人工和PID证据；重复退出重入、采集租约等的单元/离线证明仍按上一轮报告，不将其伪称本轮实际操作。没有活动视频关闭实测、真实采集/输入/双机/云、Win11、全DPI或系统关机/崩溃测试。

旧P105/P106报告、探针两次`0xc0000139`及当时NOT_COMPLETED结论未改。**普通桌面本轮实操通过，不代表独立探针失败根因已定位/修复**；探针问题仍NOT_DETERMINED。此前USER_CONFIRMED_WINDOW_VISIBLE继续保留。本轮无需再次依赖窗口自动化定位或反复启动来证明显示。

## 本轮自动复核与继承结果

本轮普通前端/Rust构建通过，两个PowerShell工具语法错误0；新增证据门禁12测试通过，包括缺步骤/重复响应、错误冒充成功、Edge替身、重复实例、PID变化、强杀、已知非零退出/残留、实验feature、错误SHA、观察超时等反例。附加12隐私、9交付、7实际权限回归通过，见[回归原始日志](../artifacts/p106-manual/regression/results.json)。这些内存变异测试不会写入真实UI记录，也不是原生实操结果。

应用源码/权限/锁未变，本轮没有重复47项完整离线套件或另跑Edge/Rust全单元。`ddbe191`已获阶段性验收的47检查、88前端/18Edge、Rust全feature82/默认62各1ignored和其他安全结果，继续作为该冻结应用源码的继承证据；详见[原P106报告](P106_TEST_REPORT.md)，不重标为本轮新跑。

`python scripts/p106-manual-evidence.py`复核原话/PID/SHA/退出信号/快照、隐私、源码/历史不变和链接，输出[verification](../artifacts/p106-manual/verification.json)。原始日志UTF-8 LF规范化后哈希，退出码未知仍保留未知。该审核不会启动程序或网络。

## 收尾

P1-06按本机普通UI限定验收更新为done。见[Phase 1最终退出评审](PHASE1_FINAL_EXIT_REVIEW.md)：原生托盘实操缺口已在此范围闭环，阶段退出仍AWAITING_USER_REVIEW，不是已批准退出/发布。C0 D1–D6/C1仍待用户决定，自研WebRTC保持主线，第三方后端未启用；R06技术问题继续OPEN_NOT_REPRODUCED。保留本机EXE供复核，不含配置/凭据。提交GitHub后停止扩展，不进入Phase 2。
