# C0-DRAFT：三件套兼容目标合同草案

日期2026-11-22；状态 **DRAFT / NOT_APPROVED / NO_RUNTIME_PROTOTYPE**。本轮仅准备合同、公开材料只读核对及待决清单，不批准安装、启动、抓桌面、实际输入、驱动/服务/路由变更或发布第三方二进制。用户审查此草案后另行确认目标、例外和原型范围。R06继续OPEN；P1-05完成不自动进入P1-06/C1或兼容原型。

## 目标和验收含义

目标不是只借鉴界面：拟支持选定版本的 **Foundation Sunshine原生主机 ↔ Moonlight V+ for PC原生客户端** 的配对/会话互操作，由RemoteHub统一管理；EasyTier是可选IP承载。当前WebRTC/DXGI/MF主链路继续保留且明确标识为WebRTC后端，不能因同为H.264、同网可达或第三方进程启动而标记兼容成功。

最终交付目标为一个RemoteHub安装器和一个用户入口，不要求用户手动安装、启动或日常维护三个独立程序；能否合法打包及是否需要可选下载，必须先通过下文版本/许可与用户决议，不能因此在本轮自动分发组件。

| 编号 | 拟验收对象 | 最低可核对结果 | 当前状态 |
| --- | --- | --- | --- |
| C0-H | 作为被控端的Foundation Sunshine目标版本 | 目标原生V+客户端可发现/指定主机、完成该后端配对与批准、播放并停止；撤销后拒绝旧配对 | NOT_IMPLEMENTED；需后续授权运行验证 |
| C0-C | 作为控制端的Moonlight V+ PC目标版本 | 连接选定Foundation主机，真实解码/呈现/停止及断网回收，错误不被进程running掩盖 | NOT_IMPLEMENTED |
| C0-U | 双端统一RemoteHub界面 | 一个产品入口负责设备/配对/批准/会话/停止/错误；常用操作无需用户另开三件套配置界面 | 目前仅WebRTC产品壳；第三方管理未实现 |
| C0-N | 可选EasyTier承载 | 隔离网络内双方实际可达且原生媒体通；实际直连/中继由该后端证据确定，退出撤销本轮资源 | NOT_IMPLEMENTED；不替代协议兼容或身份授权 |

初个原型建议Windows x64、单活动会话、单显示器、H.264 SDR；不承诺Foundation增强剪贴板/麦克风/文件映射/HDR/AV1/虚拟显示器/手柄/USB功能。实际输入及真实桌面若成为后续测试必需，必须另获明确批准；本轮及仅合成源授权不能覆盖它们。后端原生配对也不能直接继承RemoteHub实验HMAC/短票或Pinia状态作为批准。

## 版本来源与身份冻结

