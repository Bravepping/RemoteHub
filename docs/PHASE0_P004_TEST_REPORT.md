# P0-04a 原生 WebRTC / WebView2 本机视频测试报告

日期：2026-11-21，Asia/Shanghai。基线 `7fcbb51`，设计先行提交 `6d38af3`；实现 SHA 见本轮 BUILD_INFO / Git 提交。**只验收 P0-04a，同机通过不等于 P0-04 双机或 Phase 0 整体完成。**

## 结论

真实 DXGI → CPU NV12 → AMD Media Foundation H.264 → Rust webrtc-rs → DTLS/SRTP → 同一台电脑原生 Tauri WebView2 `ontrack/srcObject/video/getStats()` 已通过。

五轮各 15 秒的实验，编码/发送/接收/解码/呈现均为 450 帧；另一次 30 秒均为 900 帧。完整实验接收丢包 0，实际视频尺寸 1280×720，selected pair 的远端为 127.0.0.1。还完成主动停止（47 帧）和播放中关窗（原生发送 33 帧）的清理检查。源线程不读取 P003 文件，视频不经 IPC/base64/Canvas，也不播放磁盘 MP4。

用户亲自操作每轮勾选与原生同意、开始、最后的主动停止/关窗；Agent 实际打开、读取原生窗口状态并核对程序生成的 JSON。不是仅浏览器 E2E 或静态源码推断。公开证据已删除 sessionId，不含完整 SDP、私人地址、桌面视频或像素。

## 基线文档核对

开工时本机 HEAD 和 `git ls-remote origin refs/heads/main` 均为 `7fcbb519709a4655aa77aa4b8eca945c413891c2`。该提交的 README/TASKS 已写 P0-03 完成，故没有重复“修复 todo”。与用户审查文档所见不一致，可能是时间差或缓存，未凭空断定原因。当前 README/TASKS 按本轮实际结果更新。

## 环境与设计

| 项目 | 实测/采用值 |
|---|---|
| OS | Windows 10 Pro，19045.6466 |
| 主屏与编码器 | 2560×1440 DXGI，AMDh264Encoder，hardware=true |
| GPU / 驱动 | AMD Radeon RX 6750 GRE 10GB / 32.0.21045.16010 |
| WebView2 | 157.0.4341.60（注册表实际运行环境版本） |
| Rust / 库 | Rust 1.100.0 MSVC；webrtc/rtc 0.21.1、Tokio 1.54.1，Cargo.lock 固定 |
| 原生程序 | `--profile probe --features custom-protocol,p004-probe`；probe 继承 release，opt-level=3，LTO 关闭；内嵌构建后的前端，无开发 HTTP 服务器 |
| 转换与编码 | CPU BGRA 双线性缩放/BT.601 limited NV12；1280×720@30Hz、请求 4Mbps、Baseline、目标 GOP 60 |
| 授权 | UI 本次勾选 + Windows 原生确认 + 原生窗口/session 生命周期；实验 feature 默认关闭 |
| 队列 | 3 AU；满/发送错误明确结束整轮，不静默随机丢 P 帧 |
| 接口/源文件 | `hub-encode-win/src/stream.rs`、`hub-webrtc/{session,video,signaling}.rs`、Tauri `p004.rs`、Vue `features/p004` |

MF/DXGI/COM 只在专用 Windows 线程上存活，异步侧接收拥有所有权的 AU。原文件 probe 行为保留，新增 `request_keyframe`，原 capture crate 未修改。选择、契约与撤销边界见 ADR 0003 和 P004_FORMAT。

## SDP / ICE / H.264

- offer 与 WebView2 实际 codec stats：`video/H264` / 90000 / PT 102 / `level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f`。
- 原生只绑定 127.0.0.1 UDP，无 TCP listener、STUN/TURN、远程信令服务，mDNS disabled。浏览器 ICE server 列表为空，IPC 前过滤 LAN/mDNS/relay 候选，原生独立拒绝 off-host 候选、额外 data/audio m-line 和异常 SDP。
- 实测浏览器 selected pair：local `prflx` / remote `host`，remote=127.0.0.1；连接/ICE 都实际达到 connected。不是假定 Chromium 一定公布 loopback host 候选。初始 offer 的候选列表可为空，由浏览器对原生 127 答案的 ICE 检查完成 peer-reflexive 建连。
- 首 AU 校验 SPS/PPS/IDR；库 H264 payloader 处理单 NAL、STAP-A/FU-A。单 RTP payload≤1188，marker 只在 AU 最后一包，源 PTS 按 `round(pts*9/1000)` 转 90kHz，正常间隔 3000。15 秒最后 RTP timestamp=1,347,000，对应 MF last PTS=149,666,666（100ns）。
- 代表 15 秒样本实际 8 个 IDR，目标 GOP 60；本轮没有逐 IDR PTS 序列，因此不把数量一致进一步冒充逐间隔审计。P003 独立解码的 60 帧 GOP 验证保留。
- 本机 AMD 实际 SPS 为 **42041f**。初版程序逐字节要求 42001f，导致发送前报 `SPS_PROFILE_NOT_42001F`，这是真实失败，未记成功。差异是 Baseline profile 66 不适用的 constraint_set5_flag；ITU-T H.264 7.4.2.1.1 要求解码器忽略它。修复只影响兼容性判断，**未改写原 SPS**，并披露该驱动的编码规范瑕疵。真实 MF 60 AU → 880 RTP 包回归、WebView2 实际解码均通过。其他 profile/level/约束不匹配仍拒绝。

