# P0-03 工程交接

- 已完成（带证据）：P0-01、P0-02 保留；P0-03 AMDh264Encoder 10 秒/300 帧/720p30/零丢 tick，标准解码、八色条、PTS、GOP 通过；软件 2 秒/60 帧通过。17 Rust / 3 前端 / 1 浏览器测试。证据 artifacts/p003/ 和 docs/PHASE0_P003_TEST_REPORT.md。
- 相关文件：crates/hub-encode-win/{src/mf.rs,src/probe.rs,src/h264.rs,src/color.rs}；docs/P003_FORMAT.md；ADR 0002；TASKS.md。视频样本仅在 artifacts/private/p003-accepted，不能提交/上传。
- 未完成：P0-04 WebRTC 视频、P0-05 DataChannel、P0-06 TURN、P0-07 全阶段结论。下一轮先做有界本机受授权 WebRTC -> WebView2；不进入 P1。恢复验证：设置 REMOTEHUB_VERIFY_DIR 到新证据目录，然后 pnpm verify。
- 阻塞/限制：真实硬件丢失与 Windows 11 未测；raw ffprobe 速率估计不代表实际 FPS，MF PTS 和 MP4 已验证 30Hz。远程身份/配对/签名/ACL 尚未实现，不要把本机 consent 当作远程授权；两机/TURN 出口仍缺第二台 PC 与节点。
