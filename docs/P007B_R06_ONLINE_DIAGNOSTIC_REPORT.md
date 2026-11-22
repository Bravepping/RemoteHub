# R06 v2 最小在线双机诊断报告

日期：2026-11-22；基线 GitHub main `8d9a445`。用户在本轮明确批准最长 20 分钟、最多 4 并发 Allocation、一对会话、最多两轮 8 秒合成视频。执行依据为 [启动前清单](P007B_R06_EXECUTION_CHECKLIST.md)，未进入 Phase 1。

**结论：执行一轮真实双机短诊断，严格候选匹配通过，新版接收端独立 pre-close / final 实际落盘，两个 Allocation 均在停服前由 Refresh(0) 释放。历史候选拒绝本次未复现，旧两次具体根因仍未确定，R06 等待最终审查。** 没有进行第二轮、完整 30 秒串流或旧主动停止补测；没有真实桌面采集或实际输入。

## 范围与交付身份

两台由用户实际操作的 Windows PC，用户确认仍在同一局域网，沿用物理网卡及确切对端批准。两端每轮由用户亲自勾选并确认原生提示；本机首次启动的空目录没有统计，实际发送端运行在用户解压的本轮限时包中，随后在该目录导出，未要求重测。

使用已接受的 R06 v2 程序，EXE SHA256 `7deac839ee2e8fcc0e3ad47433f2431a0ce242bd3a297e90da0ef717effd79c0`。既有 [v2 Release](https://github.com/Bravepping/RemoteHub/releases/tag/r06-diag-v2-20261122) 未替换；本轮只生成带限时配置的私有转移 ZIP，完成后删除，未上传 GitHub。程序、产品源码、锁文件、严格 ICE 校验及授权边界未改。

服务复用既有 coturn 与已核对的 WSS：`/opt/remotehub-p006c-manual-20261122/hub-rendezvous-probe`，SHA256 `946ebb86fc60e74d00b3db6107dddb0043132b4afcfc7873d0ca5b96087c2653`。WSS 以既有非 root 账号运行，新单元均 transient；旧 P006b WSS 保持停止。全局配额仍为 8、单用户名 4，未改配置；本轮单会话、观察 cap=4，实际峰值为 2，不能将此说成服务全局硬上限已改为 4。

## 到期机制与时间线

所有下列时间为北京时间 +08:00；原始云端纳秒及 Windows 毫秒时间戳保留在证据，表格显示微秒或毫秒。没有额外时钟同步测量，不据跨电脑时间差推算网络时延。

| 时刻 | 真实动作 / 状态 |
| --- | --- |
| 13:35:10 | T0，本轮新 `/run/remotehub-p007b-r06-20261122T053510Z` 开始部署；外层截止 13:55:10。 |
| 13:35:14.153856 | 3 秒真实 transient timer 已触发 scoped cleanup 并产生回执；此时 WSS/TURN 均未启动，没有 Allocation。 |
| 13:35:15.286673 | 正式 timer active；通过 D-Bus 读取真实下一触发单调时钟，确认不晚于 13:54:10，之后才生成短期配置、启动服务。 |
| 13:35:16.469775 | 激活完成；TURN 同 PID 观察基线为 0，非 root WSS / 观察器运行；公网应用 CA/IP-SAN TLS 验证通过。 |
| 13:38:32.602375 / .725249 | 两个 Allocation 建立，服务实际授予各 600 秒租约。 |
| 13:38:41.937 / .963 | 接收端独立 pre-close、final 的真实统计采样时间。 |
| 13:38:41.967 | 接收端 final 写入时间，保留此前采样与实际清理字段。 |
| 13:38:41.709498 / 13:38:43.486090 | 两个 Allocation 分别收到 Refresh(0) 成功回应。 |
| 13:38:42.995194 / 13:38:44.522820 | 同 PID socket 采样分别确认对应分配消失。 |
| 13:39:55.305278 | 本机分类输出文件原始 mtime；分析命令先于停服执行，已保存该值及文件 SHA，非外部时间戳认证。 |
| 13:40:15.935746–13:40:16.020973 | 提前正常停止本轮 WSS/TURN/观察器并清理凭据，没有使用强制终止；随后取消 timer，复核资源。 |

从 T0 到清理完成 **306.021 秒（约 5 分 6 秒）**，没有接近 20 分钟上限。邀请截止设置为 T0+8 分钟以自动拒绝晚到的新会话；实际提前清理，未等到该时刻。正式 timer 留出最后 60 秒清理余量，运行时也设置剩余 RuntimeMax / Restart=no。

到期验证的证据边界：3 秒 timer 在**媒体服务尚未启动**时真实触发清理入口，验证了调用、权限与回执；本轮实际运行的服务随后由同一清理实现提前停止。**未执行“运行中的媒体服务一直等到第 19 分钟自然到期”的端到端场景**，不将其标成实测 PASS。见 [guard-proof.json](../artifacts/r06-online/guard-proof.json)；部署的 guard/observer SHA 与本地源码一致。

## 候选诊断与接收端独立统计

| 检查项 | 真实结果 |
| --- | --- |
| Rust 严格 selected pair | local `host / udp / private`、remote `srflx / udp / public`；两侧 reason=`matched`，exact_match_count 均为 1，approved=true。 |
| 已批准集合 | local=3，来源 validated_published_answer；remote=2，来源 authenticated_validated_offer；privateLocalPublicationApproved=true。 |
| 接收端实际路径 | srflx / srflx、direct、peerMatches=true；这是浏览器观察角度，不把两端候选类别当成必须字面镜像。 |
| 合成源 | synthetic_nv12，发送 240 帧；固定 720p30 / 8 秒合成计划。sender elapsed_ms=10802 为实验整体耗时，含协商/清理，不冒充 8 秒墙钟时长或端到端帧率。 |
| 接收端 pre-close | capture=live；240 接收 / 240 解码 / 240 呈现，1280×720，freezeCount=0，errors=[]。 |
| 接收端 final | capture=live；再次实际采样并落盘，同为 240/240/240；controlClosed、peerClosed、tracksStopped、nativeResourcesReleased 均为 true，errors=[]。 |
| 背景模拟控制 | B 发送 100、Rust 接收 100 / ACK 100、B 确认 100；pending=0、anomalies=0、rejected=0。这是短轮自然取得的背景计数，没有另做完整控制验收。 |
| 原生回收 | 两端 nativeResourcesReleased=true、error=null；用户确认看到合成画面并且两端回收。 |

独立终态的 snapshot 在关闭 PeerConnection **之前**读取，final 的清理字段在关闭后保存。因此 final snapshot 的 connectionState/iceState 仍是 connected，而 cleanup.peerClosed=true；二者属于不同步骤，不伪造关闭后的新媒体采样。pre-close 与 final 的 sampledAtMs 分别保留，未使用旧周期统计填补，也未把 null 改成 0。新证据验证的是本次自动结束路径，**不包含 OS 直接关窗、异常失败或断网下的新物理双机终态测试**。

两端白名单导出见 [sender-summary.json](../artifacts/r06-online/sender-summary.json)、[receiver-summary.json](../artifacts/r06-online/receiver-summary.json)。不公开原始会话 UUID、端点、SDP、邀请或桌面内容。

## Allocation 释放与收尾

同一 TURN PID 355468；基线 0、期间峰值 2、关闭后 0。媒体本轮走直连，TURN 分配来自真实 ICE 候选收集，不将此轮说成另测了中继媒体或自动回退。两个分配均有 Allocate 成功、实际 600 秒租约、Refresh(0) 请求及成功回应、对应 socket 消失；回应至首次观测消失分别 **1.286 秒 / 1.037 秒**。采样存在间隔，记录的是首次观测时间，不宣称精确销毁延迟。

停服前已经运行分类，结果均为 `refresh_zero_immediate`，见 [allocation-verdict-before-stop.json](../artifacts/r06-online/allocation-verdict-before-stop.json)。[停服前事件快照](../artifacts/r06-online/turn-events.jsonl)保留原数据，[最终观察文件](../artifacts/r06-online/turn-events-final.jsonl)另行只读收取，不覆盖先判定证据。[classification-record.json](../artifacts/r06-online/classification-record.json)保存原分类文件 mtime、原始字节 SHA 和 UTF-8/LF 文本 SHA 及时间证据限制，避免 Git 换行规范化破坏复算。没有等待原租约名义到期，因此该检查点仍为 null；本轮**不包含租约超时释放新用例**，也没有将停服务后的清零充当正常释放。

清理回执明确 `allocationReleaseVerdict=not_inferred_by_cleanup`。最终只读核对确认：

- TURN inactive/dead、MainPID=0、disabled；新 WSS、观察器、expiry timer/service 均 not-found/inactive，不自启。
- 专用 TCP/UDP 监听为 0；云端本轮配置、私钥、观察标签和原始控制 pcap 白名单已清理。保留非秘密脚本、公证书和脱敏回执，不保留有效邀请。
- 受保护文件、规则摘要、业务容器启动时间及运行服务集合与启动前一致；未改 coturn 配置、业务、Linux/Windows 防火墙、安全组或系统证书信任。
- 本机原目录、实际解压目录、临时配置副本及带配置 ZIP 已删除；用户明确确认第二台和其他位置的配置/带配置 ZIP 副本均删除，程序和统计保留。第二台清理依据为人工确认，未冒充远程扫描。

证据见 [cleanup.json](../artifacts/r06-online/cleanup.json)、[after-stop.json](../artifacts/r06-online/after-stop.json)、[operator-confirmation.json](../artifacts/r06-online/operator-confirmation.json)；服务端回执也单独保留。SSH 均使用固定主机密钥并关闭连接，没有在源码/报告/交付包保存 SSH 密码或长期 TURN 密钥。

## 自动回归、源码及复算

本轮只新增显式授权的激活实现及小范围清理修正：已回收或尚未创建的 transient 单元可能让 systemctl stop 返回非零，现以已确认 inactive/PID=0 判定幂等停止；查询失败仍报错并尝试清理。新增该负例，未放宽媒体/网络授权。

`python scripts/r06-online-checks.py`：**33 项离线回归通过（12 运维 + 12 隐私 + 9 交付）**，七个相关源码的语法/隐私检查通过，历史准备报告/证据未覆写。该数量不含真实双机和云端测试，不把旧 41 项产品回归重新记为本轮运行；产品源码和锁文件未变。

`python scripts/r06-online-evidence.py`：根据两端白名单摘要、guard、云端同 PID 记录及清理/人工确认复算范围、数量、时间顺序与限制；结果见 [verdict.json](../artifacts/r06-online/verdict.json)。`python scripts/r06-online-privacy.py` 对扫描器自身等11个源码、3份当前文档和全部19个其他本轮文本证据执行通用隐私检查，见 [privacy.json](../artifacts/r06-online/privacy.json)；提交前执行 git diff --check。精确命令结果以 artifacts 为准。

## R06 与 Phase 0 决议边界

本次取得了新版本真实物理双机的候选 `matched` 证据和接收端独立终态，验证了这次短会话的资源释放。**没有复现此前两次 DIRECT_SELECTED_PAIR_NOT_APPROVED，不能判定当时属于缺候选、端点变化、范围或类型差异，也不能直接宣布其根因已经修复。** 本轮未为追求复现改变量或进行第二轮；旧失败和旧独立 final 缺失不追补。

P004 录屏替代证据已批准，不重开。当前 R06=`AWAITING_REVIEW` / Phase 0 HOLD；由用户最终审查本轮结果及历史可靠性剩余风险，再决定是否满足退出条件。到此停止扩展，不进入 Phase 1。
