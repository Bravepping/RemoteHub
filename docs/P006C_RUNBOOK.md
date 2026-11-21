# P006c 自动路径探针复现

这是默认关闭的开发探针；两个PeerConnection同一台Windows机器，不是新的双机串流包或产品入口。无采集、视频轨、键鼠输入。原P006b强制TURN窗口不改变。

先准备仓库现有工具链，再编译与离线检查：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p006c-checks.ps1
```

仅本机直连和两路全断的真实原生ICE/SCTP检查，不访问云端：

```powershell
# PRIVATE_INTERFACE_IPV4替换为本机实际网卡的RFC1918 IPv4；脚本核验该地址确在本机。
node scripts/p006c-local.mjs --local PRIVATE_INTERFACE_IPV4
```

脚本生成短时私有输入，仅本地指定IP互通，结束删除输入；脱敏统计保存在artifacts/p006c/local。全断场景CLI退出1是预期网络失败，runner必须同时验证无selected pair、0发送、真实丢弃计数及close成功，不能仅凭非零退出码认通过。

真实云测试须重新获得具体启动范围授权、签发新的短票，不可复用本轮已清理的配置。每例分别使用两个不同session UUID及对应短票：

```text
{
  "operator_approved": true,
  "local": "<exact local private IPv4>",
  "approval_expires_at": <Unix seconds, now + 550>,
  "turn": {
    "endpoint": "<approved public IPv4>:3478",
    "relay": "<approved public IPv4>",
    "port_min": 49160, "port_max": 49259,
    "a": { "mode": "direct_preferred", "session_id": "<uuid-a>", "expires_at": <now+540>, "ice_servers": ["<existing IceServer contract>"] },
    "b": { "mode": "direct_preferred", "session_id": "<uuid-b>", "expires_at": <now+540>, "ice_servers": ["<existing IceServer contract>"] }
  }
}
```

此为不可执行的形状说明。真实IceServer只接受精确turn:IPv4:port?transport=udp、expiry:session用户名及28字符短票；共享密钥不能下发。CLI校验文件大小、明确批准、TTL、网卡和端点，默认执行拒绝联网。

```powershell
node scripts/p006c-case.mjs --approved-target PRIVATE_JSON --case direct
# 观察两份Allocation确已释放后，签发新会话配置，再分别运行：
# --case fallback
# --case turn-unavailable
# --case all-unavailable
```

以scripts/turn_lifecycle_observer.py及turn_lifecycle_analyze.py关联每份云端分配，实际服务PID持续不变，分例检查端口归零再继续，最大2分配。受阻两例不应有成功Allocate。服务最终按授权停止且不自启；不改防火墙制造故障。操作范围及证据限制见P006C_CLOUD_OPERATION_PLAN.md与PHASE0_P006C_TEST_REPORT.md。
