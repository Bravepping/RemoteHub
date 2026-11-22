# RemoteHub 开发任务看板

> 初始均为 `todo`；实际开始后改为 `doing`，测试验证完成后改为 `done`，否则 `blocked` 并说明原因。不得仅因生成了源码就把硬件验收项标记完成。

## Phase 0 · 先验证核心能力

- [x] `P0-01` **done** — Cargo/pnpm 锁文件、Tauri v2 原生编译、真实 WebView2 窗口启动与 IPC 刷新通过（Windows 10 22H2）；Windows 11 尚未复测。
- [x] `P0-02` **done（首个主屏捕获实验）** — AMD 主屏 2560×1440、30 帧/1059ms、28.315 观察 FPS、0 超时；本地 BMP/JSON；拒绝无同意和无效帧数。旋转/光标合成/HDR/桌面切换仍未支持或未验收，详见报告。
- [x] `P0-03` **done（本机 720p30 编码实验）** — 实际 AMDh264Encoder 硬件 MFT、10 秒/300 帧/零丢帧、Baseline L3.1、颜色/PTS/GOP/完整解码通过；软件 MFT 短时回退验证通过。物理设备丢失和长稳测试未执行，详见 `docs/PHASE0_P003_TEST_REPORT.md`。
- [ ] `P0-04` **doing（视频功能通过，基线证据有差异）** — 本机及两台真实PC局域网720p30视频已通过，启停/关窗/拔线回收已实测；原基线要求的录屏未留存，本次明确不保存画面，以真实stats和用户观看确认交付，父项暂不标done。
- [x] `P0-04a` **done（同机视频实验）** — Windows 原生 Tauri/WebView2，实时 DXGI/MF → webrtc-rs 0.21.1 → H.264 SRTP/解码；五轮 15 秒各 450 帧及一轮 30 秒 900 帧，接收丢包 0；主动停止与播放中关窗清理通过。默认关闭实验 feature，详见 `docs/PHASE0_P004_TEST_REPORT.md`。
- [x] `P0-04b` **done（有界局域网双机实验）** — 两台物理Windows10有线PC，v2的30秒900帧全部发送/接收/解码/呈现，1280×720、host/host指定对端UDP、接收丢包0；两端主动停止、接收端关窗与拔线提前回收通过。默认关闭LAN feature，详见 `docs/PHASE0_P004B_TEST_REPORT.md`。
- [x] `P0-05` **done（模拟控制，无实际输入）** — 复用P004b同一视频PC的真实可靠有序control.v1；两台Windows10电脑100条模拟事件发送/Rust接收/ACK写入/B确认全部100，ping/pong与序号连续、异常0，视频同时1280×720正常播放；两端停止、接收端关窗/拔线清理通过。43 Rust / 11前端 / 2Edge；协议与本机真实SCTP负例覆盖，详见 `docs/PHASE0_P005_TEST_REPORT.md`。
- [ ] `P0-06` **doing（跨网主链路通过，保留验收限制）** — G6及P006b真实双机relay视频/control已通过，包含实际双方selected pair；本次WSS/TURN已停止且不自启。用户决定跳过接收端补测、保留原日志限制；独立生命周期6例已验证，P2P优先与自动回退另进入P006c，不将父项提前标done。
- [x] `P0-06a` **infrastructure-verified** — 原implementation-ready的G1–5/G7/G8基础上，用户另行授权G6：Ubuntu24.04/coturn4.6.1，两次Windows原生真实relay候选（81/92ms gather），11项实际UDP协议检查通过，缺/错/过期票401、错realm437专用原因、五类危险peer403、释放与非root服务收尾通过。详见`docs/PHASE0_G6_TEST_REPORT.md`和`artifacts/p006a/g6`。原准备完整回归54 Rust/13前端/3Edge/3Python，新增4协议工具离线测试；未进入P006b/c、实际输入或Phase1。
- [x] `P0-06b` **done（用户接受现场确认，保留历史日志限制）** — 两轮30秒各900帧全发送/接收/解码/呈现、720p、丢包0/冻结0；5轮真实relay/relay UDP、每轮发送/Rust接收/ACK/确认100、连续1..101、异常0。发送端停止194帧、接收端关窗196帧、热点断网244帧提前回收；接收端主动停止用户已现场验证并明确跳过补测；900帧独立提前停止证据限制继续保留，不追填PASS日志。60 Rust/16前端/3Edge及完整构建/lint/clippy通过，云端服务已停止、不自启、socket清零；原逐会话TURN疑点由独立6例生命周期验证补齐（历史未归属socket不追溯定性）。见docs/PHASE0_P006B_TEST_REPORT.md、docs/TURN_ALLOCATION_LIFECYCLE_TEST_REPORT.md。
- [x] `TURN-LIFECYCLE` **done** — 6例真实云端/Windows自动测试：5例Refresh0即时释放，1例600秒租约超时回收；同PID、最多2分配、原租约+15秒无残留，停服务前socket已0。19组自动检查、61 Rust/11 Python通过。独立报告与Git交付先行，不重复双机串流。
- [ ] `P0-06c` **doing（真实云视频自动回退通过，物理双机待手动验收）** — Rust Session/前端/认证WSS同一视频control.v1链路，本地7轮及真实云WSS/TURN正常host/host、进程内阻断自动relay/relay均通过；两云轮各300帧720p30全接收/解码/呈现、100/100/100/100、异常0。9云分配Refresh0+socket消失，原600秒租约+15秒无残留；最多3，同PID，先判释放再停服务，最终不自启/凭据清理/业务配置FW不变。当前24前端/12 Python及类型/lint/build/3Edge、新Tauri构建通过；历史74 Rust/完整clippy回归保留未冒称重跑。待激活双机包已生成，未启动真实桌面采集/新物理双机/WebView2验收；见docs/PHASE0_P006C_VIDEO_CLOUD_TEST_REPORT.md，保持doing。
- [ ] `P0-07` **doing** — 已交首轮 Spike 报告、实测数据和 ADR 0001；整个媒体链路与备选方案结论需 P0-03..06 后补齐。

