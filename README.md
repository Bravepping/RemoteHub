# RemoteHub · Phase 0 技术验证

这是已生成并在本机编译测试的 Windows 原型：**Tauri 客户端 + DXGI 主屏采集 + Media Foundation H.264 + Rust WebRTC → WebView2**。同机/双机视频及同一PeerConnection上的真实DataChannel模拟控制、跨网强制TURN已实测。自动选路已接入视频与控制并通过本机合成实播，新版云端视频及物理双机验收待测；正式输入、持久设备身份/服务端仍未完成，整个 Phase 0 尚未通过。

原始需求基线位于 `docs/RemoteHub_完整开发文档_v1.0.md`；首轮范围、测量结果、未完成项见 `docs/PHASE0_TEST_REPORT.md` 和 `TASKS.md`。

P0-03 新实测见 `docs/PHASE0_P003_TEST_REPORT.md`：AMD 硬件 MFT，10 秒 1280×720 / 300 帧 / 零丢帧，标准解码与八色条检查通过。原始裸流不能自带容器 PTS，请用 sidecar 中的 MF 时间戳；开发验证生成的 MP4 已检查为 30fps、10 秒。

## P0-04a 同机视频实验

本机原生 WebView2 已实测：五轮 15 秒各 450 帧、一轮 30 秒 900 帧全部接收/解码/呈现；实际选中 127.0.0.1、接收丢包 0，主动停止与播放中关窗回收通过。报告见 [P0-04a 测试报告](docs/PHASE0_P004_TEST_REPORT.md)。P0-04 两机任务仍为 doing。

