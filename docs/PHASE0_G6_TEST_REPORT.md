# G6 云端 TURN 实测报告

2026-11-21，状态：**G6通过，P0-06a infrastructure-verified（8/8子关卡）**。P006父任务仍待独立跨网络双机relay视频/control验收，Phase0正式父项仍4/7。仅Allocate/STUN不证明双机媒体验收通过，本轮未进入P006b/c或Phase1。

## 授权、基线和环境

用户本轮明确指定自己的阿里云ECS并授权继续G6，提供SSH登录信息。开工fetch确认本地/远端main均5e05c090993b209a63836ee8bad2247df8eb69c9，工作树干净；原ADR0006和P006a实现继续沿用。SSH校验本机历史登记公钥，不跳过检查；密码只通过不回显输入进入连接进程内存，没有写入文件、源码、报告、命令参数或交付包。真实目标地址、网卡/映射、原日志和短期票据不上传Git。

只读核对为Ubuntu24.04.2 LTS、时间同步，实验UDP3478及49160..49259原先空闲，coturn原未安装。目标已运行26项系统服务和2个业务容器，不能称为空机。保持既有服务，官方Ubuntu源仅新增coturn **4.6.1-1build4**，安装清单0升级/0移除；临时mask不存在的默认unit，阻止包脚本启动默认监听。安装时关于masked unit/preset的提示与预防自启相符，包安装成功。没有改阿里云安全组或Linux防火墙、没有升级其他软件。

本机Windows原有TUN网卡存在；测试显式绑定实际RFC1918物理网卡，只访问批准ECS数字IPv4的3478/UDP，不修改本机路由、代理或防火墙。共享密钥在服务器生成并留在服务器；服务端受信任实验操作者按已通过向量的REST公式生成本轮300秒票据，服务器和本机时钟偏差检查小于30秒。没有部署公开签发API，未声称Rust issuer已经作为Linux服务运行，正式WSS/设备身份仍未实现。

## 实际部署与收尾状态

新建专用`remotehub-turn-g6.service`、`/etc/remotehub-turn-g6/`配置目录和`/opt/remotehub-turn-g6-tools/`校验辅助目录。实际EIP到本机接口映射已渲染，模板通过离线策略检查和目标coturn真实加载。服务以turnserver账户运行，NoNewPrivileges=yes、ProtectSystem=strict、空CapabilityBoundingSet；运行时约5.46MB内存、7任务，均为单次快照而非容量基准。

secret env权限0600，运行配置root:服务组0640、目录0750。服务pid和临时SQLite限定在专用/run目录；目标help确认--pidfile/--userdb真实存在，单元经systemd-analyze verify并成功启动。配置继续use-auth-secret、短票、配额/带宽、拒绝危险peer、禁TCP/TLS/DTLS/CLI；原始流量日志不持久保存，journal仅两行服务启动消息且未发现未知选项/配置错误。

测试后已停止本次专用unit，专用和默认包unit均 **inactive/disabled**，临时mask移除，未启用开机自启；本机短期票据文件已移除。服务器保留安装包、受限配置和手工辅助文件，以便下一轮经授权复用。关闭前抽查relay socket已0；关闭后TURN进程UDP socket、3478及中继范围监听均0，专用/run目录已移除。既有26项服务仍运行，2个业务容器列表与前相同，无新增运行服务。

iptables INPUT/OUTPUT原始规则哈希完全相同。nft原始输出哈希因40处流量计数变化而不同；仅归一化`counter packets/bytes`数字后，完整规则结构哈希与开始前相同。不能将动态计数差异说成规则变更，也不将原始哈希误记为未变化。没有安全组控制台审计权限，本任务未调用安全组修改；UDP3478实际可达不代表中继端口双机数据路径已测。

## 真实网络结果（非fixture）

原始脱敏结果在`artifacts/p006a/g6/`：native-allocate-first.json、native-allocate.json、wire-first-observation.json、wire-results.json、server-summary.json。它们不包含地址、session UUID、票据、nonce、完整SDP或屏幕内容。