## 逐轮原生记录

见 `artifacts/p004a/native-session-summary.json`、`run-01..08*.json`。这些是脱敏程序输出，不是手填成功数字。

| 轮次 | 输入 / 编码 / 发送 | 接收 / 解码 / 呈现 | 接收包 / 丢包 | 播放时间 | 结束与释放 |
|---|---|---|---|---|---|
| 1 | 450 / 450 / 450 | 450 / 450 / 450 | 6224 / 0 | 15.842s | 限时结束；原生与浏览器关闭 |
| 2 | 450 / 450 / 450 | 450 / 450 / 450 | 5729 / 0 | 15.838s | 同上 |
| 3 | 450 / 450 / 450 | 450 / 450 / 450 | 3481 / 0 | 15.848s | 同上 |
| 4 | 450 / 450 / 450 | 450 / 450 / 450 | 3042 / 0 | 15.902s | 同上 |
| 5 | 450 / 450 / 450 | 450 / 450 / 450 | 3324 / 0 | 15.865s | 同上 |
| 6 | 900 / 900 / 900 | 900 / 900 / 900 | 5398 / 0 | 30.723s | 同上 |
| 7 | 47 / 47 / 47 | 47 / 47 / 47 | 303 / 0 | 1.698s | 用户主动停止；stopped=true，两端关闭 |
| 8 | 34 / 33 / 33 | 未采集关窗后的 JS 数字 | 未测 | 未测 | 播放中关窗；stopped=true、cleanupSucceeded=true，窗口/进程消失 |

第 8 轮已输入但未输出的最后一帧随整个编码器关闭丢弃，不继续发送残缺参考链；没有把它记成浏览器解码。关窗时 JS 页面卸载，接收数字保持缺失，不填假 0。

代表第 2 轮：原生批准后到 connected=373ms、到首 AU=920ms，源 setup=490.960ms、媒体墙钟=14985.955ms；前端从点击开始到首次发现解码/播放为 2147.6ms，**包含人工确认与约 350ms 轮询粒度**。不作为纯编码或端到端延迟。本轮完整样本该指标范围 2139.5–3711.8ms。

代表样本 RTP payload 6,518,451 字节 / 5729 包；MF AU 为 7,503,033 字节。库会去掉 AUD/filler/start code 并封包，不能将该差值当作丢帧。RTP 字节也不包含所有 UDP/IP/SRTP 开销。RTCP 实际观察 107 个包，远端 lost=0；本轮 PLI/FIR 均 0，因此未宣称端到端关键帧请求恢复已实测。旧快照中库 RTT=0.0 不构成 0ms 延迟证据；最终代码只在库有效测量次数>0 时返回 RTT，否则未测。

完整轮次的实际解码计数持续增长，videoWidth/Height=1280/720，freezeCount/totalFreezesDuration=0；没有观测到统计上的播放冻结或解码帧缺失。浏览器 currentTime 含播放时间线偏移，不能拿 450/currentTime 当作严格渲染 FPS。运动窗口为可见 WinForms 色条/移动块/计时，由 DXGI 捕获，并未直接送给编码器；第 3–5 轮期间运行于主屏右侧，源 desktop_updates 为 428–441，0 missed ticks。

## 拒绝、失败、停止与回归

