# RemoteHub V2 — Clean Core

自用 Windows 远程控制软件：**Tauri v2 + Vue 3 / TypeScript + Rust**。本分支专注可用远控，不重做已成功的视频与输入链路。

**开发分支**：`v2/clean-core` · **提取基线**：`v2/fast-track@4385454`（含 `main@d7dbc9b` 的真实 LAN 视频与英文输入）。原 `main`、`v2/fast-track` 保持独立。

## 实际状态

| 模块 | 现状 |
| --- | --- |
| 局域网双机 720p H.264 视频 | 来源分支已真实验证：三轮 853 / 900 / 900 帧解码呈现 |
| DataChannel + SendInput | 来源分支实测英文输入；29 / 81 / 132 条消息分别全部收到 ACK |
| 原生授权、停止和临时配对码 | 已有产品 `p3-lan` 代码与历史人工操作确认 |
| 会话结束错误 | **P3-END-01 已通过 Windows 双机局域网回归**（精简分支构建 `77479f2`）：先撤销输入、通知原生停止，再关闭浏览器 DataChannel；正常结束提前发已认证的收尾信号，活动期通道故障仍报错。历史后两轮曾报 `CONTROL_CHANNEL_ERROR` |
| 设备身份 | 可选桌面构建支持核对对端完整指纹并原生确认；单个记录保存在本机内存。可显式用于已验证设备 LAN 视频，不授予键鼠权限 |
| 私网 WSS 信令（仅视频） | 桌面可选模式，双方设备证明及停止仍走直接 TCP；签名 offer/answer 由私网 WSS 中继转发，视频仍走指定对端的 LAN WebRTC。两台 Windows 实机已确认画面、共享端手动停止后两端结束，以及关闭中继后无法建立新连接 |
| 已验证设备 LAN（仅视频） | 可选模式，原生明确同意后，用临时配对码及双方设备签名核验；身份或信令不符直接结束，不回退。Windows 双机画面、共享端手动停止、自然结束及未核验拒绝通过（`c8122d2`） |
| Axum WebSocket 信令 | Rust 原生客户端与 Axum 已在回环打通。两个真实 WebRTC PeerConnection 通过带会话鉴权的 offer/answer/ICE 建立 DataChannel 并传输合成文本；桌面可选构建增加原生授权后的本机信令诊断；实际远控仍使用既有 LAN TCP |
| STUN/ICE + TURN | Windows 公网双机已验证合成视频 STUN 映射直连、手动强制 TURN 与建连时受控自动回退；正式产品路径未集成；**R06 仍 OPEN** |
| Windows 安装包 | 未交付 |

**注意**：表中帧数和输入 ACK 数来自原版历史测试；本次精简分支双机回归未单独记录这些计数。远控只允许在已授权的 Windows 设备上进行。

## 核心目录

| 路径 | 用途 |
| --- | --- |
| `apps/desktop` | Tauri v2 / Vue 3 客户端、五页导航、`RemoteWorkspace.vue` 实际 LAN 控制 UI |
| `crates/hub-capture-win` | DXGI Desktop Duplication |
| `crates/hub-encode-win` | Media Foundation H.264 |
| `crates/hub-webrtc` | WebRTC、H.264、LAN、ICE、DataChannel、`remote_input.rs`、SendInput |
| `crates/hub-identity` | Ed25519 / 当前 Windows 用户 DPAPI、一次性会话签名验证 |
| `crates/hub-rendezvous-probe` | Axum 服务端 `src/bin/v2_signaling.rs` 与 V2 回环集成测试 |
| `crates/hub-signal-v2` | 原生回环 WebSocket 客户端与共享信令协议 |
| `crates/hub-turn-credentials`、`hub-probe-wire` | TURN / 信令协议现有基础 |
| `scripts` | 少量本地工具链检测、开发与验证命令 |
| `deploy/coturn` | 后续 TURN 的示例配置；不代表已部署 |

这里**不包含**旧分支的 `artifacts/` 大量证据、历史 `docs/`、旧任务/报告；它们可在原 `main` 中查阅。代码依赖和自动测试文件随源码保留。本分支不宣称旧证据已在新工作树复现。

## Windows 上快速编译

在已安装项目锁定的 Rust 1.100.0、MSVC/Windows SDK、Node 24、pnpm 11.25.0 和 WebView2 的 Windows PC：

