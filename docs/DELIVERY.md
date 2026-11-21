# 首次交付清单

历史首次交付内容如下。P0-03 后续交付另见 `PHASE0_P003_TEST_REPORT.md`，新增独立 `encode-probe.exe` 与源码/非画面验证证据；没有更新首次交付 ZIP 或声称 WebRTC 已完成。

- `RemoteHub.exe`：Windows x64 release 原生测试客户端，双击打开。
- `capture-probe.exe`：独立本机 DXGI 实验工具；仅开发验证使用，客户端无需启动此工具。
- `RemoteHub-source.zip`：本轮完整源代码、锁文件、原交付设计、任务状态、ADR、测试报告与非画面证据。
- `首次交付说明.md`：使用与开发说明。
- `PHASE0_TEST_REPORT.md`：详细测试结论、真实数据、失败处理、未验收项。
- `BUILD_INFO.json` / `SHA256SUMS.txt`：源码提交、构建信息与文件校验值。

首轮可验收 P0-01 与首个 P0-02 实验；尚不能远程连接另一台电脑。没有 H.264 编码/传输、DataChannel 或 TURN 的通过结论。完整 Phase 0 退出条件仍待实现与两机实测。

源码 ZIP 不包含本机 `.tools`、`node_modules`、Cargo target、Git 内部目录或 `artifacts/private` 桌面画面。可共享的真实计数和时间戳包含在 artifacts 中。

压缩包验收：文件可解压、校验值一致、源码关键文件/锁文件存在、私密帧和工具缓存不在源码档案中。原始设计包未修改。首轮不发布到任何外部服务。