本轮只读检索确认下列公开项目身份候选：Foundation主机说明来自[AlkaidLab/foundation-sunshine](https://github.com/AlkaidLab/foundation-sunshine)，PC客户端说明来自[qiin2333/moonlight-qt](https://github.com/qiin2333/moonlight-qt)，覆盖网候选来自[EasyTier/EasyTier](https://github.com/EasyTier/EasyTier)。这是来源线索，不是当前下载包与源码一一对应证明。名称相近的[qiin2333/moonlight-vplus](https://github.com/qiin2333/moonlight-vplus)描述Android客户端，不能当PC目标；其他再fork不能静默替换这三个候选。

旧本地清单只读引用[compatibility-local-inventory](../artifacts/p007-final/compatibility-local-inventory.json)，本轮不执行安装器、不重算其现存副本或修改旧清单：

| 候选文件 | 文件自述版本（分段） | 尚缺绑定 |
| --- | --- | --- |
| Sunshine.v2026.1031.WindowsInstaller.exe | 2026 / 1031 / 121458 / 杂鱼，description Foundation Sunshine | 签名链、官方Release资产、源码commit/递归submodule与构建来源 |
| Moonlight-VPlus-Setup-6.5.6.exe | 6 / 5 / 6 / 0，Moonlight V+ for PC | 同上，且需确认是PC fork对应资产 |
| easytier-gui_2.6.4_x64-setup.exe | 2 / 6 / 4，GUI包 | GUI内core/CLI/RPC与驱动的分别版本/许可/来源 |
| Sunshine-Windows-AMD64-installer.msi | 未辨认 | 不作为Foundation默认替代品 |

旧SHA只定位当时文件，不能证明来源可信。本轮[公开来源观察](../artifacts/p105/c0/public-sources.json)记录查询时间、当时分支commit及该commit的LICENSE文本SHA；**该观察commit不是已批准目标版本，也未绑定旧安装器**。若公开查询不可得，明确记录UNAVAILABLE，不补造字段。

原型前C1必须冻结每项：仓库/Release/tag、commit和submodule revision、Windows架构、精确资产名/大小/SHA256、签名/签发者/验证结果、许可证/NOTICE及依赖清单、构建/打包脚本和变更补丁。禁止latest、仅文件名匹配、版本自述或“上游兼容”宣称替代此清单。升级必须新矩阵回归，不自动换版本。

## 统一UI与托管方案

推荐先评估“可选受管理进程后端”，仅为草案取向。Tauri/Rust仍是产品所有者，Vue只经明确类型化IPC请求动作；不增加任意shell、命令字符串或网络代理权限。拟议管理边界如下，尚未实现也未注册这些IPC：

- 后端清单只允许批准路径/哈希/版本与固定参数模板，不接受用户拼接命令。进程/子进程所有权逐次登记；不接管或关闭用户已运行的独立Sunshine/V+/EasyTier实例，不按进程名批量终止。
- 默认普通权限、手动启用，无自启/服务安装。EasyTier虚拟网卡/驱动、管理员权限及系统路由属于独立例外，未获批准即不可用，不静默提权、安装或改防火墙。
- 每产品/后端/会话独立配置目录、证书/配对及网络命名空间；不覆盖用户现有配置。秘密不经命令行、前端state、日志或包内明文分发；具体受支持的安全配置入口在C1核验，不臆造API。
- 启动后须用真实健康/握手/媒体证据确认可用；停止先撤销输入和授权，再按已核验接口优雅结束自己拥有的会话，必要时限时结束自己的受管理进程并记录。主进程崩溃/关窗/升级/子进程退出必须无孤儿资源；不依“杀整个第三方服务”冒充单会话正常释放。
- 禁用未经批准的后台自动更新/下载/遥测/UPnP和网络暴露；能否配置到此范围需C1确认。配置不支持时列阻断，不通过系统安全降级绕过。

一个可见UI与一个操作系统进程不同：Tauri/WebView2本身有子进程；新增第三方进程属于需用户确认的后端托管例外。优先要求统一设备/批准/错误控制及会话呈现。若V+只能另开原生播放窗口，应记录与目标的差距，先让用户决定是否接受“统一控制台+受管理播放窗”；不能把启动外部窗口当严格单UI已满足。若必须真正单窗口，再评估经许可核验的核心嵌入/FFI/源码派生，单独ADR、崩溃隔离和生命周期成本；不假设Qt窗口可安全嵌入WebView，不做未经授权的窗口注入或钩子。

## 许可证核验门槛

公开仓库当前LICENSE文本分别为[Foundation GPL v3](https://raw.githubusercontent.com/AlkaidLab/foundation-sunshine/master/LICENSE)、[PC fork GPL v3](https://raw.githubusercontent.com/qiin2333/moonlight-qt/master/LICENSE)、[EasyTier LGPL v3](https://raw.githubusercontent.com/EasyTier/EasyTier/main/LICENSE)。本轮只确认文本头及内容哈希，不给旧安装包、所有依赖或最终组合分发作许可通过结论；具体only/or-later声明、额外条款/NOTICE和文件头须以冻结revision核验。

| 门槛 | 原型/分发前必须取得的材料 |
| --- | --- |
| 来源与完整性 | 目标revision、递归依赖、构建来源及资产SHA/签名能互相对应；不能拿标准Sunshine/月光上游许可替代fork材料 |
| 修改/链接/聚合 | 区分不修改外部进程、代码派生、静态/动态链接、同包分发；不要仅因“分进程”就判定没有组合义务；按实际接口/打包方案复核 |
| 对应源码与告知 | 适用许可证全文、版权/NOTICE、补丁与对应源码/构建脚本、分发方式的来源提供方案；嵌入/替换组件权利及其他依赖限制逐项记录 |
| 依赖与驱动 | Qt/FFmpeg/解码库、common-c、EasyTier GUI/core、虚拟显示/网卡驱动分别列清单和许可，不泛称全套一个许可证 |
| 退出条件 | 来源不明或许可无法满足则停止该候选，不下载第三方再包装“先测后补”；本轮无分发第三方二进制 |

本表是工程交付门槛，最终许可适配必须基于已选版本及分发模式审查；草案不改变RemoteHub许可证，也不授予重许可或复制fork代码的权限。

## 后端与统计诚实性

UI必须显示实际后端与数据来源。WebRTC路径仍由真实ICE selected pair确定，严格候选/SDP/指纹/授权不放宽，R06拒绝必须保留失败。Foundation/V+原生会话使用其真实协商/渲染证据；EasyTier overlay路径独立展示，不能冒称ICE selected pair。RTT、解码/呈现/丢包/ACK按各后端定义与采样时间记录，不跨来源拼接同一成功结论。未知能力显示不可用，PID存活不等于媒体正常或授权成功。

## 后续验证门槛与待用户决策

| 决策 | 建议草案 | 当前状态 |
| --- | --- | --- |
| D1 目标方向 | 双向角色均纳入最终目标；先Foundation主机↔V+ PC配套一对，再按证据决定与其他Moonlight/Sunshine版本互通范围 | PENDING_USER_DECISION |
| D2 版本与来源 | 确认上述三个公开仓库候选、是否沿用旧本地候选或重新冻结Release；不自动选当前HEAD/latest | PENDING_USER_DECISION/C1 |
| D3 统一UI口径 | 默认严格统一控制台/呈现；独立原生播放窗若必要作为明确可接受的过渡例外 | PENDING_USER_DECISION |
| D4 托管/分发 | 是否允许受管理子进程、可选打包和配置隔离；嵌入核心另议，不默认下载/安装 | PENDING_USER_DECISION/许可证核验 |
| D5 EasyTier | 作为可选网络承载，分别审批驱动/管理员/路由/持久化与服务；默认关闭 | PENDING_USER_DECISION |
| D6 原型测试权限 | 先离线接口/源码/进程生命周期审查，再单独申请限定合成媒体/网络运行；真实桌面/输入永不隐含批准 | NOT_AUTHORIZED_THIS_ROUND |

C1材料完整后再提交可审查的原型计划：本机无媒体生命周期→明确授权的原生配对/合成媒体→授权的双机/网络→撤销/拒绝/超时/断网/崩溃/版本不匹配与资源回收；每步必须真实统计和失败证据。该顺序是未来门槛，不是本轮执行清单。草案完成只表示C0-DRAFT交付，**C0正式签署、C1核验、第三方互操作与Phase1整体均未完成**。
