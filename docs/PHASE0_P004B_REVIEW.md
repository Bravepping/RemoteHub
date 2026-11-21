# P0-04b 变更复核

2026-11-21，自审；不是独立安全审计。

范围：hub-webrtc新增网络scope、socket过滤、短时认证信令/租约、Session显式LAN入口；Tauri实验feature/命令/原生授权/关闭处理；双角色前端、规则/构建脚本和证据。普通构建默认关闭LAN入口，原P004a构造器保持loopback。新直接依赖hmac/sha2/uuid沿用锁文件已有版本。

检查：LAN配置/本机地址绑定、单窗口来源/命令白名单、启动互斥及旧实验互斥；原生两端确认；先认证和ICE再采集；TCP来源与UDP收发目的/来源均限定对端；DTLS证书指纹不跳过验证；MAC版本/方向/nonce/序号/长度；固定文件目录和统计字段白名单；任务停止/join、超时和重复停止；迟到授权不重启；源释放与接收端指标分开；实际数据不补0/假截图。

发现并修复：真实库应答component2被初版拒绝，以及oneshot结束掩盖原生错误。真实生成应答回归先失败后通过；坏认证应答TCP测试确认原错误能返回。候选兼容只在answer+rtcp-mux、同IP/UDP/host，不放宽offer。最终verify、all-feature clippy、36 Rust/8前端/1Edge以及真实五轮双机实测通过；原始记录见测试报告。

未发现当前有界实验范围内的阻塞代码问题。边界/未验证项：未留存基线所列录屏；B本地清理文件未收取、用户确认B操作；规则脚本安装/删除未经实测；无正式身份、持久配对/ACL、TURN、DataChannel、长稳、Win11、独立渗透或RustSec审计。不能把此原型作为公网或无人值守产品发布。完整30秒轮900帧计数一致；四个提前结束轮只保存最后回传样本，不推断其最终丢帧率。

回退：关闭p004-lan feature即可恢复默认入口；旧P004a脚本独立。开发归档与Git必须排除.tools、target、node_modules、artifacts/private和任何画面/真实IP/配对码/原SDP。文件CRC/SHA与远端commit匹配另作交付核验。
