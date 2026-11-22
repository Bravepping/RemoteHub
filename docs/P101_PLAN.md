# P1-01 工程基础与检查入口计划

日期2026-11-22；基线`767ff44`，main与本地一致、工作树干净。[用户决议](PHASE0_EXIT_DECISION.md)已批准限定退出并仅授权本任务。栈和锁定版本保持Tauri v2/Rust/Vue3/TypeScript/Tailwind，不新建workspace、不引入依赖。

## 范围与完成条件

| 工作 | 实施与验证 |
| --- | --- |
| 记录批准/状态 | 新增退出决议；当前README/TASKS/开发指引同步7/7与P1-01状态；R06接受但OPEN，旧报告/ADR/证据不回写。 |
| 固化构建隔离 | 新增8种现有desktop构建配置清单；检查实际Cargo metadata解析出的feature/可选依赖，默认不引入实验网络，R06需要合成feature。生成型负例验证误启默认实验、串入模式、丢失合成限制时拒绝。 |
| 统一质量入口 | 根`verify:p1`调用新P1脚本；复用现有P007B离线41项检查，补默认cargo check/test/实际构建和TURN feature检查；新证据仅写artifacts/p101，不覆写旧轮。锁定/离线运行，无可用私有配置/采集参数。 |
| IPC/状态合同 | 记录现有get_runtime_info、LanStatus、独立终态与实验feature/handler边界；拟议产品状态/单会话所有者/错误与统计约束为下一轮设计合同，不新建P1-04命令/Pinia或改变媒体协议。明确R06不能被映射成成功/绕过。 |
| 交付 | 适用fmt/type/lint/clippy/单元/集成/Edge预览和原生编译通过；保存各退出码、构建清单、脱敏报告。没有新媒体/云/桌面实测，不将旧测试数量当新运行。GitHub正常提交后停止。 |

P1-02导航、设备卡片、托盘、正式身份、实际输入、三件套运行集成不在本轮。C0当前可实施范围为现有WebRTC后端的工程基础；原生Foundation/V+互操作及多进程例外未获批准，不能预先添加兼容运行能力。

## 影响面与安全自审

修改集中于root package.json、scripts/quality构建清单/检查器及Node负例、P1检查包装器、当前状态和新契约/报告。代表风格沿用现有Node ESM/2空格/单引号/Prettier、ASCII PowerShell包装器；不更改Rust/Vue媒体代码、Cargo/pnpm依赖锁、Tauri权限或原生handler。无数据库/权限/协议迁移，图谱不可用时以rg/read实码复核；不安装hooks或第三方工具。

新检查器只调用固定的cargo metadata --locked --offline读取解析结果，路径/凭据原文不输出到摘要。清单不能启用实际网络；metadata通过与运行时可连接是不同证据。编译检查继续验证默认/loopback/LAN/control/TURN/direct/diag现有组合，旧授权及严格SDP/指纹/候选边界完全沿用。

验证采用Node单元与真实Cargo解析/构建/旧安全回归集成路径，并自审新脚本是否有私有配置、采集或云调用；不通过模拟Cargo返回值宣称真实构建成功。批次边界更新handoff；新产物失败先修检查器/脚本，不改历史证据或放宽门槛。可用普通后续提交撤销本轮工程入口，无破坏性清理/重写历史。
