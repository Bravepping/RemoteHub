# P0-04a 交付检查点

- 已完成带证据：基线 7fcbb51，ADR 先行 6d38af3。用户亲自批准并操作原生 WebView2，五轮15秒450帧 + 30秒900帧全接收/解码/呈现、丢包0；主动停止47帧、关窗33帧清理通过，最终窗口/进程消失。artifacts/p004a 原始脱敏计数、pnpm verify、25 Rust(all features)、5前端、1Edge、显式1项私有真实MF AU测试均通过。
- 相关文件：stream.rs（Windows线程、不写视频）、hub-webrtc/session.rs/video.rs/signaling.rs、Tauri p004.rs/feature/ACL、Vue features/p004、scripts/p004.ps1、P004_FORMAT.md、PHASE0_P004_TEST_REPORT.md、ADR0003。默认 feature 关闭；显式脚本优化构建到 target/probe。视频统计仅存程序目录 artifacts/private/p004。
- 未完成：P0-04a done，原P0-04仍doing；P0-05/06未做，不进入P1。下一步双机范围与授权设计。剩余本轮流程是最终 Git 提交/上传及交付归档核验，最终SHA/包见后续会话记录。
- 已知风险/限制：AMD真实SPS42041f，按标准忽略对Baseline不适用constraint5做兼容，不改字节。browser无candidate offer由127答案建立prflx已实测；不能推广所有环境。原生同意需用户亲自操作。无Win11/长稳/物理GPU丢失/PLI端到端/RustSec；GUI OS退出码与播放截图未单独留存，报告明确。私有MF样本环境变量必须绝对路径，Cargo测试cwd是crate目录。

## P0-03 基线

- 已完成（带证据）：P0-01、P0-02 保留；P0-03 AMDh264Encoder 10 秒/300 帧/720p30/零丢 tick，标准解码、八色条、PTS、GOP 通过；软件 2 秒/60 帧通过。17 Rust / 3 前端 / 1 浏览器测试。证据 artifacts/p003/ 和 docs/PHASE0_P003_TEST_REPORT.md。
- 相关文件：crates/hub-encode-win/{src/mf.rs,src/probe.rs,src/h264.rs,src/color.rs}；docs/P003_FORMAT.md；ADR 0002；TASKS.md。视频样本仅在 artifacts/private/p003-accepted，不能提交/上传。
- 未完成：P0-04 WebRTC 视频、P0-05 DataChannel、P0-06 TURN、P0-07 全阶段结论。下一轮先做有界本机受授权 WebRTC -> WebView2；不进入 P1。恢复验证：设置 REMOTEHUB_VERIFY_DIR 到新证据目录，然后 pnpm verify。
- 阻塞/限制：真实硬件丢失与 Windows 11 未测；raw ffprobe 速率估计不代表实际 FPS，MF PTS 和 MP4 已验证 30Hz。远程身份/配对/签名/ACL 尚未实现，不要把本机 consent 当作远程授权；两机/TURN 出口仍缺第二台 PC 与节点。
