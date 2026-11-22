# P1-03 UI卡片、连接弹窗与会话工具栏交付报告

日期2026-11-22；基线main `d0fd876e17ccaf2293075a9b15178ed97d98a018`，开工本地干净且origin一致。用户仅批准P1-03；不启云、不采集真实桌面、不执行键鼠注入，提交后等待审查。R06技术仍OPEN_NOT_REPRODUCED/历史根因NOT_DETERMINED，风险接受不等于修复。

## 实现及真实能力边界

| 内容 | 实际可用交互 | 未接入部分 |
| --- | --- | --- |
| PanelCard | 首页概览/设备列表/会话工作区与记录统一标题、图标、状态与操作槽 | 不生成远端设备集合或示例卡片。 |
| LocalDeviceCard | 首页和设备页复用真实get_runtime_info，显示读取中/已读取/不可得，可刷新 | OS/架构来自现有字段；本机响应不表示WS在线/对端批准。没有设备ID、主机名、虚构在线或性能统计。 |
| ConnectionDialog | 连接页打开准备弹窗，关闭按钮/右上关闭/Escape可用，回到触发按钮；页面离开关闭modal | 设备与授权“尚未接入”、协商与通道“未开始”；发起连接禁用并说明，不用定时器推进ICE/直连/中继。打开弹窗不是授权。 |
| SessionToolbar | 无会话画面工作区中浮动展示，可收起/展开、键盘操作及导航到连接页 | 全屏/画质/码率/断开禁用并附原因；路径暂无会话，RTT/FPS未取得。没有video/canvas或输入监听，不把空画面当串流。 |

沿用Tauri v2/Rust/Vue3/TypeScript/Tailwind、现有组件与受限hash导航，无新依赖/IPC/Pinia。样式受.product-shell限定；原实验、原生Rust及tauri.conf/CSP/capability、候选/SDP/指纹/授权/超时、依赖锁均不改。P1-04真实产品状态与命令未接入；本轮没有实现未来网络状态、设备编辑或正式独立远控窗口。

## 实际测试

`pnpm verify:p103`复用离线基础和既有安全负例，只写artifacts/p103。正式47项命令全部符合预期退出码，负例的非零是拒绝通过，非网络测试成功。见[质量结果](../artifacts/p103/quality-results.json)。

| 项目 | 本轮结果 | 证据与范围 |
| --- | --- | --- |
| 前端单元/集成 | 50通过 | 卡片字段/加载/失败清旧值/刷新、远控flag不转授权、工具栏禁用/折叠/本地导航；原实验同意测试是明确fixture，不采集桌面。 |
| 真实Edge | 14通过 | 继承9项导航/布局/WebRTC属性；新增5项modal焦点/背景inert/Escape/关闭回焦点、路由卸载、工具栏折叠/禁用和900/450小窗。没有mock本机核心成功。 |
| Rust | 全feature82通过/1ignored；默认56通过/1ignored | 两组有重叠，不能相加作独立测试数；私有MF样本仍ignored。 |
| 构建隔离 | 8真实Cargo解析配置、9隔离测试通过 | 默认构建不引入可选WebRTC实验依赖；metadata不等于运行授权。 |
| 工具与构建 | 12 Python；类型/格式/lint/clippy适用组合、实验/默认Windows EXE编译通过 | 既有协议、授权、配置缺失/占位拒绝回归，不访问云端。 |

首轮Edge12/14：一项错误假设原生modal的反向Tab必须直达最后按钮；实际观察为Edge把焦点交给浏览器栏，页面背景仍inert。测试现验证“弹窗内或浏览器栏”、对背景元素程序化focus仍拒绝、正向Tab与关闭恢复。另一项只滚动顶部折叠按钮却要求整个工具栏底部操作同时可见；改为滚动完整工具栏并断言操作在视口及页面无横溢，没有关闭或移除可达性断言。原始失败见[初轮日志](../artifacts/p103/development/browser-initial.txt)，原因核对见[焦点观察](../artifacts/p103/development/modal-focus-observation.json)，修正后预检与完整回归均14通过。辅助脚本的Windows模块URL/控制台编码问题仅影响诊断输出，已修正，不是产品修复证据。

视觉复核后，小高度弹窗改为标题/关闭区域固定、正文内部滚动；新增标题与关闭按钮保持视口可达的断言，真实Edge再次14通过，并补跑lint、前端构建及默认Windows编译，见[后续构建](../artifacts/p103/post-review-build.json)。这是47项主回归之后的定向验证，未把重复运行相加为新的独立UI用例。窄窗弹窗使用实际视口截图，长页面截图先回顶部，避免整页截屏重定位固定元素造成误导。

初轮可读TXT统一换行并去除两处行尾空格，以通过Git空白检查；原始UTF-8输出以无损JSON字符串及字节SHA另存[raw日志](../artifacts/p103/development/browser-initial-raw.json)，失败内容和计数保持，可核对两种表示。

## 视觉、范围与安全复核

真实浏览器截图：[首页卡片](../artifacts/p103/screenshots/home.png)、[设备](../artifacts/p103/screenshots/devices.png)、[连接页](../artifacts/p103/screenshots/connect.png)、[连接弹窗](../artifacts/p103/screenshots/connection-dialog.png)、[会话工具栏](../artifacts/p103/screenshots/session-toolbar.png)、[收起](../artifacts/p103/screenshots/session-toolbar-collapsed.png)、[450弹窗](../artifacts/p103/screenshots/dialog-450.png)、[450会话](../artifacts/p103/screenshots/session-450.png)。10张白名单图片的字节SHA见[visuals](../artifacts/p103/visuals.json)。浏览器不能调用本机核心，所以截图为真实未接入/不可读状态，不能当原生字段读取或双机串流证据。

离线`python scripts/p103-evidence.py`核对真实计数、截图哈希、变更白名单、原生/安全/锁/历史证据不变、文档链接及脱敏，结果见[verification](../artifacts/p103/verification.json)。静态复核结合实际Edge检查卡片/弹窗/工具栏三条主线；未发现阻塞/严重问题，无原生资源句柄/后台定时器/新远端请求。dialog卸载关闭，折叠仅局部状态，未产生会话事件。

## 限制、回退与交付

- 本轮未运行新的原生Windows UI、物理双机、云端或真实媒体测试；默认Windows编译通过不替代原生交互、所有DPI/Win11验收。P1-02原生观察限制保持历史事实，不追补PASS。
- 真实设备列表/身份/配对批准、网络进度/拒绝/错误与实际停止仍须后续任务接真实事件；当前不能发起产品远控。没有实际键鼠注入、桌面图或后台采集，未修改系统/云安全配置。
- R06剩余风险仍开放。历史报告/统计/旧失败不改；正常Git提交可回退本轮UI，不重写Git历史。

[计划](P103_PLAN.md)/[ADR0018](ADR/0018-p103-inert-connection-and-session-ui.md)与README/TASKS/开发指引已更新。只交P1-03，不进入P1-04至06或兼容运行；GitHub交付提交见Git记录及本轮回复，提交后停止等待审查。