```powershell
git fetch origin
git switch v2/clean-core
git pull --ff-only
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test
cargo test -p hub-webrtc --features p3-lan --locked
cargo test -p remotehub-desktop --features p3-lan --locked
cargo test -p hub-signal-v2 --locked
cargo test -p hub-rendezvous-probe --bin v2_signaling --locked
pnpm build
cargo build -p remotehub-desktop --profile probe --features p3-lan,custom-protocol --locked
```

构建输出由 Cargo profile 决定，通常在 `target/probe/remotehub-desktop.exe`。**最后的 build 仅编译，不会启动或采集桌面。** 实际打开两台电脑的产品程序、测试真实画面/输入/停止前，先明确用户批准并记录 EXE SHA256。

WebSocket 本地信令独立程序采用 `REMOTEHUB_V2_TOKEN`（临时随机 64 位十六进制）、`REMOTEHUB_V2_BIND`（默认 `127.0.0.1:48620`），默认仅允许回环，TLS 证书/私钥须成对配置。握手使用 Rust 客户端的 Bearer 请求头；共享 token 只用于访问本地中继，不是设备身份。端到端信令复用配对密钥和 HMAC，将消息的房间、会话、方向及顺序绑定；这不加密信令。桌面诊断默认不编入：仅 `v2-loopback-diagnostic,custom-protocol` 构建显示入口，原生确认后连接固定的 `127.0.0.1:48620` 并验证四条内置消息及退出；需先单独启动本机中继，两个进程使用同一临时令牌。不会连接其他设备、采集屏幕或操作键鼠；不替代真实 LAN TCP 路径。

原生库的回环 WSS 使用独立配发的 CA 验证服务端证书链与 IP SAN，不修改系统信任，握手失败不回退到明文。回环测试另用两枚临时 Ed25519 身份签名，通过预先信任的完整公钥校验房间、会话、角色、一次性挑战、有效期及信令内容，再使用 SDP/ICE 建立 DataChannel。两端在 WSS 上交换一次性挑战，再验证签名；联测分别通过公开配对卡和独立提供的完整指纹绑定双方公钥。任一端发起挑战后，必须收到双方挑战才允许发 offer；旧诊断路径保留原有行为。原生 `v2-verified-signaling` 模式在本机 WSS 上只接受各一次携带完整候选的签名 offer/answer，不允许额外 trickle ICE 或明文回退。使用错误的对端公钥时，在 SDP 交给 WebRTC 前关闭连接。回环测试不代表跨机产品已验收。默认桌面远控继续使用 LAN TCP，桌面本机诊断仍只使用回环 WS；未部署云端。

Axum 中继的显式 `REMOTEHUB_V2_MODE=private-lan` 模式只接受指定的 RFC1918 IPv4 和非零端口（`REMOTEHUB_V2_BIND`），强制配置 `REMOTEHUB_V2_TLS_CERT` 与 `REMOTEHUB_V2_TLS_KEY`。两个角色必须配置不同的实际 TCP 来源地址（`REMOTEHUB_V2_LAN_HOST_IP` / `REMOTEHUB_V2_LAN_VIEWER_IP`）和不同的随机令牌（`REMOTEHUB_V2_LAN_HOST_TOKEN` / `REMOTEHUB_V2_LAN_VIEWER_TOKEN`，各 64 位小写十六进制）。来源从套接字读取，不信任转发头；角色从来源及对应令牌限制，令牌不能代替设备签名。私网模式在双方各发一次挑战之前不转发 offer。原生客户端固定来源和目的 IPv4，校验预置信任 CA 和目标 IP SAN，不解析域名、不跟随重定向、不降级明文，也不修改系统信任。中继可与一台测试设备同机，但两角色来源仍须不同。私网模式已在同一局域网的两台 Windows 电脑上验证；未验证公网服务。

启用 `v2-peer-pairing,custom-protocol` 构建后，可在「设备」页读取并复制本机配对卡与完整 SHA-256 指纹。把公钥卡交给另一台机，并通过另一条可信渠道核对完整指纹；两边都可分别验证。短设备 ID 不能用于确认；拒绝自配对、错误指纹与不合规公钥。原生确认默认为「否」，新确认成功才替换本应用内存的上一个对端，退出即清除。此步骤不需要服务器、不发网络请求、不连接设备，也不授予观看或输入权限。

