# P1-05中文状态/错误反馈 + C0-DRAFT报告

日期2026-11-22；基线main/origin `9418db2b6ff83cc2207b89df077a737e6580b825`，开工干净。用户只批准本轮两项，不启云/双机串流、不采集真实桌面、不注入远端输入、不安装或启动三件套、不扩大Tauri权限。R06仍OPEN_NOT_REPRODUCED/根因未定，不进入P1-06或兼容原型。

## 实现与状态语义

| 请求结果 | 界面实际反馈/动作 | 数据保证 |
| --- | --- | --- |
| 首次读取 | 正在读取本机信息，可取消 | 未取得字段不填默认值 |
| 刷新 | 正在刷新，等待本次结果；可取消 | 开始即清上一轮runtime，设置字段显示读取中，不把旧值冒充新结果 |
| 原生IPC失败 | 请重新检测，仍失败可关闭重开客户端 | 不再提浏览器；未知异常只经固定错误码重建消息 |
| 浏览器不可用 | 明确浏览器预览不能连接本机核心 | 仅BROWSER_UNAVAILABLE使用该原因 |
| 超时/合同错型 | 固定超时/无法识别反馈，手动重试 | 5秒上限与严格类型/字段/长度校验不变，不自动重试 |
| 取消 | 本次读取已取消，旧信息不保留，可重试 | 代次失效/清timer和监听；迟到成功、错误及finally不能复活旧状态 |
| 恢复/完成 | 首读、刷新完成、失败或取消后重新读取各自提示 | 仅本代次真实成功才显示完成，前端不授予远控权限 |

全页ReadFeedback分别用status/alert，提供取消和手动重试；首页/设备卡片/设置字段与本轮数据一致。卸载reset静默清状态，不误显示用户取消。取消只停止等待只读IPC，不声称撤销Rust任务或媒体会话。现有Pinia实例隔离/刷新合并保持，原实验feature/同意流程不改。未接入的设备/连接/会话能力继续空态，路径/RTT/FPS没有假值。

## C0交付与未批准事项

已补交[指定合同草案](C0_COMPATIBILITY_TARGET_CONTRACT_DRAFT.md)：Foundation Sunshine原生主机↔Moonlight V+ PC原生客户端的互操作方向、统一UI/单安装器、受管理进程/独立播放窗例外、EasyTier承载、配置与凭据隔离、停止/崩溃/升级、版本来源/许可及D1–D6待决清单。不是WebRTC“借鉴体验”替代目标；C0正式签署/C1冻结/兼容实现仍未完成。

只读查看了公开候选仓库/许可文本，记录分支观察commit、LICENSE SHA和时间；公开文本头Foundation/PC为GPL v3、EasyTier为LGPL v3，不能替代冻结版本、依赖及最终分发的许可核验。PC候选qiin2333/moonlight-qt与Android仓库qiin2333/moonlight-vplus已区分。见[来源观察](../artifacts/p105/c0/public-sources.json)。没有第三方二进制下载/运行、安装器执行或源代码移植；旧本地包只引用旧哈希，未在本轮重新验证其来源或签名。

## 实际回归

`pnpm verify:p105`复用既有基础/安全套件，独立写artifacts/p105；47项命令全部符合预期退出码，安全负例预期非零是正确拒绝。见[quality-results](../artifacts/p105/quality-results.json)。

| 检查 | 本轮结果 | 证据边界 |
| --- | --- | --- |
| 前端单元/集成 | 88通过 | 中文映射/原生vs浏览器、刷新清旧值、取消/恢复/reset/代次、旧请求成功/错误隔离及原实验fixture |
| 真实Edge | 18通过 | 原导航/modal/工具栏/布局；SDK故障替身验证刷新→取消→迟到→恢复、敏感异常/合成端点与token不显示、合同错型、实际等待5秒超时→手动恢复。替身有醒目标记，不当原生故障结果 |
| Rust | 全feature82通过/1ignored；默认56通过/1ignored | 两组重叠不相加，私有MF样本仍ignored |
| 构建隔离/工具 | 8真实Cargo配置/9隔离测试；12生命周期Python | 非云端/媒体测试 |
| 类型/格式/lint/clippy/构建 | 适用组合及默认Windows EXE通过 | 不替代原生五页验收 |
| 隐私与交付工具 | 12隐私/9交付自动回归通过 | 随机/文档地址fixture，无生产端点；见[附加结果](../artifacts/p105/privacy/results.json) |

本轮类型/单元/Edge及47项主回归均首轮通过；没有为通过测试修改底层安全、超时或候选白名单。默认打包资源无SDK替身标记/故障载荷。旧报告/统计与锁不改，Tauri权限/CSP及Rust/媒体均不变。

## Windows原生补验：未完成，未记PASS

最终默认EXE编译通过后，以窗口工具启动了两个本轮测试实例（一次初试/一次重试），但工具始终无法返回目标窗口，因而没有可靠的页面导航/文本观察入口。只读进程检查与提升权限后的检查对窗口句柄可见性有差异，**原因未定位，不能据此判定应用UI本身故障或工作正常**。没有切换到其他应用或绕过输入保护，也没有native截图/点击/键盘动作。

初次清理权限不足，提升后的“无窗口”前置条件拒绝而未操作；随后按程序路径、精确启动时间及仅本轮两个实例的进程身份结束测试，立即复查曾短暂仍可见，后续独立核对剩余0。本轮启动的测试实例已清理，没有按名称关闭其他应用。证据/程序SHA见[native/result](../artifacts/p105/native/result.json)。

本轮原生首页/设备/连接/会话/设置均**NOT_COMPLETED**；不引用P1-04首页或Edge结果补写PASS。用户无需本轮立刻配合；后续如需原生补验，应先恢复可靠的目标窗口观察，并在不影响人工操作的范围另行执行。五页原生验收及Win11/全DPI仍是限制，不隐藏为全部验证完成。

## 交付与限制

真实Edge图：[默认首页](../artifacts/p105/screenshots/home.png)、[默认设置](../artifacts/p105/screenshots/settings.png)、[刷新替身](../artifacts/p105/screenshots/refreshing-sdk-fixture.png)、[原生错误替身](../artifacts/p105/screenshots/native-error-sdk-fixture.png)、[超时替身](../artifacts/p105/screenshots/timeout-sdk-fixture.png)、[恢复替身](../artifacts/p105/screenshots/recovered-sdk-fixture.png)，SHA见[visuals](../artifacts/p105/visuals.json)。默认浏览器不可用和显式SDK替身分别标注，均不冒充原生验收。

[计划](P105_PLAN.md)/[ADR0020](ADR/0020-p105-request-feedback-and-compatibility-draft.md)和当前README/TASKS/开发方案已更新。离线`python scripts/p105-evidence.py`检查计数/变更白名单/隐私/链接/SHA/原生限制/C0未批准状态，见[verification](../artifacts/p105/verification.json)。自动检查passed只指离线检查，不提升native NOT_COMPLETED或C0 DRAFT。

未发现新增阻塞/严重源码问题；原生窗口工具可见性仍未解决，兼容来源和用户决策未闭环。没有新设备身份/会话命令、云服务/双机/真实桌面/远端输入/第三方运行或系统配置变更。正常Git提交可回退本轮界面，不重写历史。提交GitHub后停止扩展，等待审查。
