# RemoteHub 项目约定

2026-11-21 首次工程。Windows / Tauri v2 / Vue 3 / Rust，遵循原交付设计；Phase 0 真实媒体优先。不可用时报告失败，不显示假设备/RTT/FPS。不实现未授权后台监控、权限提升、凭据采集。

## Decision Record

首轮使用有限本机实验，理由与备选见 ADR 0001。远控身份未完成时不开放网络入口；本机显式同意仅作用本次本机采集。完成配对、签名与 Session ACL 后方可重新评估远程集成。

## Convention Capture

Rust fmt、TS strict、Vue/TS Prettier。模块依赖 Tauri -> hub-capture-win -> windows。输出样本放 artifacts/private 并排除 Git/打包。正确：测试返回值可来自 fixture；错误：产品显示固定假测量。每次以 artifacts 和当前实测结果更新 TASKS，不继承未实测的成功。

## Known Issues

DXGI 目前 CPU staging、主屏、无光标合成、旋转拒绝、access lost 终止。Windows 10 本机通过，Windows 11 未测。PowerShell 5 脚本使用 ASCII，避免无 BOM UTF-8 中文解析问题。此机交互 PATH 引号异常，在本次构建进程内修复，不修改全局设置。

## Glossary

观察 FPS：收到 DXGI 桌面更新的计数/实验总秒数，不能等同视频编码/传输/解码 FPS。首轮交付完成不等于 Phase 0 退出条件满足。