在 `v2-verified-lan,custom-protocol` 构建中，两边都明确选择「已验证设备（仅视频）」。先在「设备」页各自核验对方的完整指纹，再指定对方的 LAN IPv4，使用本次临时配对码并分别原生确认。该模式先验证双方持有对应设备密钥，再验证签名的 offer/answer 和指定 UDP 候选。没有 DataChannel 或键鼠；会话不自动降级为普通模式。原有 LAN TCP 模式仍为默认选项。对已存身份仅做只读重载，失败不重建或修复身份。两台 Windows 的实测结果见下方，包括手动停止、自然结束和未核验拒绝。

启用 `v2-private-wss-lan,custom-protocol` 后，桌面允许手动选择「私网 WSS 信令（仅视频）」。仍需完整指纹核验、当前临时配对码、原生批准以及明确的两台电脑 LAN IPv4。两个桌面进程各自通过本机环境提供 `REMOTEHUB_V2_DESKTOP_RELAY`（同一字面私网 IPv4:端口）、`REMOTEHUB_V2_DESKTOP_CA_DER_HEX`（独立可信渠道提供的 CA DER 小写十六进制）、`REMOTEHUB_V2_DESKTOP_TOKEN`（仅该电脑角色的临时令牌）。地址或证书格式不正确时拒绝会话；令牌和证书不经过 WebView IPC。窗口的原生确认显示选定的中继地址。此配置不改系统信任或防火墙。

TCP 的双方身份验证同时绑定所选中继地址。每个 WSS 房间和信令密钥从该次新鲜握手独立派生。WSS 仅用于经过设备签名的 offer/answer；中继可见信令，不能改写并通过设备签名。观看端验签并确认候选后，用现有 TCP 反馈并进入原停止/心跳流程。WSS 模式不发送鼠标/键盘事件，不接受未核验设备或模式混用，不回退；Windows 私网双机已确认正常画面、共享端手动停止后两端结束，以及关闭中继后无法建立新连接，见下方实测记录。

## 私网 WSS 双机测试准备（操作前先确认）

测试工具 `v2_lan_test_setup.exe` 只离线生成约一小时有效的测试证书和两份角色配置，**不会启动进程、进入远控、导入系统证书或修改防火墙**。建议中继与共享端同机；两台电脑使用互不相同的局域网 IPv4。测试结束退出程序、关闭 PowerShell 窗口，并清理这次专用目录和观看端副本；不要重复使用这套证书和令牌。

共享端将下面两个示例地址替换为两台电脑实际的局域网 IPv4；Windows 输出必须是当前用户 LocalAppData 下一个尚不存在的直属子目录。先解压测试包，进入其目录再运行：

```powershell
$sharingIp = '192.168.1.20'
$watchingIp = '192.168.1.21'
$testDir = Join-Path $env:LOCALAPPDATA ('RemoteHub-WssTest-' + [guid]::NewGuid().ToString('N'))
.\v2_lan_test_setup.exe $sharingIp $watchingIp $testDir
```

`host/host.json` 和中继私钥只留在共享端，生成后不要移动该目录（其中使用绝对路径）。只把 `viewer/viewer.json` 通过可信的私下方式交给观看端，勿发到聊天、提交仓库或放在公用目录。生成器仅显示公开的 CA SHA-256，不显示令牌。观看端应通过独立可信渠道核对生成器显示的指纹；不要把令牌当作设备身份，也不要从 TLS 提示中接受临时证书。中继路径默认 TCP 48620，设备既有直接 TCP 会话用 48604。遇到 Windows 防火墙/权限提示先停下，不改设置。

### 用角色启动脚本，免手动加载环境变量

将仓库的 `scripts/start-private-wss.ps1` 放到解压后的测试包目录，与 EXE 同级。实际测试先取得同意，再从该目录分别运行下列命令。每条独立执行；路径在提示出现后粘贴，不加引号。若脚本被本机执行策略拦截，先停下，不修改策略。

- 共享端第一个 PowerShell 窗口，选择刚生成的 `host.json`。保持窗口打开，结束时按 Ctrl+C。
  ```powershell
  .\start-private-wss.ps1 -Role Relay -Profile (Read-Host 'host.json path')
  ```
- 共享端第二个窗口，选择同一份 `host.json`。
  ```powershell
  .\start-private-wss.ps1 -Role Host -Profile (Read-Host 'host.json path')
  ```
- 观看端选择私下收到的 `viewer.json`。脚本要求输入共享端生成器显示的完整 CA SHA-256，不一致就拒绝启动。
  ```powershell
  .\start-private-wss.ps1 -Role Viewer -Profile (Read-Host 'viewer.json path')
  ```

