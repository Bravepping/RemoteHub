# P006a 实施计划

1. 已核对fetch/main079275d、干净工作树、AGENTS/TASKS/设计与P005证据、ADR4/5/control契约。先写并提交ADR0006与新ICE/rendezvous契约。锁定0.21.1源码证明UDP-only和实际API，未知云资料不推断权限。
2. server-only TURN issuer纯模块：已知HMAC/Base64向量、时间/TTL/realm/会话审批/撤销与重放守卫、无秘密Debug。出口：单元测试和依赖图证明desktop不链接共享密钥模块。
3. 独立ICE策略/RelayOnly原生配置与socket边界、SDP约束和脱敏真实pair诊断；默认关闭p006-turn-probe，无采集/网络IPC。TS纯配置工厂实际relay属性。出口：协议/候选/未测null/旧入口不放宽，真实API编译测试。
4. coturn模板、env占位、校验脚本/渲染、Linux包/systemd手工部署/回退。出口：配置正负例、bash语法、Python静态校验/本机只读环境，不登录云、不装包、不改安全组。
5. 完整fmt/clippy/typecheck/build/test及旧feature检查，审查/README/TASKS/报告和G1–G8矩阵。G6真实云Allocate/STUN与负例为blocked，其余通过仅implementation-ready。Git上传/源码配置测试交付包核验后停止。

影响面：新server-only crate、hub-webrtc新ICE/probe模块与共享SDP校验的明确policy入口；Tauri独立feature声明，无UI公网页面；TS工厂/测试；deploy/coturn、scripts/docs/artifacts。旧NetworkScope/PinnedRuntime/媒体/控制消息语义不变，无DB、无实际输入、无云变更。依赖只提升锁定已有版本；新增直接许可记录，不声称完整RustSec。风格沿用Rust fmt+strictTS/Prettier。A1–A10覆盖如上；B2/3/5/6/9/10命中，DB/破坏迁移N/A。输入与手动测试同样受限，无管理员豁免。每波记handoff；恢复为关闭新feature，Git基线保留。

收口：步骤1–5离线实现/验证/文档完成，17组检查达预期、54 Rust/13前端/3Edge/3Python通过；source-audit覆盖所有保护路径与依赖隔离。G1–5/G7/G8 implementation-ready，G6真实云功能blocked，不属于本轮未经授权可执行内容。完整SELF-AUDIT范围/对称RX-TX/期限-撤销/协议形态/调用方与未知输入负例、源码引用/日志/依赖复核见测试报告与原始输出；无破坏迁移、无云部署、无新增UI/HTTP入口。交付Git与归档完成后停止等待审查。
