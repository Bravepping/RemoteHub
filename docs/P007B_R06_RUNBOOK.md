# R06 最小诊断包：开始这里（待激活，尚未授权在线测试）

本包只生成720p30运动图案，最长8秒，不采集桌面、摄像头或麦克风，不执行鼠标键盘输入。用途是检查ICE候选被拒绝的具体原因，以及接收端关闭前/清理后的独立最终统计；不重复完整30秒串流测试。

现在无需操作两台电脑。包内没有有效服务器邀请，双击只打开待确认界面；缺少r06-private.json时开始会拒绝，不会复用旧p006c-private.json。不要自行将示例改成有效配置或恢复旧服务器。本轮阿里云WSS/TURN未启动，未来最多20分钟定向验证须用户单独授权。

## 先核对包和程序

下载/拷贝后在解压目录运行：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\Verify-SHA256.ps1
```

应显示`SHA256 OK`，与BUILD_INFO.json和GitHub交付记录的programSha256一致。也可手动`Get-FileHash .\RemoteHub-R06.exe -Algorithm SHA256`；ZIP的SHA在仓库artifacts/p007b/delivery.json。脚本只核对程序，不联网、不修改执行策略持久配置。程序未签名，SHA只能核对是否与本次交付一致，不能替代发行签名。

## 另行获授权后才执行的最小步骤

1. 操作者先在授权窗口内临时启动既有专用WSS/TURN，设置到期停服兜底，生成新的最多20分钟邀请与本轮公有CA配置；不改业务/coturn配置/防火墙/安全组或自启。确认两个程序SHA相同。新r06-private.json分别放在两台EXE旁；不要发送配置、配对码或原始日志到聊天。
2. 优先使用原同局域网拓扑和物理网卡。两台打开本包，A选“发送合成图案”、B选“接收合成图案”，本机网卡选实际有线IPv4。双方“允许直连的对端局域网IPv4”填写彼此当前地址，只在两台输入，不发到仓库。没有权限/配置或不确定当前地址时停止，不猜测、不扩大白名单。
3. 每轮双方亲自勾选本次同意、开始并确认原生提示。A产生一次性配对码，只输入B。若候选被拒绝，等待状态失败及资源回收，不反复点击重试；若成功，看到合成图案后可立即停止（无须100ACK或8秒满帧），也可让8秒自动结束以读独立终态。控制消息仅为有界背景连通性统计，少于100不自动判本次诊断失败。
4. 等两端显示已回收。B应在`artifacts/private/p007b-r06`有`UUID-receiver-pre-close.json`与`UUID-receiver-final.json`；A对应native-final/final保存`source.direct_pair_diagnostic`。不手动修改任何JSON、不给丢失文件填0或true。无pair、提前失败可能只有错误和空字段，按证据不足记录。
5. 每台运行`powershell -NoProfile -ExecutionPolicy Bypass -File .\Export-R06Summary.ps1`。工具选择最新已结束会话，输出本目录`R06-summary-时间.json`；多轮时先拷出每轮摘要，或用`-SessionId 本机文件的UUID`指定（不发UUID到公开仓库）。只交这份白名单摘要，原始统计本机保留。未知类型/不合法字段会拒绝导出；工具报错时只报告固定错误码，不发送原文件。

默认只做一次失败和一次成功对照、最多两轮，不强求故障复现；若首轮成功，只能说“本次未复现”。若首轮失败且新诊断已足够解释，先收证，不现场放宽候选或切换relay-only掩盖问题。是否需要第二轮及唯一变化由操作者按批准方案决定。与原始故障不同的网络/权限设置不可宣称同条件复现。

## 看什么、不能推断什么

发送端摘要的source.directPairDiagnostic给出local/remote的reason、候选类型/协议/地址类别、集合大小和匹配计数、私有host发布批准布尔。matched/approved只表示该次严格路径通过；port_changed/address_changed表示相对于已批准集合的差异，不证明历史网络变化。candidate_type_absent_from_sdp不是自动授权未发布host的依据。

接收端receiverPreClose/receiverFinal分别核对实际sampledAtMs、recordedAtMs、capture、视频计数/ICE/模拟控制、cleanup。live是当次读取；last_known保留旧采样时间；unavailable和null是不可得，不表示0或已回收。原生回收与浏览器peerClosed分别核对。重试成功不关闭旧历史根因；短轮帧数不要求240一致，只有完整8秒本地自动用例才有完整帧检查。

本轮已验证代码/本机TLS与Edge合成传输，尚未执行这份新包的物理双机、嵌入WebView2关闭链路或新云端实验；不能在验收单上预填PASS。不要求重复旧P006b接收端主动停止补测。

## 收尾

操作者先观察各Allocation的Refresh(0)回应及实际socket消失，必要时等实际租约到期，禁止靠停整个TURN伪造单会话回收；然后在授权窗口内停止专用服务、取消timer，确认不自启和端口/进程清理。20分钟不足就按截止停止并报告回收证据不完整，不延长、不补写成功。

两机删r06-private.json及后续任何带有效配置的拷贝/ZIP，保留无凭据程序、SHA和统计；操作者删除云端/本机本轮邀请/临时私钥。后续每次联网都需新授权和新配置。本轮到此只完成准备，不进入Phase1。
