# P0-05 执行计划

执行结果：步骤1–6的代码与真实证据已完成，双机两轮视频+100事件和四种清理通过；最终Git/归档标识见交付BUILD_INFO。P005完成后停止，不进入P006/P1。

1. 已fetch核对origin/main=b914942，工作树干净；读取AGENTS/TASKS/设计及P004b报告。先写ADR0005/CONTROL_V1契约并提交，不改变现有成果。
2. 纯协议与权限/seq/rate/size测试，SDP新增显式video+application策略。出口：旧video-only/P004a继续拒绝DataChannel，P005负例全拒绝。
3. Session复用同一PC增加受控DataChannel事件任务；LanSession显式P005认证信封/权限上下文，Tauri原生确认+feature及统计。出口：真实本机SCTP集成正常100条及非法/重放/越权/超限/停止验证，不采集桌面。
4. Vue复用LAN视频连接和UI，创建control.v1、100事件面板、ACK/pong/异常/关闭、终态统计。出口：前端事件/协议/授权迟到回归、类型/构建、原视频回归通过，生成可复用原路径的更新包。
5. 两台Windows用户亲自确认：30秒视频+至少100模拟事件，A/B计数一致；主动断开、关窗、拔线与真实错误。出口：真实DataChannel统计与清理记录；未执行不PASS，不保存画面/敏感字段，不动安全配置。
6. 全verify/all-feature clippy/test、自审、README/TASKS/报告/记忆、Git上传与源码/二进制归档CRC/SHA核验；到P005停止，不进入P006/P1。

影响面：hub-webrtc control/signaling/session/lan；Tauri LAN commands/config/main与feature；Vue LAN控制器/面板；脚本与docs/artifacts。无数据库、无新依赖/锁文件升级。现有Rust fmt、Vue3 TS strict+Prettier风格；恢复点为本基线提交和默认关闭新feature。网络/并发/输入安全均走单轮有界任务、无管理员豁免、源关闭后join再报释放。A1–A10有上述映射；B3/B4/B5/B6/B9/B10命中，DB/破坏迁移N/A；无性能提升声明。每波/约5文件更新handoff与daily。失败保存错误及位置，先复现再修；原生隐私确认由用户亲自操作。