| 项目 | 执行及结果 |
|---|---|
| 无同意、非法尺寸/时长 | 实际执行原生 Config 校验单元测试，拒绝在初始化之前；前端未勾选按钮禁用。未人工点击原生对话框“否” |
| 并发与错误 session/window | 生产 reserve/find/origin 校验路径单元测试：第二次 BUSY、foreign session 与非 main/外站 URL 拒绝。未伪称执行了两窗口恶意 IPC 实测 |
| 连接失败 | 真正创建两个 Rust loopback peer，只交 offer 而不在接收端应用 answer；真实 ICE 等待超时，原生 failed、0 编码/发包、driver 关闭，stop 再次调用通过 |
| 背压 | 实际容量 3 通道第四 AU 返回明确失败；源 sink 失败停止、PTS 重排拒绝。未在实机人为制造拥塞后宣称恢复 |
| 编码/NAL 失败 | 初版 SPS 拒绝曾在真实原生窗口发生，资源回收显示完成；已修复并加入驱动约束位回归。无 MFT/COM 清理与设备丢失保持已有受控测试；未物理重置 GPU |
| 主动停止/关窗 | 第 7、8 轮原生实测与 JSON；关窗前 join/close 成功才销毁窗口；随后进程与窗口均不存在。未保留该 GUI 进程的 OS 退出码，不能凭推测写 exit 0 |
| P003 CLI 实机回归 | Windows 交互桌面实际执行 2 秒：AMDh264Encoder、60 帧、零 missed ticks，CLI exit 0，ffmpeg `-xerror` 完整解码 exit 0。普通受限执行上下文最初 DXGI 拒绝访问，随后按授权在实际用户桌面上下文重跑；未修改系统权限设置 |
| 完整 `pnpm verify` | 7 项命令 exit 0；默认 Rust 24 项（capture 6 + encode 12 + webrtc 6），前端 5 项；完整原始输出 verification.txt/json |
| 全 feature 检查 | clippy all-targets/all-features -D warnings 与测试 exit 0；额外 Tauri 门禁测试使 Rust 共 25 项；见 additional-verification.json 与原始 txt |
| 私有真实 AU 解析 | 1 项默认 ignored 的开发测试另显式执行并通过：真实 MF 60 AU / 880 RTP 包 / SPS 42041f。普通 cargo test 的 ignored 如实保留，不计成默认 PASS |
| Edge 与最终构建 | 浏览器 E2E 1/1，exit 0；最终优化原生程序构建 exit 0。E2E 不替代上面的原生 WebView2 实测 |

收尾还加入了“窗口已停止但原生授权结果迟到时立即撤销、旧轮询不能复活已停止状态”的前端守卫及回归，以及无有效测量次数时 RTT 留空。最终重新构建、全检查通过；没有为这些不改变媒体格式的收尾更改额外采集桌面。

开发测试复现时曾把私有样本环境变量设为相对路径；Cargo 在 crate 目录运行测试，导致找不到文件（exit 101），记录保留在 actual-mf-relative-path-failure.txt。改为 Resolve-Path 得到的绝对目录后重新执行，60 AU/880 RTP 包通过，exit 0。不是把缺失样本一项标成默认通过。

## 复现

```powershell
pnpm install --frozen-lockfile
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p004.ps1
# 在窗口每轮明确同意并开始，5–30 秒；点击停止或关闭窗口
# 运动图案（开发期可见窗口，自动关闭，不做采集）
powershell -NoProfile -STA -ExecutionPolicy Bypass -File scripts/p003-motion.ps1 -Seconds 60 -Left 1290 -Top 60
# 按当前主屏尺寸选择可见位置；1280 宽的运动窗口需实际可见

. ./scripts/use-toolchain.ps1
$env:REMOTEHUB_VERIFY_DIR='artifacts/p004a-recheck'
pnpm verify
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
pnpm test:e2e
node scripts/p004-validate.mjs artifacts/p004a
# 新实测统计在 target/probe/artifacts/private/p004；完成足够启停后可对该目录运行同一校验器
```

`scripts/p004.ps1 -BuildOnly` 只构建；普通未开 feature 的构建不含这些 Tauri handler，UI 也不显示实验入口。原生窗口本轮写入的仅是统计，原始桌面没有导出；另做的 P003 回归文件在忽略目录。公共统计移除了 sessionId，完整 SDP/私人候选不落日志。已有实验数据不擅自删除。

## 审查、限制与下一步

关键文件：ADR 0003、stream.rs、hub-webrtc、Tauri p004.rs/ACL/CSP、Vue webrtc.ts/播放器、p004.ps1/p004-validate.mjs、TASKS/README。审查重点为授权先于采集、非 Send COM 不跨线程、有界队列/失败中止、真实 PTS、停止幂等、晚到授权撤销、SDP 限制、私有素材排除；检查记录见 P004_REVIEW.md。

未做 Windows 11、双机、公网 P2P/TURN、20 分钟长稳、真实 GPU 丢失、多 GPU/HDR、PLI/FIR 端到端恢复、端到端延迟或本轮完整 CPU/GPU/内存采样。未执行完整 RustSec cargo audit，不宣称依赖无漏洞；媒体依赖声明许可来自下载的官方包，见 media-dependency-licenses.json，非完整法律审查。同步驱动硬阻塞仍可能导致 CLEANUP_PENDING，不作硬实时保证。

截图仅用于初始原生 UI 检查，未另存播放截图/录像；私有原生文字记录和逐轮真实统计已保存。本轮没有伪造一张“播放截图”补证据。GUI OS 退出码未捕获等覆盖范围已在上表披露。

**P0-04a done；P0-04 doing；P0-05 todo；P0-06 blocked。**下一轮是受授权的两台真实 PC 视频互通及身份/会话边界设计；DataChannel 与 TURN 不在本轮。

📊 完成度：Phase 0 父任务 3/7（42.9%）+ P0-04a 子关卡通过 | 主线：进行中 | 产出：本机真实 WebRTC 视频与启停证据 | 待办：双机/DataChannel/TURN | 下一步：P0-04 双机范围对齐
