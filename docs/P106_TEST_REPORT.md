# P1-06托盘与退出生命周期测试报告

日期2026-11-22；基线main/origin为`7f36b393658e2050fa9f976bc95524bb48893798`，开工工作树干净。范围仅本轮托盘/退出和Phase 1退出评审准备，不启动阿里云、不新测双机、不采集真实桌面、不执行实际远端输入、不启动第三方软件、不修改驱动/路由。先写[计划](P106_PLAN.md)及[ADR0021](ADR/0021-default-tray-and-safe-exit-lifecycle.md)，复用工程和原媒体实现。

## 最终实现与适用范围

| 场景 | 实现行为 | 边界 |
| --- | --- | --- |
| 普通桌面默认点击X | 阻止直接销窗，统一进入真正退出流程 | 默认不隐藏；每次进程启动恢复该默认 |
| 原生托盘主动勾选“本次允许托盘驻留” | 无任务且图标可用才启用；之后X隐藏 | 无持久化/开机自启，无新增IPC或前端权限开关 |
| 托盘显示/退出 | 显示、取消最小化并聚焦；退出撤销驻留/阻止新采集，重复退出不创建额外线程 | 显示不能复活退出中的实例；退出事件移除自有托盘对象 |
| 取消驻留 | 恢复前台窗口，下次X退出；重复取消幂等 | 不改变远程权限 |
| 托盘创建/隐藏失败 | 不允许驻留或转退出 | 不把失败当成功隐藏，不依赖托盘恢复不可见进程 |
| 普通构建的既有有界采集租约 | 采集繁忙时拒绝驻留；驻留/退出中拒绝采集；退出等待租约释放 | 本轮不调用采集；原任务上限5秒，等待上限7秒。超时恢复可见并提示重试，不强杀冒充释放 |
| 原p004及其LAN/relay/direct/diag实验构建 | 不安装托盘生命周期，原关窗停止/回收handler保留 | 不因驻留而后台继续实验；未在本轮重测真实视频关闭 |

普通采集/驻留/退出共用同一策略锁和busy所有者，租约Drop释放busy；取消驻留先在菜单主线程恢复窗口，再放开采集门禁，恢复失败转退出。超时工作线程显示窗口前释放策略锁，菜单对象也克隆后释放其锁再调用原生方法，避免锁跨线程调度互等。不添加网络、输入或采集自动启动。菜单完全由Rust管理。设置页只说明真实入口，不显示未经读取的当前勾选状态。视频源、编码、RTP、ICE、控制和独立最终统计源码未修改。

## 权限审计

开启Tauri既有`tray-icon` feature后，原`core:default`会包含前端tray/menu默认命令。此次将其拆为原有path/event/window/webview/app/image/resources默认集合，移除前端tray/menu访问；既有实验自定义命令权限、main/local窗口范围和CSP不变。没有新IPC、shell、文件系统或第三方运行权限。

`scripts/quality/p106-permissions.mjs`解析本机构建生成的真实ACL manifest，比较原集合与当前集合的允许命令，要求新增为0、tray/menu允许命令为0；7项测试覆盖真实解析、恢复宽默认、tray/menu/shell扩权、重复权限、外部窗口和远程来源拒绝。计数及capability哈希见[有效权限证据](../artifacts/p106/permissions.json)。不是只凭配置行数声称未扩权。无新依赖版本，Cargo/pnpm锁不变。

## 本轮真实执行结果

