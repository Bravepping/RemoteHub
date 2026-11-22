# P1-04 Pinia状态管理与类型化Tauri IPC报告

日期2026-11-22；开工main/origin均为已审查`481e17e5587186c08ee262d2159c1d8454dd7b0e`，工作树干净。仅P1-04，不启云、不采集桌面、不执行键鼠注入，不进入P1-05/06。R06技术仍OPEN_NOT_REPRODUCED/历史根因NOT_DETERMINED。

## 实际交付

- 精确Pinia3.0.4；每应用独立store与DesktopApi依赖。App通过storeToRefs共享本机runtime/status/error，五页导航保留状态，重复读取合并；无持久化、设备实体或假会话。
- 类型化只读DesktopApi复用真实get_runtime_info，不暴露任意命令字符串；RuntimeInfo必填/可选字段有运行时类型/长度/已知字段校验并冻结副本。旧bridge兼容导出，原采集函数/实验模板不变。
- 固定错误分类与脱敏消息，5秒上限、手动重试、取消与代次隔离；取消/卸载清计时器/监听，旧成功、错误、finally不能覆盖新代次。原生invoke已发出的读取不能真正撤销，取消只停止等待，不声称停止了Rust业务或媒体。
- 默认真实Tauri适配器；浏览器仍明确不可用，测试替身仅应用级显式注入，同一接口和响应校验。没有产品mock模式或URL开关。正式设备/会话API与事件未实现，保留空态/禁用，不增加假的订阅或空操作命令。

详见[计划](P104_PLAN.md)、[合同](P104_IPC_CONTRACT.md)和[ADR0019](ADR/0019-p104-pinia-typed-readonly-ipc.md)。Rust/媒体/权限/候选/SDP/指纹/批准/超时边界、实验feature和Tauri配置均未变；没有为Pinia增加shell/fs/network插件或后台访问。

## 实际验证及证据类型

入口`pnpm verify:p104`，独立写artifacts/p104，复用47项既有基础/安全检查，全部符合预期退出码；负例的非零表示正确拒绝。见[完整结果](../artifacts/p104/quality-results.json)。

| 项目 | 结果 | 边界 |
| --- | --- | --- |
| 前端单元/集成 | 79通过 | 包括真实Pinia+显式测试API、应用实例隔离、刷新合并、失败清旧值/重试、取消前不调用、迟到成功/错误/finally隔离、scope销毁；SDK传输替身验证类型/字段/超长/控制字符/未知字段/继承对象、固定错误、超时/取消/监听和计时器释放。原实验同意测试仍fixture。 |
| Edge | 15通过 | 继承14项真实导航/卡片/modal/工具栏/布局/WebRTC属性；新增1项明确SDK替身，真实Pinia驱动跨页共享/失败清值/脱敏/重试，命令记录仅get_runtime_info共3次。不是原生IPC或双机结果。 |
| Rust | 全feature82通过/1ignored；默认56通过/1ignored | 两组重叠不相加；私有MF样本仍ignored。 |
| 构建隔离/工具 | 8真实Cargo配置、9隔离测试、12 Python通过 | metadata不是运行或媒体授权。 |
| 类型/格式/lint/clippy/编译 | 适用组合及默认Windows EXE通过 | Rust/默认/实验原生编译不等于新媒体测试。 |
| 真实原生只读 | 默认Tauri/WebView2首页文本观察通过 | 新编译程序显示本机核心已连接、windows/x86_64、远程访问关闭，确认实际Rust IPC→响应校验→Pinia→界面。没有点击、键盘动作或截图；只验证首页读取，不扩成原生刷新/超时/取消验收。 |

原生证据见[readonly-home](../artifacts/p104/native/readonly-home.json)。程序留在本机首页供用户查看，无活动会话/采集；本轮没有自动操作实验入口。浏览器默认[首页](../artifacts/p104/screenshots/home.png)/[设置](../artifacts/p104/screenshots/settings.png)保留真实不可用态；[SDK替身截图](../artifacts/p104/screenshots/p104-explicit-sdk-fixture.png)顶部有醒目标记和fixture字段，不能当原生成功证据。图片SHA见[visuals](../artifacts/p104/visuals.json)。

首轮类型检查的两个错误（可选options被推断为{}、注入服务在回调内丢失非空收窄）已通过显式参数注解和非可选局部绑定修正，随后类型/79单元/完整回归通过。记录见[开发问题](../artifacts/p104/development/issues.json)，没有把初轮失败标PASS。

## 依赖与安全复核

Pinia官方元数据为MIT，3.0.4与现有Vue3.5.46/TS6.0.3匹配。未采用4.x的新增依赖机制；精确新增Pinia与11个传递包，既有锁记录不删除/升级，Cargo锁不变。安装禁用脚本，12个新增包均无preinstall/install/postinstall；许可证为MIT或BSD-3-Clause，已核对speakingurl的BSD正文，后续二进制分发应保留相应版权/许可。本轮未发布新安装包。

本次实际pnpm audit报告0已知漏洞（仅反映当时库覆盖，不是无漏洞保证），见[审计](../artifacts/p104/dependencies/audit.json)、[依赖审查](../artifacts/p104/dependencies/review.json)、[新增包与许可](../artifacts/p104/dependencies/new-packages.json)和[固定版本元数据](../artifacts/p104/dependencies/pinia-metadata.json)。未调整原release-age策略或系统包管理器。

离线`python scripts/p104-evidence.py`核对计数、依赖差异、图片SHA、原生观察范围、App模板/原采集函数、Rust/权限/历史证据不变、文档链接与脱敏，见[verification](../artifacts/p104/verification.json)。自审未发现阻塞/严重问题；没有任意命令代理、假授权、自动重试或新网络请求。Pinia仅显示状态，任何前端修改都不能成为原生会话批准。

## 限制与收尾

设备身份/正式配对/会话命令及事件依然未实现；本轮不承诺全产品状态机或媒体停止。没有事件源时不伪造订阅；未来接入必须按真实会话/代次/终态独立设计。原生只读首页以外的交互、故障注入、Win11/全DPI、双机/云/新视频测试未执行。取消不能回收Tauri内部尚未完成的native callback，这个只读命令本身同步返回，没有新增长任务。

旧报告/统计/历史失败不改；R06仍开放，P004已批准例外不重开。README/TASKS/当前方案与指引已更新，GitHub交付SHA见本轮提交/回复。正常Git提交可回退本轮状态层及锁文件，不重写历史。只完成P1-04，提交后停止等待审查。
