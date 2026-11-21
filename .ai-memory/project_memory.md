# RemoteHub 项目约定

2026-11-21 首次工程。Windows / Tauri v2 / Vue 3 / Rust，遵循原交付设计；Phase 0 真实媒体优先。不可用时报告失败，不显示假设备/RTT/FPS。不实现未授权后台监控、权限提升、凭据采集。

## Decision Record

首轮使用有限本机实验，理由与备选见 ADR 0001。远控身份未完成时不开放网络入口；本机显式同意仅作用本次本机采集。完成配对、签名与 Session ACL 后方可重新评估远程集成。

P0-03 采用真实 MF MFT，AMD 硬件异步事件驱动与 Microsoft 软件同步回退，CPU 缩放/NV12；见 ADR 0002。真实 10 秒运动样本通过。视频时钟必须在 GPU 计数器初始化之后开始；允许有界短时落后复用最近真实 NV12。未来有实测瓶颈再评估 GPU 纹理路径。

## Convention Capture

Rust fmt、TS strict、Vue/TS Prettier。模块依赖 Tauri -> hub-capture-win -> windows。输出样本放 artifacts/private 并排除 Git/打包。正确：测试返回值可来自 fixture；错误：产品显示固定假测量。每次以 artifacts 和当前实测结果更新 TASKS，不继承未实测的成功。

## Known Issues

DXGI 目前 CPU staging、主屏、无光标合成、旋转拒绝、access lost 终止。Windows 10 本机通过，Windows 11 未测。PowerShell 5 脚本使用 ASCII，避免无 BOM UTF-8 中文解析问题。此机交互 PATH 引号异常，在本次构建进程内修复，不修改全局设置。

P003 raw Annex B 的 ffprobe r_frame_rate 是估计值，本机报告 60/1，不能覆盖实际 MF 30Hz PTS；MP4 已逐包检查 30fps/10 秒且解码帧与原始 AU 一致。AMD GPU 计数器叫 Video Codec 0，不能只看通用 VideoEncode 名称。WinForms 运动源必须 DPI aware / AutoScaleMode None，使用真实物理边界验证颜色。P0-03 不等于 WebRTC/浏览器或公网互通。

## Glossary

观察 FPS：收到 DXGI 桌面更新的计数/实验总秒数，不能等同视频编码/传输/解码 FPS。首轮交付完成不等于 Phase 0 退出条件满足。