当前交付：P0父任务正式验收仍4/7（P001/02/03/05）；P004基线录屏差异保留，Phase0未通过退出条件。P006b按用户决定收尾，TURN生命周期独立6例已交付；P006c视频/控制/认证WSS集成及真实云自动回退通过，新版物理双机由用户后续另行批准和操作。2026-11-22 09:28:18专用WSS/TURN已停止、不自启，临时凭据及原始控制捕获已清理，不进入Phase1。

## Phase 1 · 工程与 UI

- [ ] `P1-01` Cargo + pnpm monorepo、格式化测试脚本
- [ ] `P1-02` 首页/设备/连接/会话/设置导航
- [ ] `P1-03` UI 卡片、连接弹窗和会话工具栏
- [ ] `P1-04` Pinia + Tauri IPC
- [ ] `P1-05` 空状态、加载、错误中文提示
- [ ] `P1-06` 托盘常驻与退出

## Phase 2 · 认证与信令

- [ ] `P2-01` Ed25519/DPAPI 设备身份
- [ ] `P2-02` Axum/SQLite/WS 服务端
- [ ] `P2-03` presence、心跳与限流
- [ ] `P2-04` 首次配对/批准/撤销
- [ ] `P2-05` 签名 SDP 信令、篡改/重放检查
- [ ] `P2-06` Session ACL/主动断开/超时清理

## Phase 3 · 远控闭环

- [ ] `P3-01` 集成 DXGI+MF+webrtc-rs
- [ ] `P3-02` 会话窗口收流与播放
- [ ] `P3-03` DataChannel 键盘鼠标注入
- [ ] `P3-04` 多 DPI/窗口缩放处理
- [ ] `P3-05` 断开与资源清理
- [ ] `P3-06` 被控端提示/一键停止

## Phase 4 · 公网 P2P/中继

- [ ] `P4-01` STUN/ICE 直连实测
- [ ] `P4-02` TURN 短时凭据与服务器配置
- [ ] `P4-03` P2P 失败使用 TURN 发送视频/控制
- [ ] `P4-04` 显示真实 selected candidate pair
- [ ] `P4-05` 断网/切网/有界重连
- [ ] `P4-06` 中继配额与滥用防护

## Phase 5 · 优化

- [ ] `P5-01` H.264 优先硬件编码
- [ ] `P5-02` 1080p60 真实性能测试
- [ ] `P5-03` 可生效画质/码率档位
- [ ] `P5-04` 弱网、关键帧、限流与积压丢帧
- [ ] `P5-05` 用户级登录后自启

## Phase 6 · 发布

- [ ] `P6-01` 安全负向测试
- [ ] `P6-02` Windows 安装包测试
- [ ] `P6-03` Linux 部署/备份/健康检测
- [ ] `P6-04` 隐私、日志脱敏和开源许可
- [ ] `P6-05` 最终 E2E 测试与已知限制