脚本仅为所选进程传递该角色需要的固定环境变量，不把令牌写入命令参数、打印到终端或改写系统环境。它不修改证书信任、防火墙或启动远控会话。启动后仍需双方核验设备完整指纹、手动选择「私网 WSS 信令（仅视频）」并分别批准本次会话。只共享空白记事本的测试画面；点「停止并回收」，关闭两端程序和中继窗口。不启用键鼠。只反馈结果和固定错误码，不上传截图、配置、临时配对码、证书私钥或令牌。

## 公网选路探测（非产品入口）

`ice-path-probe` 还是单机双原生 PeerConnection、仅合成文本的显式探测工具，不采集桌面、不发键鼠。每侧只用在自身授权范围内验证过的 offer/answer 候选检查实际选中对；发包前后分别核对，没有读到新候选对时不把旧值当成最终结果。报告保留粗分类 `path`（直连/中继），另用 `route` 区分私网主机、公网主机、STUN 映射候选和 TURN 中继，用 `selected_phase` 区分流量前后；都不输出 IP、端口或凭据。仅收集到候选、或选到私网主机候选，并不证明公网连通。外网测试须确认具体端点和范围后另行执行。

跨网双机在单独的 Windows `p007b-diag` 构建验收，只生成 8 秒合成图案，不采集桌面、不注入系统键鼠，不自动启动会话。双方就同一个获批 WSS 端点各自确认原生弹窗（显示 IP 和端口）。两台设备各保留一份私有 `r06-private.json`：相同的 `server_ipv4`、`signaling_port`、可信 `ca_pem` 和本次 `invitation_expires_at`，发送端另含且只含 `sender_invitation`，接收端另含且只含 `receiver_invitation`。错角色、无令牌或同时包含两份令牌均在联网前拒绝。不要发送整个配置、令牌或配对码到聊天或仓库。旧的单机 RPC 合成测试仍按原双邀请格式，不用于双机验收。

R06 诊断版要求两端分别手动选择同一个模式：「优先直连，允许中继」或「仅 TURN 中继」。优先直连允许 ICE 在建连时选中 TURN；本诊断不验证播放中途切换。「仅 TURN」仍需原生同意和 8 秒合成图案，双方核对实际选中的 UDP 中继候选分别存在于当前认证过的 SDP 中；未选中、选了同范围的其他端口或握手模式不同都不能记为通过。该模式对外信令移除候选的关联基址/端口并清理其他地址字段；不改变 ICE 内部的套接字。这个模式不修复直连的私网 host 候选问题，也不把仅 TURN 结果计入直连。

跨网验收标准：两台 Windows 分别走宽带和热点，不填私网对端，确认收到并解码合成图案，两侧实际选中的候选匹配已验证信令且走非中继公网路径，最后两端均释放资源。若实际选中 TURN，只记录中继结果，不算公网直连；受控回退测试须另行批准。只有候选收集、画面或最后一次旧样本不能判定选路通过。实测记录见下文。

### 构建 Windows x64 便携诊断包

在已安装项目工具链的 Windows 电脑上，从 **Developer PowerShell for VS（x64）** 进入干净的 `v2/clean-core` 检出目录后运行：

```powershell
.\scripts\build-r06.ps1
```

脚本只构建并打包，不运行应用、不安装 SDK、不启动公网诊断。`artifacts` 下输出带提交号的未签名 ZIP，只含 `remotehub-r06-synthetic.exe` 和该程序的 SHA-256 清单；不会把现有配置、证书或测试统计放进包。此包不是安装器，运行需要 WebView2；脚本被系统策略拦截时先停下，不绕过策略。

### Windows WebView2 离线显示检查

独立的 `remotehub-display-check` 只打开内置静态页面，不注册产品命令，不建立 WebRTC/WSS 会话。需要 Windows x64 和现有 WebView2 Runtime，不需要安装 RemoteHub。构建：

```powershell
cargo build --locked -p remotehub-desktop --bin remotehub-display-check --profile probe --features synthetic-display-check,custom-protocol
```

早期测试包误加载主程序的 Tauri 配置，会多开窗口且找不到播放按钮。请不要继续使用旧包。修正版把测试配置放入独立目录，并在启动前核对包内的配置标识和静态资源。

运行后点「播放并测试」。看见左上红、右上绿、左下蓝、右下黄和移动白块，且显示 `DISPLAY_PASS`，才算这台机器的 WebView2 本地解码/呈现通过。点「停止」可结束；「重置」后可重试。若失败，记录状态码；无需提供屏幕截图或设备信息。

