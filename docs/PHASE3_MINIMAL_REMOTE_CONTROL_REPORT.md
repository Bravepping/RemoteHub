# Phase3 最小局域网远控闭环汇总

状态：**最小真实远控目标已实测通过；完整Phase3退出不作声明。** 实施基线：382aac026bafae3b5ad5c74b6ee4ebf7508c235f；本轮核对 origin/main 未有后续更新。当前DPI矩阵、独立短轮覆盖和收尾通道错误保留限制，不提前记全场景PASS。

## 决议与实现范围

用户已接受 P2-01，并明确暂缓 P2-02–06，直接串联原工程产品 UI、DXGI 主屏采集、MF H.264、WebRTC 和真实 DataChannel 基本键鼠。用户指定同 LAN 两台自有 Windows PC，本机控制、另一台被控，具体端点只保存在两台本地；每轮由用户亲自批准。按用户本轮决议不另建逐子任务 Plan/ADR/Contract/Evidence，保留此单份汇总及必要自动测试记录。

- 显式 `p3-lan` feature，默认普通版不开远控；产品身份沿用 P2-01。复用既有 `p004_lan_*` 命令，无新 Tauri 命令或 capability。旧 loopback/LAN/sim/relay 实验不启用实际输入。
- 临时 TCP 信令固定端口48604、指定私有 IPv4、一次性高熵码、HMAC/递增认证帧、120秒配对等待、30秒会话。信令未提供 TLS，仅限已批准 LAN 范围；正式身份签名/数据库/持久信任未实现。媒体与输入走同一 PeerConnection 的 SRTP / DTLS-SCTP，未用 IPC/TCP 冒充数据通道。
- 被控端原生授权明确说明本次真实主屏与基本输入；控制端须在解码画面及对端校验后主动启用。Esc、失焦、移出画面暂停并释放；被控端停止、关窗、Ctrl+Alt+F12 可结束，不能托盘隐藏继续采集。
- 严格候选要求：原生真实选中 pair 两侧 host/UDP、IP处于本次 scope，地址/端口/类型与已认证 SDP 精确匹配。仅产品发布路径支持把 UUID mDNS host 候选映射为已原生批准、确认由本机持有的选定 IPv4，保留原端口；不解析任意 DNS、不放宽 UDP 白名单、不接受未声明 prflx。多网卡映射不对应实际端口时仍须拒绝。
- 主屏归一化坐标、视频黑边过滤、物理像素/虚拟屏原点换算；普通进程 SendInput，UIPI/管理员/安全桌面不绕过。无剪贴板/文件传输/Unicode文本批量注入/Win或Alt系统组合键/多显示器切换/公网/自动重连。

## remote-input.v1

可靠有序通道，标签及 protocol 都为 `remote-input.v1`，与旧 `control.v1` 模拟通道分离。消息外层为 `v=1`、递增 `seq`、本次 `session_id` 和 `event`；事件只允许 `mouse_move`、`mouse_button`、`key`、`wheel`、空 `release_all`。归一化坐标0..1、按钮0..2、固定基本DOM键白名单、滚轮±120。

原生先检查当前授权/期限，再验证文本/大小（最多512B）、严格字段、序号、会话、速率（120条/秒）、总数（1000），通过实际应用后才返回同通道 ACK。停止标记在输入互斥锁内重新读取，撤销释放仅本次注入的按键/按钮；释放失败不记清理成功。不会记录输入内容、配对密钥、完整 SDP 或桌面画面。

## 真实双机尝试

| 程序 | 用户操作结果 | 独立证据及限制 |
| --- | --- | --- |
| v1 | 失败，控制端CONTROL_CHANNEL_ERROR、被控端INPUT_SELECTED_PAIR_NOT_APPROVED；用户查看offer候选类型为空 | 本机最终getStats：ICE/PC connected、local prflx / remote host、0呈现帧/0输入/0ACK、资源回收。被控端错误/空类型依据USER_RESPONSE；没有独立复制被控端全量文件。见first-attempt-*。 |
| v2 | 失败，被控端SPS_PROFILE_INCOMPATIBLE_42c01f | 本机确认原始2条mDNS候选、发布2条、原生认证、独立最终统计/清理；0呈现帧/0输入。用户提供被控端脱敏诊断：local和remote均host/UDP EXACT_MATCH，计数1/2，故已越过v1候选拒绝。不能把浏览器local prflx字样直接等同原生实际选中候选不匹配。见second-attempt-*。 |
| v3 | 用户确认看到画面，本机输入英文后另一台同步出现；后续四项人工操作回复“正常” | 本机CIM只读核对PID19320与交付SHA一致。先前两次Get-Process未发现，用户确认窗口可见，不能将其解释为启动失败。真实接收最终证据及覆盖限制见下表，不继承旧Phase0成功轮。 |

| v3独立接收端最终记录 | 实际解码/呈现 | 实际发送/ACK | 未确认 | 通道异常 | 原生/浏览器回收 |
| --- | --- | --- | --- | --- | --- |
| 第一轮 | 853 / 853，1280×720 | 29 / 29，序号1..29 | 0 | 0 | true / true |
| 后续记录一 | 900 / 900，1280×720 | 81 / 81，序号1..81 | 0 | 1，CONTROL_CHANNEL_ERROR | true / true |
| 后续记录二 | 900 / 900，1280×720 | 132 / 132，序号1..132 | 0 | 1，CONTROL_CHANNEL_ERROR | true / true |

三轮均原生认证、ICE/PC connected、接收对端校验通过，独立getStats在PC关闭前保存；发送计数包含按键、移动、按钮、释放消息，不等于同数量的键盘字符。第一轮原生周期末统计836帧，独立浏览器final为853，不补写为900；后两轮900只说明接收解码/呈现数量，没有独立被控端源文件证明发送端完整计数。