| 检查 | 实测 |
|---|---|
| Rust webrtc0.21.1第一次实际Allocate/gather | exit0，1个relay候选，gather81ms，PC资源释放true，error=null |
| 独立新票重复原生gather | exit0，1个relay候选，gather92ms，资源释放true；所用CLI与5e05c09交付二进制相同 |
| STUN Binding | 真实UDP成功响应0x0101，XOR-MAPPED-ADDRESS结构有效；本轮事务45.255ms |
| 正确短票Allocate | 真实0x0103成功，响应MESSAGE-INTEGRITY校验通过，relay地址/端口匹配批准范围；本轮事务36.418ms |
| 无凭据Allocate | 真实401挑战，无分配 |
| 错误密码 | 经真实nonce/realm挑战后签错误认证，服务器401，无分配 |
| 已过期、但公式正确签发的票据 | 请求实际到服务器，401，无分配；不是仅本地过期拦截 |
| 错realm | 真实437，且服务端原因明确为realm value is incorrect，无分配 |
| loopback/private/metadata link-local/CGNAT/multicast peer | 5种真实CreatePermission均403，响应完整性有效；只向批准TURN请求权限，未向这些目标发包 |
| 释放 | 协议客户端Refresh lifetime=0成功且验证响应完整性；native PC close成功；服务器收尾socket为0 |

第二轮协议工具 **11/11通过**，allocationsReleased=true。STUN/Allocate事务耗时来自真实本次请求，不是ICE RTT、媒体延迟或长期性能承诺。没有远端SDP、没有对等端建连，selectedPair/ICE RTT/packetsLost均null；没有传视频、模拟坐标或实际输入。

首轮11项中10项断言通过：错realm实际被437拒绝，但测试最初只预期401/441，故整体退出1，如实保留wire-first-observation。核对coturn4.6.1 `check_stun_auth`实际源码后确认：Allocate的realm不匹配使用437；其他请求使用441。修正工具仅对**该专用原因+437+未分配**判通过，泛化437仍失败；新增离线回归后再用新票重跑，11/11通过。不是放宽服务端安全策略，也未修改原生库。REST票据HMAC本身仍不绑定realm、不能单票即时撤销；此特定服务端拒绝实测不代表所有部署均有相同行为。[coturn4.6.1源码](https://github.com/coturn/coturn/blob/4.6.1/src/server/ns_turn_server.c)

## 自动测试与可复现性

新增`scripts/g6_turn_wire.py`是显式--approved-target的操作者开发工具，默认只显示参数错误、不联网；不接受共享secret。只向私有输入的固定UDP3478端点收发，限制数据包/属性数量、核对来源/事务ID/长度/CRC/消息完整性，释放分配并只打印脱敏统计。服务端签发与登录辅助是本轮临时操作，不是产品后台或运行依赖。

`python -m unittest discover -s scripts -p test_g6_turn_wire.py -v` **4项通过**：RFC5769已知MI/CRC向量、packet构建/padding/超限、篡改拒绝、realm专用错误与泛化437区别。原coturn离线Python3项一起通过，总7项；这些离线测试与上述11项真实网络检查分开计数。[RFC5389认证](https://www.rfc-editor.org/rfc/rfc5389.html)、[RFC5769向量](https://www.rfc-editor.org/rfc/rfc5769.html)

完整回归在`artifacts/p006a/g6/final-checks`和baseline保存本轮原始UTF8输出/退出码：17组全部达到预期（15个exit0、两个预期拒绝exit2/1），含pnpm verify、all-feature clippy/test、Edge、四feature独立检查、默认原生build、配置/脚本检查与CLI优化构建/帮助。54 Rust、13前端、3Edge、3配置Python通过，私有MF1项ignored未重采集。新增4项协议Python及默认拒绝另留日志。原apps/crates/锁文件与5e05c09无diff，没有扩大旧loopback/LAN、修改媒体/control或添加公网桌面入口。

部署复现及回退见deploy/coturn/README：配置示例增加专用运行目录pidfile/userdb参数；其他目标需重新核对和授权。回退只操作本次明确创建的unit/文件，默认包unit保持禁用，不卸载被其他服务依赖的包，不清空规则或修改其他业务。G6提交SHA以Git和交付BUILD_INFO为准，避免文档自引用。

## 验收结论与未测项

G1–G5/G7/G8的原准备证据继续有效，本轮G6真实云分配与拒绝补齐，P006a达到infrastructure-verified。没有双机relay视频/control、relay端口数据流、自动P2P回退、TURN TCP/TLS、长稳/并发配额/账单上限或完整安全审计；这些不计PASS。TURN测试服务当前已停止，后续测试需明确启动；不能把服务器上留有配置理解为产品已经上线。下一步先审查本轮结果，再独立安排P006b；本轮停止扩展。
