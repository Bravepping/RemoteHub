# P006b 实验契约

接入：wss://批准IPv4:8443/probe，应用专用CA验证IP SAN/有效期；Authorization Bearer角色邀请（非TURN密钥），Origin为固定remotehub-p006b。服务配置期限≤2小时，默认不暴露普通HTTP认证/设备查询API。

Hello={version:1,role:sender|receiver,room:SHA256(pairKey),seconds:5..30}。两角色各一次原生批准后才由客户端发Hello。Sender先创建房间，Receiver通过人工配对码算出room加入；未配齐不签票。Ready={session_id,nonce,expires_at,ice_servers}，由server-only issuer签发不超过会话期限的短票，批准relay端点/范围以客户端私有配置为准，不信任服务端任意扩大范围。

DataFrame={version:1,session,direction,nonce,seq:1..256,expires_at,kind,payload,mac}。direction为sender-to-receiver或receiver-to-sender；kind仅offer/answer/beat/metrics/stop。MAC是HMAC-SHA256(key,UTF8(serde_json紧凑数组["RemoteHub.Relay.v1",version,session,direction,nonce,seq,expires_at,kind,payload]))，64位hex、恒时比较。payload≤64KiB、序列化帧≤96KiB、10条/秒/方向。错误关闭；旧LAN协议不能混用。offer/answer传完整SDP并执行P006a relay SDP策略；metrics仅允许既有脱敏统计字段，控制事件不在此通道。

等待配对最多120秒；Ready期限180秒（TURN剩余TTL60..600秒，实际≤会话）；媒体5..30秒，协商独立有界，运行时信令租约4秒。断线/关窗/stop撤销本地会话并回收媒体、SCTP、PC、WSS；不自动恢复。邀请、TURN短票和配对密钥不写报告。

两端独立验证selected pair为relay/relay、UDP、批准IP/端口范围；未选中为null，路径不符停止；控制批次仅在已验证路径且真实视频播放后开始。实际控制协议仍control.v1，无Windows输入API。默认构建无新入口；仅p006-relay实验版启用该实验。
