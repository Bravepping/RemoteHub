# 首次工程交接

- 已完成（带证据）：P0-01 真实原生窗口，P0-02 首个 DXGI 主屏实验；完整检查和 6 Rust / 3 前端 / 1 浏览器测试通过。证据 artifacts/verification.txt、capture-cli.txt、native-window.txt。
- 相关文件：TASKS.md；docs/PHASE0_TEST_REPORT.md；crates/hub-capture-win/src/dxgi.rs；apps/desktop/src-tauri/src/main.rs。
- 未完成：P0-03 MF H.264、P0-04 WebRTC 视频、P0-05 DataChannel、P0-06 TURN、P0-07 全阶段结论。恢复先读报告再运行 pnpm verify。
- 阻塞：两机/TURN 出口需要第二台 Windows PC 和可授权测试节点；当前媒体与身份/配对/签名尚未实现。优先完成不依赖云的 P0-03。不要把本机 --consent 当作远程授权。