### Windows WebView2 单机签名串流检查

独立的 `remotehub-stream-display-check` 启动进程内回环 WSS 和模拟 STUN，用临时身份完成双向核验及 WebRTC DTLS/SRTP 传输。原生仅向本窗口返回两帧内置的 32×32 灰阶图案，不读取用户文件、不采集屏幕、不注入输入、不连接真实服务器。Windows x64 需要现有的 WebView2 Runtime。重新构建：

```powershell
pnpm --dir apps/desktop exec tsc -p stream-display/tsconfig.json
pnpm --dir apps/desktop exec vite build stream-display --base ./ --outDir dist
cargo build --locked -p remotehub-desktop --bin remotehub-stream-display-check --profile probe --features synthetic-stream-display-check,custom-protocol
```

运行后点「开始本机诊断」。画布可见四个从暗到亮的灰阶区域、且出现 `STREAM_DISPLAY_PASS`，才算 Windows 显示通过。点「停止并清屏」后画面应变空白，状态显示 `STREAM_DISPLAY_STOPPED`，旧画面不能恢复；可再启动验证。这里验证的是两帧的单机诊断，不是连续帧速率或公网会话。

## 本分支已执行验证

- 单机签名串流诊断：原生本机联测通过，验证两帧证据及取消清理；WebView2 页面通过 TypeScript、Vite 构建和模拟收帧、解码回调及停止检查。Windows 实机首轮报告 `STREAM_DISPLAY_PIXELS_MISMATCH`，失败路径清屏，未通过。修订诊断优先使用软件解码及便于读取的 2D 画布，仍要求四个区域满足原灰阶判定，失败时显示四区 RGBA 读数用于复查；修订版尚待 Windows 复测，不作为公网或实际桌面验收。
- 已校验串流的显示交接：原生接收器只从已验签且通过实际 ICE 核验的会话读取 RTP，按严格序号和时限组装有界 H.264 Annex B 帧；缺片、错误格式或停止均不得继续产出画面。Web 前端新增独立的 WebCodecs 显示组件，停止立即清空画布并关闭解码器，拒绝旧一轮的迟到回调。当前分别测原生回环解码与页面状态，尚未通过 Windows WebView2 实机验证两者之间的连接，也未启用公网入口。
- WebView2 离线合成显示入口：独立二进制无远控或采集命令，前端仅播放内置的 320×180、24 fps、2 秒 H.264 文件，使用视频帧呈现回调及四个区域的像素核对。FFmpeg 已离线解码 48 帧验证图案；Windows 实机（2026-11-23，用户实测，修正版 v2 `95c59cc`）：四色图案可见，状态为 `DISPLAY_PASS`。该结果确认这台机器的 WebView2 能解码并呈现随包附带的本地合成 H.264 图案，不覆盖 WebRTC 串流、真实桌面或公网产品会话。
- 合成画面解码：可选本机联测使用 FFmpeg/libx264 生成 32×32 四象限图案，经过已验签 WSS、真实回环 WebRTC 及 RTP 候选核验。接收端核对序号、RTP 时间戳及帧结束标记，重组后由 FFmpeg 解码，比较整帧亮度和色度样本（允许最大偏差 4；本次连续三轮的实测最大偏差均为 0）。复跑：`cargo test -p hub-rendezvous-probe --bin v2_signaling --locked public_video_wss_decodes_synthetic_pattern_pixels -- --ignored --nocapture`。FFmpeg 仅是这个测试的工具，不是产品依赖；未测 Windows WebView2 显示、真实桌面采集或公网。
- 公网仅视频的本机停止联动：新增会话驱动统一持有 WSS 和 WebRTC，主动停止或发现对端离开后不再接受 RTP，关闭原生 PeerConnection 并报告回收结果。回环 TLS/模拟 NAT 联测覆盖正在收发时主动停止及对端 WSS 意外断开；检查停止后收发均拒绝、接收端 PeerConnection 关闭、房间和连接槽释放。未接入桌面产品入口或部署公网。
- 公网仅视频的 RTP 放行：写入前检查当前发送轨道的 DTLS/ICE 候选对；接收端在应用读取和交付包时也验证实际轨道。回环 WSS + 进程内 STUN/NAT 测试让真实 WebRTC 选中已签名的映射候选，收到了合成 H.264 RTP 测试字节；未签名的回环选路禁止写入且未触发接收轨道。测试网络只用回环，不访问公网，不解码或渲染画面。正式会话的授权、停止联动及实际媒体仍待接入。
- 公网仅视频的原生候选读取：从唯一且方向正确的视频 RTP transceiver 的 DTLS/ICE 传输读取当前选中的 UDP 候选对，不借用 SCTP。读不到、超时或不匹配即关闭信令。本机测试建立两端真实 WebRTC 视频传输（无媒体帧），证实未列入签名 SDP 的已选回环候选对被拒绝、服务端房间释放；未做成功播放验收或开放公网入口。
- 公网仅视频的 ICE 核验已接入本机签名 WSS：发送前和对端验签后分别检查仅视频 H.264 SDP 与确定范围内的 IPv4 UDP 候选；拒绝额外媒体/数据通道、非内嵌候选和未批准的 TURN 地址或端口。签名交换完成后，候选对必须逐项匹配两端当次 SDP（含方向、IP、端口和候选类型），不匹配即关闭信令。回环 TLS 测试使用合成候选统计，不发公网 ICE 包、不传输视频；实际媒体会话及现场验证仍待接入。
- 公网仅视频签名协议的本机 WSS 接入：新增可选 `v2-public-video-wss`，复用原生客户端和 Axum TLS 路由，只允许连接回环。测试覆盖签名 offer/answer、证书不受信、选路模式/设备指纹不符、重复 offer、主动停止和连接断开；结束后服务端房间及连接槽均释放。签名中携带的公网 WSS/TURN 地址仅用于核验，不会被连接；本增量不开放入口、不接入实际 ICE 或媒体、不改变已有 LAN/WSS 路径。
- 正式公网仅视频会话的身份核验基础：新增未接入桌面入口的 `v2-public-video`。双方用已核验的设备指纹签名一次性 offer/answer；签名绑定本次会话、明确选路模式、WSS 端点和 CA 指纹、TURN 端点及中继端口范围与有效期。answer 必须对应已验证的原始 offer。离线 Rust 回归共 63 项通过（含 4 项新测试）；不代表已接入公网会话。后续仍需连接 WSS、验证实际 ICE 候选、原生许可及停止回收；本增量不开放入口、采集桌面或启用键鼠。
- R06 建连时受控自动回退公网双机复测（2026-11-23，用户实测，Windows v6 `656971b`）：两端选「优先直连，允许中继」，仅发送端模拟直连不可用。发送端报告拦截 3 次直连 ICE 检查（共拦截 6 个包），原生候选校验本地与远端均 `matched`，实际走中继；接收端显示 `relay / relay`、当前中继、路径校验通过，正常解码 240 帧（1280 × 720），停止后显示资源回收已完成。接收端页面的原生发送端候选字段未取得，本次原生证据来自发送端报告。以上通过的是跨网合成视频建连回退，不覆盖播放中切换、真实桌面、键鼠或正式产品集成，R06 仍 OPEN。
- R06 建连回退诊断：仅合成发送端可为本次 `DirectPreferred` 手动开启进程内故障，原生同意界面明确展示。保留 ICE All 与 STUN/TURN，仍精确校验两端候选。统计单独记录向认证 Host/Srflx 端点丢弃的出站 ICE Binding 检查；没有命中不能判定为回退成功。只验证会话建立时回退，不验证播放中途切换。47 项 Rust 本地测试通过，覆盖 UDP 故障范围、保留指定服务器及角色限制；169 项前端测试、Windows MSVC 编译检查通过。
- R06 优先直连公网双机复测（2026-11-23，用户实测，Windows v5 `839d61a`）：合成图案正常；浏览器显示 `srflx / srflx`、当前直连、路径校验通过，资源回收显示「已完成」。本次公网 STUN 映射直连成功（用户报告；未单独核对原生端候选日志），没有走 TURN。这一轮不覆盖受控自动回退、真实桌面或键鼠，R06 仍 OPEN。
- R06 直连候选修正：锁定的 WebRTC 封装增加可选 Host 收集开关。原生 DirectPreferred 未获准发布私网地址时，在 ICE 收集阶段禁用 Host，保留 STUN/TURN 收集及实际候选对的精确核验。双机合成诊断还会脱敏对外 SDP 的关联地址和端口，不改内部 ICE 套接字。46 项 Rust、165 项前端测试及 Windows MSVC 编译检查通过；回环模拟 STUN 在两次收集中都保留映射候选且不产生 Host。未做公网直连或自动回退复测。
- R06 强制 TURN 公网双机复测（2026-11-23，用户实测，Windows v4 `711b2f3`）：用户确认合成图案正常发送和接收；浏览器界面显示 `relay / relay`、当前中继、路径校验通过，资源回收显示「已完成」。强制 TURN 合成视频验收通过（用户报告；未单独核对原生端候选日志）。未测试公网直连、自动回退、真实桌面或键鼠，R06 仍 OPEN。
- R06 强制 TURN 首轮公网未连上，接收端只显示 `RECEIVER_INTERNAL_ERROR`。本机代码确认新诊断发送的 `connectionPath` 会被旧通用指标校验拒绝；现仅在强制 TURN 合成诊断里校验该字段，且仍要求两端匹配的中继候选，并把固定阶段错误保留为脱敏代码。44 项 Rust 本地测试、163 项前端测试和 Windows MSVC 编译检查通过；这不能证明公网失败只有这一处原因，未重新运行公网测试。
- R06 手动仅 TURN：`cargo test -p hub-webrtc --features p006-session-probe --lib --locked --offline` 43 项通过，新增本机脱敏、ICE 解析和精确候选校验覆盖；前端 141 项测试、类型检查及构建通过。Windows MSVC 目标的诊断构建及测试源码通过编译检查；未连接真机或公网 TURN。
- R06 公网双机复测（用户操作）：匹配版本的 WSS 服务端使配对进入 ICE 阶段。发送端 `DIRECT_SELECTED_PAIR_NOT_APPROVED`；脱敏最终统计：本地候选 `host/private`，`candidate_type_absent_from_sdp`，已发布候选 2 个、同类型 0 个、未批准发布私网候选；远端 `matched`。接收端同时报 `CONTROL_CHANNEL_ERROR`。当前原生 ICE 仍可能选中仅在 SDP 阶段过滤掉的候选，拒绝是预期行为；不放宽核验。未验证公网直连、TURN 转发或画面；R06 仍 OPEN。
- R06 首轮公网尝试（用户操作）：信令 TCP 和 TURN UDP 均已监听，但发送端未进入等待对端便报 `P006_WSS_CLOSED`，未验证实际选路或画面。旧信令程序缺少 `direct_preferred` 标记；历史 `977f97f` 的握手严格拒绝这个字段。当前源码 `e186880` 的静态 Linux x64 信令程序已通过本机 WSS 实测：R06 发送端收到 `waiting`、双角色收到 `ready`，旧格式不带模式字段也可等待；`cargo test --locked --offline -p hub-rendezvous-probe --bin hub-rendezvous-probe` 6 项通过。新程序未上传云端或部署，未验证公网、TURN 转发或实际画面。
- Windows x64 便携合成诊断包：应用源码 `6e149c7` 已用 `cargo build -p remotehub-desktop --target x86_64-pc-windows-gnullvm --profile probe --features p007b-diag,custom-protocol --locked` 交叉链接。已静态检查 AMD64 PE、动态依赖及 ZIP 内容；LLVM 版同目录提供匹配的 `WebView2Loader.dll` 和文件校验清单，未签名，不附私有配置，未在 Windows 实机运行。ZIP SHA-256：`61b2eb30ae68c57591938311b456112d52139af30f77e5d5d3072aa0cef7c11b`。此构建不等于公网验收。
- `scripts/build-r06.ps1` 在 PowerShell 7 用替身编译器验证了 ZIP 文件白名单、可核对的 SHA-256、临时文件清理及拒绝覆盖/脏工作树；没有用替身产物冒充可执行包。前端冻结依赖、构建及 135 项测试通过。
- Linux：`cargo test -p hub-webrtc --features p006-session-probe --lib --locked --offline` 通过（42 项）；覆盖发送、接收各自配置、错角色、双令牌拒绝及单机双令牌格式保持。后续已通过 `cargo check -p remotehub-desktop --target x86_64-pc-windows-msvc --features p007b-diag,custom-protocol --locked --offline`；这是编译检查，不等同于链接出正式 MSVC EXE 或实机测试。
- Linux：`cargo test -p hub-webrtc --features p006-direct-probe --lib --locked --offline` 40 项通过。新增自动回归只绑定 `127.0.0.1`，使用两端各自的授权范围建立真实原生 WebRTC DataChannel，100 条合成文本全部收齐并收到 ACK；读取两端实际选中的 UDP 主机候选对后，双方正常关闭。测试入口不编入正式探测器；`cargo check -p hub-webrtc --features p006-direct-probe --bin ice-path-probe --locked --offline` 通过。此前 `p006-session-probe` 编译检查也已通过。未启动 STUN/TURN，未做跨网双机验证。
- Linux PowerShell 7.4：`pwsh -NoProfile -File scripts/test-private-wss-launcher.ps1` 通过；覆盖角色与入口匹配、指纹错误、错误角色与损坏配置拒绝、子进程环境隔离。使用替身文件，不启动程序或中继；新脚本尚未在 Windows 实机验证。
- Linux：`pnpm install --frozen-lockfile`、`pnpm typecheck`、`pnpm test`（135 项）、`pnpm lint`、`pnpm build` 通过。
- Linux：`cargo test -p hub-webrtc --features v2-private-wss-lan,v2-verified-signaling --locked` 36 项通过、1 项忽略；覆盖 WSS/TCP 会话引导、不同模式/中继拒绝、精确 answer 的 TCP 确认及旧 LAN v1 帧逐字节兼容。
- Linux：`cargo test -p hub-rendezvous-probe --bin v2_signaling --locked` 23 项通过，覆盖私网规则、真实回环 TLS 套接字的来源和角色检查、WS/WSS 的原生 WebRTC 回环链路、双向挑战顺序与防重放、设备签名后应用 SDP/ICE、内联候选及错误设备公钥的拒绝、取消清理和禁止降级；未采集屏幕或注入键鼠。`cargo test -p hub-signal-v2 --locked` 6 项、`cargo fmt --all -- --check` 通过。
- Linux：`cargo test -p hub-identity --locked` 12 项通过，覆盖只读重载的非 Windows 拒绝、手动配对的公钥/指纹校验及本机身份绑定，以及签名中的角色、篡改、过期与重放。
- Linux：`cargo test -p hub-webrtc --features v2-verified-lan --locked` 26 项通过、1 项忽略；含 4 项内存双端测试，拒绝错指纹、错端口、错临时码、旧协议和签名篡改/重放。
- Linux：`cargo test -p hub-rendezvous-probe --bin v2_lan_test_setup --locked` 3 项通过，覆盖角色配置分离、绝不覆盖既有输出、签发的证书精确 IP 校验和有效期；只生成测试夹具。Windows x64 目标的权限检查源码通过编译和 Clippy，尚未在 Windows 真机运行生成器或权限测试。
- Windows x64 MSVC 目标：桌面常规 `p3-lan`、可选 `v2-loopback-diagnostic` 、`v2-verified-lan` 和 `v2-private-wss-lan` 交叉检查、诊断、配对及已验证设备 LAN 构建严格 Clippy、WebRTC 目标测试代码检查通过；未在 Windows 真机运行诊断版。交叉检查本身不是真机证据。
- Windows 实机（用户报告，2026-11-23；`e51491c` 测试包）：两台 Windows 在同一局域网选择「私网 WSS 信令（仅视频）」。中继运行时观看端有画面；共享端点「停止并回收」后，两端均正常结束；关闭中继后无法建立新连接。未验证中继中断对已建立会话的影响，不覆盖公网、TURN 或键鼠。
- Windows 实机（用户报告，2026-11-22；`c8122d2` 测试包）：两台电脑在同一局域网互相核验对端配对卡和完整指纹后，观看端画面正常；共享端点「停止并回收」后，观看端也结束。补测通过：不手动停止时，两端在会话时限后结束；一端未核验当前对端时拒绝会话、无画面且未退回普通模式。未覆盖双机错误公钥/篡改、键鼠、公网与 TURN。
- Windows 实机（用户报告，2026-11-22）：两台电脑在同一局域网使用同一份 `77479f2` Windows x64 包，按约定回归场景测试均正常（画面、自然结束及英文键入后手动停止）。这不覆盖公网、TURN、安装器或 R06 的全量可靠性验证。

## 敏捷开发约定

请阅读根 `AGENTS.md`：**修复关闭错误 → 稳定真实 LAN 会话 → 接通 Axum 信令 → 公网 STUN/TURN → Windows 安装器**。Agent 可连续提交代码/执行离线测试，不为每个小任务撰写 Plan/ADR/Contract/Evidence；每阶段在此 README 用少量文字记录实际结果、失败和遗留问题。真实桌面、键鼠和云端部署始终由用户审批具体操作。

Sunshine、Moonlight V+ PC 与 EasyTier 都不是本分支的必要依赖。