普通构建默认关闭此入口。显式构建实验版：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p004.ps1
# 只构建、不打开窗口
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p004.ps1 -BuildOnly
```

窗口内每轮须重新勾选本次同意，并由原生确认框批准；5–30 秒，随时停止或关窗。视频经本机 WebRTC/SRTP 传送，未走磁盘视频或 IPC 像素。结束后仅在程序目录 `artifacts/private/p004/` 保存统计，不保存画面。直接运行本轮交付包的 `RemoteHub-P004a.exe` 可进入该实验。

AMD 驱动实际 SPS 标识为 `42041f`，其中不适用约束位按 H.264 解码规则处理；不修改原 SPS，SDP 为 Baseline `42001f`。驱动瑕疵与互通边界见 [设计说明](docs/P004_FORMAT.md)。

## P0-04b 指定局域网双机实验

显式构建 `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p004b.ps1 -BuildOnly`，生成的优化程序支持发送/接收两个角色。每轮两端原生确认、一次性码认证，原生TCP/UDP仅允许指定对端，连接中断或最长30秒后停止；统计保存到程序目录 `artifacts/private/p004b`。普通构建和 P004a 的网络范围不变。

操作与限定防火墙设置见 [双机操作说明](docs/P004B_RUNBOOK.md)，范围/兼容性见 ADR0004。首轮RTCP候选校验错误已修复；v2真实双机30秒900帧全部接收/解码/呈现，1280×720、丢包0，主动停止/关窗/拔线回收通过。详见 [双机测试报告](docs/PHASE0_P004B_TEST_REPORT.md)。本轮不保存画面，基线要求的录屏未留存，P004父项仍保留证据差异。

## P0-05 模拟控制 DataChannel

显式构建 `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p005.ps1 -BuildOnly`，生成P005实验版。WebView2控制端在现有视频PeerConnection创建可靠有序control.v1，Rust被控端真实接收归一化模拟坐标并经通道返回ACK；不调用Windows输入API。

两台真实Windows10有线PC，两轮30秒视频+100模拟事件，发送/Rust接收/ACK写入/B确认全为100、序号连续、异常0，720p视频同时正常播放。两端主动停止、接收端关窗/拔线清理通过。43 Rust/11前端/2Edge及受限SDP/权限/大小/限流/重放负例通过；本机SCTP自动测试与双机实测分别记录。

操作见 [P005双机步骤](docs/P005_RUNBOOK.md)，协议见 [control.v1](docs/CONTROL_V1.md)，实测与限制见 [P005测试报告](docs/PHASE0_P005_TEST_REPORT.md)。新feature默认关闭，旧P004a/04b保持视频-only范围。统计位于程序目录artifacts/private/p005；不记录坐标、完整SDP、配对码或画面。P005已获阶段性审查通过，后续TURN与跨网进展见下文。

## P0-06a TURN/ICE 准备

状态为 **infrastructure-verified**：准备实现之后，用户单独授权的G6真实云STUN/Allocate、缺失/错误/过期票据、错误realm及危险peer拒绝已通过；[云端报告](docs/PHASE0_G6_TEST_REPORT.md)记录两次真实原生gather与11项协议检查。G6收尾时独立TURN测试服务已停止且不开机自启；后续P006b按新授权临时启动，保留受限配置，原业务服务及防火墙策略保持。双机relay视频仍待独立验收，P006父项未完成。

原生webrtc/rtc仍锁定0.21.1，其当前TURN实现只支持UDP。本轮严格拒绝TCP/turns/DNS和DirectPreferred执行，不以配置字符串冒充实际relay。旧Loopback/LAN PinnedRuntime、视频、control.v1及原生授权入口不变。TURN共享密钥只属于独立服务端模块，不链接进桌面应用；REST票据在有效期内可复用且不能单票即时撤销，正式持久身份/ACL尚未实现；P006b新增的临时WSS不等同正式身份服务。

完整说明见 [ADR0006](docs/ADR/0006-p006a-turn-preparation.md)、[ICE与信令契约](docs/P006_ICE_CONTRACT.md)、[coturn操作说明](deploy/coturn/README.md)、[测试与验收矩阵](docs/PHASE0_P006A_TEST_REPORT.md)。54 Rust/13前端/3Edge/3Python通过；Edge仅检查真实relay属性，没有连接云端。

在已有工具链和依赖缓存的工程目录可复现离线检查：

```powershell
Set-ExecutionPolicy -Scope Process Bypass
. ./scripts/use-toolchain.ps1
$env:REMOTEHUB_PYTHON=(Get-Command python).Source
# 使用本机 Git Bash 的真实路径；不要误用 WSL 或 Windows Store Python 别名
$env:REMOTEHUB_BASH=(Get-Command bash).Source
node scripts/p006a-checks.mjs
```

CLI构建需显式`--features p006-turn-probe`，检查脚本只运行帮助、默认拒绝和无法解析的占位模板，不连接公网。普通启动无采集或网络探测；真实目标的`--approved-target PRIVATE_JSON`只能在获该端点测试授权后使用。没有给默认客户端增加新公网页面或IPC。以上为G6历史交付范围，当前P006b进展单列下文。

## P0-06b 跨网络强制 TURN 实验

宽带发送端与手机蜂窝热点接收端已真实验证：两轮30秒720p30视频各900帧全发送/接收/解码/呈现，丢包和冻结均0；5轮都为双方真实selected relay/relay UDP，同一视频PC上的100条control.v1发送/Rust接收/ACK/确认均100，序号连续、异常0，不执行实际输入。

发送端停止、接收端关窗及热点断网分别在194/196/244帧提前回收。接收端主动停止由用户现场验证并明确跳过补测；对应900帧记录不能独立证明提前停止，原报告保留该限制，不追填日志。P006b按用户接受的现场确认收尾；原未归属TURN套接字保持历史记录，后续独立生命周期测试见下文。P006父项继续doing，按本轮要求进入P006c，尚不进入Phase1。

60项Rust、16项前端、3项Edge及13组完整检查通过；Ubuntu隔离信令服务4测试和构建通过。结束后WSS/TURN已停止、保持disabled、实验socket清零。原基线26项服务仍运行，两个业务容器保持；新增观察到fwupd运行，未操作。源码不含实际节点地址、角色邀请、TURN共享secret或TLS私钥。

操作见 [双机步骤](docs/P006B_RUNBOOK.md)，范围见 [ADR0007](docs/ADR/0007-p006b-cross-network-relay.md) 和 [协议](docs/P006B_CONTRACT.md)，真实统计位于artifacts/p006b/two-pc。复现自动检查：`powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p006b-checks.ps1`；显式构建：`powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p006b.ps1 -BuildOnly`。实验重新使用需要新的明确授权和私有限时配置，每轮双方亲自确认；验收归档不附带有效邀请。

## TURN Allocation 生命周期验证

按用户授权单独验证了6例：协议Refresh(0)、停止续租、Windows原生/Edge gather后close，以及已连接的Rust↔Edge data-only双方close。5例均真实观察到0请求/回应及约1.1–1.5秒内套接字消失；不续租例按实际授予600秒，到期后约0.318秒首次观察到回收。每例均留存基线、期间、关闭后、原租约到期及宽限后的资源状态，同一coturn PID贯穿测试，停服务不参与单例判定。

完整[独立报告](docs/TURN_ALLOCATION_LIFECYCLE_TEST_REPORT.md)、[复现说明](docs/TURN_LIFECYCLE_RUNBOOK.md)及artifacts/turn-lifecycle区分真实云/Windows与离线fixture。19项检查达到预期，61 Rust/16前端/3 Edge/11 Python；本轮服务已停止、不开机自启，配置/业务容器启动时间/防火墙策略未变。没有屏幕采集或重复双机串流；Edge不是嵌入WebView2，不能将本轮data-only扩大为所有媒体负载保证。

接收端主动停止按用户现场确认跳过补测，原独立日志限制保留，不再阻塞推进。本轮生命周期报告与代码独立提交后才进入P006c；P2P优先与自动回退状态另列，不由这些测试推断。

## P0-06c 直连优先与自动回退首步

生命周期交付88f1edc之后，新增默认关闭的`p006-direct-probe`，通过独立DirectGrant/套接字边界使用ICE All；旧本机、LAN、强制relay入口不变。一个物理Windows机器上的两个真实原生PeerConnection已自动完成真实云端四例：TURN可用选host/host；进程内直连受阻自动选relay/relay；TURN受阻仍host/host；两路均断明确失败。前三例均100条发送/接收/ACK一致，第四例0消息、无pair，不能当成连接成功。

首步云端4个实际Allocation均有Refresh0成功回应及约1.14–1.40秒首次观察socket消失，停止前已经0；该轮服务已停止、不自启，WSS始终关闭。首步24项自动检查通过，66 Rust/16前端/3 Edge/11 Python，回退轮耗时34.873秒。该历史报告仅验证无视频原生路径；当前视频集成结果另列，不能互相替代。

见[首步报告](docs/PHASE0_P006C_TEST_REPORT.md)、[契约](docs/P006C_CONTRACT.md)、[复现步骤](docs/P006C_RUNBOOK.md)。`powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p006c-checks.ps1`只做离线回归；`node scripts/p006c-local.mjs --local PRIVATE_INTERFACE_IPV4`显式运行无云/无采集的本机真实传输。真实云测试需新范围授权及短票，不能复用已清理配置。

## P0-06c 视频与模拟控制集成

已依次接入Rust Session、现有WebView2前端代码和认证WSS，复用同一个视频/control.v1 PeerConnection。DirectPreferred显式使用ICE All并核对双方真实selected pair；旧本机/LAN/RelayOnly限制保留。新path策略经HMAC认证，只发布对方明确允许的私有host；停止、过期、模式错配和重放均拒绝。

本机7轮真实合成视频测试通过；认证WSS完整轮300帧输入/编码/发送/接收/解码/呈现，720p30，100事件发送/原生接收/ACK/确认一致，异常0、host/host、资源回收完成。35组完整检查通过（74 Rust/22前端/3 Edge/11 Python）；Tauri新版已构建。本轮未采集桌面或启动云服务，单机Edge合成图案测试不能充当新版嵌入WebView2、物理双机或云视频回退验收，P006c保持doing。

见[独立测试报告](docs/PHASE0_P006C_VIDEO_TEST_REPORT.md)、[协议契约](docs/P006C_VIDEO_CONTRACT.md)、[复现与构建](docs/P006C_VIDEO_RUNBOOK.md)、[待授权云端范围](docs/P006C_VIDEO_CLOUD_PLAN.md)。构建入口`powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p006c-video.ps1 -BuildOnly`；离线回归`powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p006c-video-checks.ps1`。正式窗口每轮仍须亲自确认；合成源及故障注入只在独立操作员probe启用，无实际键鼠注入。

## P0-03 本机编码 probe

以下命令在源码根目录执行，编译后也可直接运行 `target/release/encode-probe.exe`。默认不采集，只有显式 `--consent` 才采集；输出限定在 `artifacts/private/NAME` 的新目录。

```powershell
Set-ExecutionPolicy -Scope Process Bypass
. ./scripts/use-toolchain.ps1
cargo build -p hub-encode-win --bin encode-probe --release --locked
./target/release/encode-probe.exe --consent --seconds 10 --encoder auto --name my-local-test
```

`--encoder software` 可独立验证软件回退；`--stop-after-ms 2000` 用于主动停止测试，控制台 Ctrl+C 也请求正常停止并 drain。最长 30 秒，不覆盖已有样本。运动源复现、独立 FFmpeg 解码和统计口径见 P0-03 报告；FFmpeg 只用于开发验收，不是客户端运行依赖。原有 Tauri 采集窗口没有接入新编码器，本轮无需操作该窗口。

## 使用已编译程序

解压首次交付 ZIP，双击 `RemoteHub.exe`。需要 Windows x64 和已安装的 WebView2 Runtime。窗口启动时不采集；明确勾选本次同意后，点击“开始采集实验”，运行最多 5 秒并显示真实统计。窗口不保存截图或发送画面。关闭窗口即退出。

`capture-probe.exe` 是开发用的独立本机实验工具，不是产品运行依赖。只有显式执行下列命令才会保存当前主屏的 BMP 和 JSON：

```powershell
.\capture-probe.exe --consent --frames 30 --out .\private-capture-001
```

输出目录必须不存在，避免覆盖样本。样本包含桌面内容，自行决定保留。本轮软件为未签名测试构建，尚无安装器，也未做干净系统安装验证。

## 从源码运行

要求 Rust 1.100.0 MSVC、Microsoft C++ Build Tools / Windows SDK、Node 24、pnpm 11.25.0、WebView2。准备方式参照 [Tauri 官方 Windows 前置条件](https://v2.tauri.app/start/prerequisites/)。本机临时开发工具位于 `.tools`，没有修改全局 PATH；该目录不包含在源码 ZIP 中。换机请准备官方工具链，或在已配置的 Developer PowerShell 中执行。

在工程根目录运行：

```powershell
pnpm install --frozen-lockfile
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/doctor.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/dev.ps1 -Mode Desktop
```

其他入口：

```powershell
# 完整格式/类型/前端/Rust 测试，保存原始输出与各命令退出码
pnpm verify
# 浏览器验证需要本机 Microsoft Edge，仅作为预览测试
pnpm test:e2e
# 构建内嵌前端资源的独立 Windows 程序，不生成安装器
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/dev.ps1 -Mode Build
# 明确授权有限本机采集并导出样本
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/dev.ps1 -Mode Capture
```

`Browser` 模式只用于页面预览，无法调用 DXGI。不能把浏览器预览当成原生窗口已通过的证据。默认没有远程监听、配对旁路、键鼠注入、后台采集、自启或云端部署。

## 工程内容

- `apps/desktop`：Vue 3 / TypeScript / Tailwind / Tauri v2，受限本机 IPC。
- `crates/hub-capture-win`：D3D11 / DXGI Desktop Duplication、限时实验、BGRA 读回及 BMP 导出。
- `crates/hub-encode-win`：MF 编码、CLI 文件实验与无文件实时 AU 生产者。
- `crates/hub-webrtc`：隔离的本机/指定LAN会话、SDP/RTP、3 AU队列、DataChannel及清理；P006a另有默认关闭的TURN准备模块。
- `crates/hub-turn-credentials`：仅服务端使用的短期票据签发纯模块，无HTTP接口，桌面客户端不依赖。
- `deploy/coturn`：不可直接运行的占位模板、离线校验/渲染及授权后手工操作说明。
- `scripts`：环境检测、运行入口、可保存结果的验证脚本。
- `artifacts`：非画面测试证据；`artifacts/private` 中的桌面样本不进 Git 或交付 ZIP。
- `docs/ADR`：本机实验边界、未开放远控能力的原因及后续接入条件。

首轮真实测试：Windows 10 22H2，AMD Radeon RX 6750 GRE 10GB；30 帧、2560×1440、1059ms、观察帧率 28.315 FPS。该数字是 DXGI 收到桌面更新的计数，**不是视频编码、传输或解码帧率**。
