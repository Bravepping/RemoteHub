# P1-01 桌面基础、IPC 与状态合同

日期2026-11-22；本轮只整理现有接口/检查边界，不新增产品命令、Pinia store、页面或后端插件。下列“现有”字段来自本轮基线源码；“后续约束”是P1-02/04等任务的实施合同，不表示已实现。

## 构建与实验隔离

构建配置清单为 `scripts/quality/desktop-builds.json`，真实Cargo解析核对由 `build-matrix.mjs`执行：

| 配置 | 显式顶层feature | 实际边界 |
| --- | --- | --- |
| default | 无 | 现有本机能力与有限采集实验；默认无远程访问，启动不采集。采集命令仍需每次用户同意；本轮不调用。 |
| loopback | p004-probe | 现有本机loopback视频授权和严格本机候选范围，不继承LAN能力。 |
| lan | p004-lan | 包含loopback编译能力及指定对端LAN实验，不把编译依赖当远端授权。 |
| control | p005-control | 包含LAN与模拟control.v1；不提供实际输入API。 |
| turn-probe | p006-turn-probe | TURN探针可编译；有效目标/授权缺失仍拒绝。不是默认远程访问。 |
| relay | p006-relay | 临时认证WSS/relay实验的命令分支；不注册旧采集或loopback/LAN handler。 |
| direct | p006-direct | 包含认证relay基础与严格DirectPreferred策略；不自动启动云服务或放宽selected pair校验。 |
| r06-diagnostic | p007b-diag | 原R06专用分支，固定synthetic/8秒/原生同意；不得开放真实桌面或丢包注入CLI。 |

`custom-protocol`是生产资源加载所需的构建修饰feature，不开启实验权限；本轮另实际编译默认、relay、direct、diag的相应组合。`--all-features`仅供安全回归，包含诊断分支，**不能当成普通产品发行构建**。

清单核对实际Cargo resolve，不执行应用，也不把metadata当运行时权限验收。当前Tauri capability文件包含所有已声明实验permission名称的集合，实际可调用handler仍由原生cfg分支决定；权限名存在不表示该命令已注册。P1不得新增任意shell/fs/network插件权限来替代明确IPC。

## 现有只读能力与实验接口

- `get_runtime_info`返回app_version、os、arch、capture_backend、remote_access_enabled以及p004/p004_lan/p005/p006/p006c/p007b_diag enabled标记；这些是编译/实验能力，不是对端在线状态、用户本次授权或网络可达证明。
- `run_capture_probe`仅在非relay分支注册，保留本次consent及互斥限制；UI不能因显示按钮或编译通过视为批准。后续产品能力检测不得隐式调用采集。
- `p004_*`、`p004_lan_*`和`p006_*`保持原来的原生来源/授权/会话/停止检查；主窗口标签与实验origin约束不变。产品阶段不能把实验短票当正式持久设备身份。
- 新入口只运行cargo/pnpm与已有无目标/无同意负例，不调用上述有效开始命令；不读取r06-private.json、旧邀请或本机真实桌面样本。

## 状态、统计和 R06 的后续映射要求

现有 `LanStatus`包含role/state/elapsed_ms/authenticated/messages_received/source/receiver_samples/resources_released/error。信令认证、选路批准、视频/控制数据、停止/回收分别保留，不能仅用GUI进度、收到一个消息或看到非null source判定正在播放。

独立receiver-pre-close与receiver-final保留stage、capture（live/last_known/unavailable）、sampledAtMs、recordedAtMs、snapshot、cleanup和errors。snapshot在关闭PC前读取，final的cleanup在实际关闭后记录：connected样本与peerClosed=true可以同时存在但属于不同步骤。last_known不能改新时间，unavailable/null不能改0或true。旧末周期893/880不能重命名最终900。

后续产品状态建议分idle、等待批准、协商、播放、停止中、已停止、失败；本轮不定义已生效的新IPC schema。Rust持有授权/Session/资源所有权，前端使用本次会话标识及代次关联事件；停止后迟到结果不能复活旧会话。单会话上限/幂等停止将在相应实现任务里验证，不声称检查脚本已经实现它。

R06的DIRECT_SELECTED_PAIR_NOT_APPROVED及分侧reason必须保留为真实失败；不得因用户接受剩余风险就把它映射为connected、默认无限重试或静默禁用直连。仅对已认证、未过期且精确批准的候选允许正常回退；不接受别名端点/同IP不同端口，不扩大UDP白名单。本轮不改指纹/SDP/m-line/候选类型/范围限制。

## 兼容后端与本轮验收

C0本轮可实施内容仅现有WebRTC后端的工程基础；Foundation/V+ PC/EasyTier运行接口、来源/许可、原生互操作方向及多进程/驱动例外仍未确定/授权。接口占位和编译成功不得显示“已兼容”或自动运行三件套。

本合同以构建清单负例、实际Cargo解析、默认工程check/build、旧安全回归集成验证。无新真实媒体/云/双机结果；P1-01结束等待审查，不自动进入P1-02或P1-04。
