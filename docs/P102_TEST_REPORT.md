# P1-02 首页、设备、连接、会话、设置导航报告

日期2026-11-22；基线GitHub main `fdfb9e359ee9a69ab46746ca2d97c2a8a1b99991`，开工本地干净/远端一致。用户审查P1-01后只授权本任务；R06已接受为未关闭剩余风险，Phase0限定退出已批准。本轮不启动阿里云、不调用真实桌面采集或远控输入，不进入P1-03/04。

## 页面与交互

复用Tauri v2/Rust/Vue3/TypeScript/Tailwind及现有工程，无新增依赖。普通构建以概念图的深色侧栏、蓝色选中态、浅色卡片工作区呈现五页；SVG图标由代码提供，无外部图片/字体请求。

| 页面 | 实际内容 | 尚未接入能力的呈现 |
| --- | --- | --- |
| 首页 `#/home` | 真实get_runtime_info检测状态、只读刷新、设备/连接概览及跨页入口 | 设备尚未接入、无会话路径/延迟/画面统计，不复制概念图假设备ID/绿灯/帧率。 |
| 设备 `#/devices` | 设备列表区域与连接页入口 | 明确“设备列表尚未接入”，添加设备禁用并附原因。 |
| 连接 `#/connect` | 真实不可用状态、设备页入口和双方同意说明 | 未接入的连接按钮禁用，不模拟进度、配对或P2P/TURN成功。 |
| 会话 `#/sessions` | 会话记录空态 | 无记录能力，不编造时长、历史会话或0延迟。 |
| 设置 `#/settings` | 核心返回的版本/OS/架构/后端、远程访问只读状态和重新检测 | 未读取字段保持“未读取”；后台/自启设置尚未提供，无假开关或持久化。 |

固定allowlist hash导航支持直接链接、刷新、前进/后退、键盘Enter、当前页标识和跳到正文。未知hash及带自动开始参数的输入回首页，不解析连接指令或票据。页面切换将焦点移至标题；跳到正文不改路由。能力失败清除旧值、可重试，错误采用固定可读反馈，不透出原生私密错误文本。

旧App提取为features/experiments/ExperimentWorkspace，原采集同意fixture测试继续保留；显式loopback/LAN/relay/diag构建仍显示原实验流程，普通壳不渲染采集/远控按钮。原生能力初值复用，不重复读取。Rust Session/候选/SDP/DTLS指纹/授权/过期、CSP/capability和锁文件均未修改；tauri.conf仅默认窗口标题改为RemoteHub，专用实验标题继续原setup覆盖。

## 测试与编译

入口 `pnpm verify:p102`复用P1-01检查函数，仅写artifacts/p102；原P1-01及Phase0证据未覆盖。浏览器输出由Node环境的e2e.mjs管理，测试用TestInfo.outputPath，无新增Node类型依赖。原P1-01质量入口仍可使用。

| 项目 | 本轮结果 | 证据边界 |
| --- | --- | --- |
| 质量命令 | **47/47**符合预期退出码 | [quality-results](../artifacts/p102/quality-results.json)，包含安全负例预期非零。 |
| 前端单元 | **43通过** | 含五页导航、真实字段映射、空态、失败重试/旧值清除、并发刷新、非法hash和实验隔离；原3项采集测试为明确fixture，不调用真实采集。 |
| Edge | **9通过** | 五页实际点击/截屏、直接链接/刷新/历史、非法hash、键盘/跳正文、900/640/450像素布局无横溢，以及原2项WebRTC属性用例；不是双机媒体测试。 |
| 证据目录保护 | **2项拒绝通过，Edge定向复跑9通过** | 父目录越界及跨盘输出在本地服务器启动前被拒绝；保护补充后正常路径再次运行真实Edge。见[负例](../artifacts/p102/browser-path-negatives.json)和[复跑](../artifacts/p102/browser-final-guard.json)，不计为额外独立UI用例。 |
| 构建清单 | **8种真实Cargo解析、9项隔离测试通过** | metadata不是运行授权或媒体验证。 |
| Rust | 全feature **82通过/1ignored**；默认 **56通过/1ignored** | 私有真实MF样本仍ignored，不能相加为138个独立用例。 |
| Python工具 | **12通过** | 离线协议/生命周期用例，不连云。 |
| 类型/格式/clippy/构建 | 适用默认/全feature/实验组合均通过 | 默认Windows原生EXE及relay/direct/diag实际编译；未创建新Release/诊断ZIP。 |

首轮类型检查发现旧实验可选初值与浏览器测试Node类型耦合，已修正；首轮Edge8/9的唯一失败是定义列表term定位方式，已改为内容定位，最终9/9。见[开发期问题记录](../artifacts/p102/development/issues.json)，没有将失败写成PASS。

## 视觉与原生验证

已目检真实Edge的[首页](../artifacts/p102/screenshots/home.png)、[设备](../artifacts/p102/screenshots/devices.png)、[连接](../artifacts/p102/screenshots/connect.png)、[会话](../artifacts/p102/screenshots/sessions.png)、[设置](../artifacts/p102/screenshots/settings.png)及[450像素布局](../artifacts/p102/screenshots/settings-450.png)。截图是浏览器无法读取本机核心时的真实空态，没有伪造原生核心成功。小宽度为布局覆盖；**未独立验收所有Windows DPI或系统缩放比例**。

启动本轮实际默认Tauri EXE，原生可访问性观察取得[首页文本](../artifacts/p102/native/home.json)和[设置文本](../artifacts/p102/native/settings.json)：本机核心已连接、远程访问关闭，设置实际返回版本0.1.0、windows、x86_64及DXGI backend。辅助截图首次返回非产品背景图，已丢弃、未入Git；之后原生仅使用文本观察。检测到并发人工输入后已停止自动点击，窗口保留给用户查看；**其余三页原生自动点击没有完成，不列为PASS**。五页交互由真实Edge覆盖；原生结果及截图限制见[native/result](../artifacts/p102/native/result.json)和[限制记录](../artifacts/p102/native/screenshot-limitation.json)。

没有调用RemoteHub真实桌面/输入入口，没有新云端或双机串流、原生实验同意、诊断票据或媒体启动。工具无效截图被丢弃的事实单独保留，不把浏览器截图冒称原生像素证据。

## 范围、复核及交付

新增[ADR0017](ADR/0017-p102-local-navigation-shell.md)和[P1-02计划](P102_PLAN.md)，更新当前任务/指引。离线复核入口`python scripts/p102-evidence.py`检查本轮计数、原生实际范围、截图哈希、能力边界、CSP/权限/依赖锁/旧证据不变及隐私；结果见[verification](../artifacts/p102/verification.json)。历史旧报告和R06未复现/根因未知结论保持。

P1-02实现完成后提交GitHub等待审查；P1-03设备编辑/连接弹窗/会话工具栏、P1-04 Pinia/新产品IPC、托盘/设置持久化及三件套运行集成都未开工。未接入能力保持空态，不把产品壳当可发布远控成品。
