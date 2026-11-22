# P0-06c-DIAG 契约

本轮仅增加受限本地诊断，不改变SDP/WSS/control.v1或UDP授权语义。

候选诊断version=1：local/remote独立固定原因、选中候选类型/协议/地址类别、已批准集合大小和相等关系计数；会话作用域和有效性单独判断。数据不含原始IP、端口、完整SDP、密钥、配对码或可跨会话关联的端点摘要。失败时继续拒绝，不能用诊断计数推导新的放行规则。

ReceiverEvidence version=1：stage=pre_close|cleanup；capture=live|last_known|unavailable；sampledAtMs为实际采样时刻、recordedAtMs为本次记录时刻；snapshot是受限视频/ICE/模拟控制统计，last_known不得改采样时间，unavailable的snapshot和采样时间为空。errors为有限数量固定安全代码；cleanup分别标原生资源、控制关闭、PC关闭、track停止，不把未知写成true。

记录在原生拥有的会话目录，只接受本地接收角色、既有会话ID和严格schema，无客户端自选路径。pre-close证据在PC/track关闭前落盘；cleanup另存并包含原生资源真实状态，不修改pre-close文件。重复/越权/超限/任意额外字段和任意字符串拒绝。数据只能用于诊断，不能授予会话或输入权限。

复用p006_save的receiverEvidence可选字段；无字段时保持旧调用。异常结束仍先尝试采样与落盘，再独立完成所有回收步骤，不能因统计失败而漏关PC。关窗限时通知前端，无响应明确记录missing/unavailable。新测试不追填旧两机893/880周期样本，不将旧失败归因为未经记录的具体原因。
