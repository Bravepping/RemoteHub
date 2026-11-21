# Phase 0 首轮验证计划

## 自动验证

- `pnpm verify`：前端格式、类型、交互测试、构建，Rust fmt/clippy/workspace 测试。首次需先安装官方依赖。
- `pnpm test:e2e`：真实 Edge 浏览器打开 Vite 页面，断言标题、浏览器预览提示、禁止本机采集和刷新后的失败状态；不注入假 IPC 成功状态。
- Rust 测试：未经本机同意时优先拒绝；零帧/超大帧/持续时间越界；GPU RowPitch 填充移除；缓冲截断/溢出/无效尺寸；BMP top-down 格式及禁止覆盖。
- CLI：缺少 `--consent` 或无效参数应退出 1，不访问 DXGI，不产生输出目录。

## 本机原生验证

1. `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/doctor.ps1 -OutputPath artifacts/environment.json`。
2. `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/dev.ps1 -Mode Desktop`。观察真实 WebView2 窗口，启动时不得采集；无同意时按钮不可用。
3. 明确勾选同意后启动实验，检查真实尺寸、帧数、计数；完成后同意勾选自动复位。失败必须显示底层错误。
4. `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/dev.ps1 -Mode Capture`：显式授权本机采集，生成 BMP 和 JSON。样本包含当前桌面，仅保留本地。
5. 退出窗口；检查程序不常驻、不监听远程端口、不再采集。

## 尚需独立验收

- Windows 11 实机、旋转/分辨率变化、锁屏与桌面切换、HDR、显示休眠、保护内容、光标合成。
- MF 720p30 H.264：实际编码、SPS/PPS/IDR/profile 与浏览器兼容。
- webrtc-rs → WebView2 连续真实视频、PLI/FIR、背压、实际 selected candidate pair。
- DataChannel 真实收发、有序性与负向授权校验；首轮不进行 SendInput。
- 两台 Windows 实机直连、coturn 强制 relay；观察双方视频/控制消息及候选对。

未执行项不能以单元测试或浏览器预览替代。实际结果见 PHASE0_TEST_REPORT.md。
