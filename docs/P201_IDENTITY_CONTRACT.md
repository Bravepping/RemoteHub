# P2-01本地身份契约v1

提交标记补充：identity.lock不仅作跨进程互斥，还保存RHCOM001（8字节）及已确认公钥的SHA256（32字节），总40字节。空锁仅用于尚未提交的新建或中断恢复；持有有效标记时缺失identity.dpapi必须拒绝，已有密文解开后的公钥必须匹配该标记。首次成功对外返回前写入并sync标记，故单独丢失密钥或意外替换另一份合法密文不能静默换号。标记损坏/未知长度不自动修复。删除/回滚密文和标记全部状态仍不在防护保证内。

普通Windows启动后，由Rust创建/恢复当前Windows用户的长期Ed25519身份。目录为KnownFolder LocalAppData/RemoteHub/identity-v1，路径不由前端输入；非Windows拒绝持久化。原实验构建不初始化。

磁盘：identity.dpapi包含8字节RHDPA001头、little-endian u32密文长度与DPAPI密文，总长度上限16KiB。密文内为RHIDKEY1+32字节seed+32字节派生公钥；CurrentUser/UI_FORBIDDEN与固定用途entropy，解开后严格长度/头/公钥一致性验证。私钥/明文/密文不写日志或IPC。存在文件错误不自动重建。独占identity.lock负责跨进程串行；临时文件带本轮随机后缀、仅密文，残留返回不可用；发布不覆盖目标。

Rust Identity不提供私钥导出/序列化。local-proof仅签固定RemoteHub.identity.local-proof.v1用途和32字节nonce，并strict验签，非会话/登录/SDP协议，不开放Tauri IPC。

get_runtime_info可选identity：ready分支严格为status、device_id、fingerprint；unavailable分支严格为status、error_code。ready的RH-XXXX-XXXX-XXXX为公钥SHA256前12个大写hex字符分组，fingerprint为完整64个小写hex字符。后续认证不可只看短ID。unavailable仅固定IDENTITY_*码；无文件路径/底层异常/密钥或公开设备的自动日志。没有identity字段表示尚未接入/实验/旧响应，不伪填成功。

本地公开信息不代表服务器在线、配对完成或远控授权。无重置/旋转/导入/删除IPC、无文件系统/签名权限新增；远程访问仍false。TS拒绝额外字段、错型/错形状/错误码和混合分支；新读取清旧数据、取消/代次不变。