用户按清单确认点击位置/Shift+Esc暂停及恢复小写、控制端停止、被控端应急键、被控端关窗和拔线回收正常。**本机实际只取得上述3轮独立final，后两轮已达到完整900帧，无法一一映射为要求的4个提前结束短轮。** 人工确认作为USER_RESPONSE归档，不写成4个独立短轮PASS；被控端完整原始统计本轮未独立收取，第一轮实际英文输入由现场确认，Rust处理后ACK由真实DataChannel统计和代码路径对应。

后两轮错误事件发生于约32秒的会话末尾，均有open=1、close=1、error=1，最后周期统计无错误，最终统计保留错误，且所有已发消息获ACK、资源已回收。现有证据将问题范围缩小到会话结束/通道关闭路径，但**尚未定向复现证明具体竞态，也不直接定性为误报**。此问题登记为P3-END-01 OPEN，不清零计数、不伪装修复、不要求用户重复整套测试。

v2进一步排除了“本轮仍只因候选发布”这一假设。首轮缺失候选问题的修复与后续SPS兼容问题分别记录，不补写第一次失败。历史R06网络/机器/证据不同，保持OPEN，不能凭本轮进展宣称历史根因已修复。

## 已复现与修复

1. 输入协议空release事件最初使用serde单元变体，额外字段负例实际失败；改为空结构体变体后严格拒绝。失败记录见 `artifacts/p301/initial/release-extra-field.txt`。
2. 旧 `lanOffer` 只保留字面IPv4，UUID mDNS候选被过滤为空。离线回归复现，v2新发布及被控端EXACT_MATCH由上述两类证据分别确认。脱敏诊断只含原因/类型/数量，区分空候选、范围、地址、端口和类型/协议变化。
3. 旧packetizer只接受42/00/1f或已记录AMD42/04/1f，误拒绝42/c0/1f。新增回归在修复前真实失败，见 `initial/constrained-baseline-before.txt`；修复后目标测试通过。按[RFC6184 §8.1表5](https://www.rfc-editor.org/rfc/rfc6184.html#section-8.1)，接受profile_idc=42、同一L3.1的Baseline/Constrained Baseline约束组合，保留既有AMD constraint_set5披露；仍拒绝其他profile/level/未支持低位。SDP仍42001f，原SPS不改写。单元fixture验证原始SPS在RTP中保持，不冒充第二台硬件解码已测。

## 自动验证与交付

v2完整回归：基础47项、产品5项、权限7项通过；105前端、21 Edge、Rust全feature104/默认77（各另1个需私密MF样本的ignored）、p3核心30/桌面7；另12隐私/9交付和4摘要隐私测试通过。数量存在跨命令重叠，不相加。真实SCTP测试的输入通道仅给空injector发送release_all，无SendInput动作，不能作为真实键鼠验收。Edge产品UI使用明确标识SDK替身，不作为原生远控。

v3重新执行基础47项、产品5项/权限7项通过；105前端、21 Edge、Rust全feature105/默认78（各另1个私密MF样本ignored）、p3核心31/桌面7；另12隐私/9交付及5摘要隐私测试通过。摘要额外覆盖CLI非法参数不回显、非有限数值和固定SPS错误类别。第一次格式门禁暂停已修正并归档 `initial/v3-format.txt`，最终fmt/clippy/构建均通过。`v3-application-scope.json`核对v2→v3应用源码仅video.rs兼容检查/测试与p004_lan.rs构建标记变化，候选/授权/输入代码未变。

v3程序：`artifacts/private/p3-delivery-v3/RemoteHub-P3-LAN.exe`，SHA256 `82249657a33cb1856a09795ad7bb082f372c6160f5613492df08750f4a37d968`。

v3 ZIP：`artifacts/private/RemoteHub-Phase3-LAN-v3.zip`，SHA256 `bbc4522b5cb5804c63cc3ac46af6e84e2984d67260096945a1a3f91a0f2f0c61`。

已归档v1/v2/v3程序哈希及代码清单于 `artifacts/p301/package-v1.json`、`package-v2.json`、`package-v3.json`；当前包清单为 `package.json`。ZIP仅程序/中文步骤/SHA三文件，无端点配置或票据，本地位于ignored的artifacts/private；不是公开GitHub下载地址。首轮零异常记录与后续含错误记录均保留，不用后续文件覆盖初次结果。

复现入口：`powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p3.ps1`（完整离线门禁及显式产品构建）；`python scripts/p3-summary-test.py`；`python scripts/p3-package.py`（仅打包、不启动）；`python scripts/p3-evidence.py`（核对冻结源hash、真实统计、失败保留及已知限制；ZIP存在时验证实际ZIP及内含EXE，缺失时不冒充已下载验证）。工具链沿用scripts/use-toolchain.ps1。不同构建EXE不能混用其SHA。私密MF样本用例不默认运行，不读取用户桌面作为测试fixture。

## 未完成与风险

- 真实视频+英文输入最小目标已通过；点位/暂停/四项停止操作获用户确认。独立日志仅3轮，不能映射完整提前停止矩阵；P3-END-01两轮收尾通道错误OPEN，未冒充全场景零异常或Phase3完整退出。
- 主屏DPI算法仅纯函数验证；不同物理DPI/缩放/副屏/旋转/HDR/Win11/长稳/安全桌面未完成，不标全环境兼容。
- 临时LAN配对非P2正式信任系统，P2-02–06仍暂缓；同用户恶意进程/管理员威胁、正式安装器及后台服务不在此轮保证内。
- R06 OPEN、C0待决；无阿里云/第三方运行/防火墙、路由或驱动变更，不进入Phase4。