| 检查 | 结果 | 证据类型 |
| --- | --- | --- |
| `pnpm verify:p106` | 47项主检查符合预期 | [质量汇总](../artifacts/p106/quality-results.json)；安全负例的预期非零为正确拒绝 |
| 前端单元/集成 | 88通过 | 既有状态/代次/隔离/空态及设置渲染；无模拟设备或统计 |
| 真实Edge | 18通过 | 五页/卡片/弹窗/工具栏/反馈回归；浏览器不是原生托盘验收 |
| Rust全feature | 82通过/1私有MF样本ignored | 原协议/ICE/权限/资源回归，未启动真实媒体 |
| Rust默认 | 62通过/1私有MF样本ignored | 增加4策略反例、默认origin校验测试和busy租约释放；两组重叠不相加 |
| 权限回归 | 7通过 | [原始输出](../artifacts/p106/permission-tests.txt)；解析真实manifest只减不增 |
| 构建配置/生命周期 | 8真实Cargo配置、9隔离测试、12 Python生命周期通过 | 不代表本轮实机视频资源验收 |
| 附加隐私/交付 | 12隐私、9交付通过 | [结果](../artifacts/p106/privacy/results.json)，生成fixture而非真实网络端点 |
| 格式/类型/lint/clippy/构建 | 适用组合通过，默认Windows EXE编译通过 | 独立automatic日志；原生探针也完成编译 |
| 原生生命周期探针 | **NOT_COMPLETED** | 两次启动均返回`0xc0000139`，无native JSON，未验证托盘或退出动作 |

首次探针编译时用Result直接比较触发类型错误，改用`matches!`后构建及clippy通过；没有通过放宽检查消除错误。原生启动两次分别在受限运行环境和批准的正常Windows执行环境尝试，均立即以十进制3221225785结束；没有目标测试进程残留，隔离的WebView测试目录已删除。错误根因未定位，不能据此判定生产窗口或托盘功能通过/失败。保留[初次元数据](../artifacts/p106/native/initial-launch.json)、[再次元数据](../artifacts/p106/native/results.json)及[判定](../artifacts/p106/native/assessment.json)，不继续盲目重试或更改系统安全设置。

探针是独立Rust example，复用生产生命周期并创建真实Tauri/Wry窗口和托盘，通过原生API触发close/show/退出；测试用`run_return`收集退出结果，生产使用`run`及相同回调。计划覆盖默认X、驻留隐藏/重复显示/退出、取消驻留、模拟busy等待退出四例。两次启动发生在收尾锁顺序收紧前；最终源码重新编译通过后未再次盲目启动，最终构建与此前被执行的程序SHA分别记录，见[最终构建](../artifacts/p106/native/final-build.json)。此次第一例启动即失败，四例均不得算作原生PASS；即使将来API探针通过，也不能替代真实菜单点击或实际视频清理。busy分支只是租约模拟，没有采集。

## 用户确认、历史与退出材料

用户已明确亲眼看见P1-05 Windows原生窗口成功显示，按本次指令追加 **USER_CONFIRMED_WINDOW_VISIBLE**，见[追加记录](P105_USER_WINDOW_VISIBILITY_ADDENDUM.md)及[独立用户证据](../artifacts/p106/native/user-confirmation.json)。原P105定位失败日志及报告不改，五页原生逐页交互仍未完整验证。此次探针错误不撤销此前用户确认，也不要求重复确认窗口是否出现。

[Phase 1退出评审准备](PHASE1_EXIT_REVIEW_PREPARATION.md)已汇总代码/自动/原生/用户证据、未完成项和退出选项。原生托盘实操、完整五页原生交互、Win11/DPI和真实视频关闭未在本轮验证；不自动认定阶段退出。C0 [D1–D6决策单](C0_D1_D6_USER_DECISION_SHEET.md)均等待用户选择，自研WebRTC保持主线，第三方兼容后端未实现或启用；C1来源/许可冻结仍未完成。R06仍OPEN_NOT_REPRODUCED/NOT_DETERMINED，Phase0已接受的有限风险不扩大到产品可靠性。

## 收尾与可复核性

`python scripts/p106-evidence.py`仅离线复核实际计数、变更白名单、权限收窄、旧报告/媒体/锁不变、敏感信息与链接、程序SHA和原生限制，见[verification](../artifacts/p106/verification.json)。其passed只表示离线证据自洽，原生验收字段仍NOT_COMPLETED。默认EXE及探针SHA均来自本轮实际构建，不当签名/发布安装器或实机PASS。

README、TASKS、开发指引/方案、ADR和评审准备同步；未重写历史或刷新旧证据。测试后无本轮原生测试进程，临时WebView目录清理；没有云服务操作或远程资源可冒充回收。可通过正常后续提交回退本轮变更，不删除用户数据。提交GitHub后停止扩展，等待审查；不进入Phase 2或第三方兼容原型。
