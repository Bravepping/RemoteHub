# R06 v2 启动前核对与离线准备报告

日期：2026-11-22；基线 `01bddac6ae1738d75441fafeee891068b880be9d`，准备前及提交前核对 GitHub main。范围为用户要求的**只读实际云端核对 + 最小在线执行清单**；没有新启动授权。本报告不替换旧测试结论，不关闭 R06，不批准 Phase 0 退出。

## 实际核对

13:05:17 +08:00 通过固定主机密钥的 SSH 读取服务状态、文件属性/二进制哈希、配置安全字段和一致性布尔、专用端口、旧临时文件残留及到期脚本。会话关闭；没有远端写文件、签发凭据、探测媒体或启动服务。证据见 [server-readonly.json](../artifacts/r06-preflight/server-readonly.json)。该记录是当时快照，不能保证未来授权时配置未变化。

10 个离线基线判定均通过，三个已部署观察/分析/协议辅助文件的 SHA 与本地源码一致，详见 [readiness.json](../artifacts/r06-preflight/readiness.json)。专用 TURN 和旧 WSS inactive/dead、MainPID=0、disabled；新 R06 WSS、观察器和 expiry 单元尚不存在；专用端口无监听。之前目录中所列临时配置/密钥/pcap 均未发现，新 R06 运行目录尚不存在。秘密只在远端内存比较，未把原始 IP、凭据、完整 SDP 或原始命令输出导出。

存在两项不能照搬旧方案的差异：

1. 旧 cleanup 固定上一轮目录、observer 为 5400 秒，TURN 自身 RuntimeMax 无限，不能作为本轮 20 分钟兜底。因此准备新的 scoped guard、transient 观察器及纯计划生成器；**它们目前仅在本地，未安装或在云端执行**。获授权后须先上传、核对 SHA、设置并检查独立 timer 的实际触发时刻，再启动服务。到期脚本的真实服务器执行效果未测试，不填 PASS。
2. coturn 全局配额实际为 8、单用户名为 4；不改配置。本轮使用一对会话、一对邀请、每机单窗口、逐轮清零、同 PID 观察器 cap=4 及额外活动即停止。监测有采样间隔，不能表述为全局硬配额 4。WSS 二进制的通用连接/房间上限也未改为 1；单会话通过本次凭据发放和操作顺序约束。该事实已明确写入待授权范围，不隐瞒或擅自修改配置。

选定 WSS 为 `/opt/remotehub-p006c-manual-20261122/hub-rendezvous-probe`，实测 SHA256 `946ebb86fc60e74d00b3db6107dddb0043132b4afcfc7873d0ca5b96087c2653`，与 v2 申请一致。旧 P006b 二进制不同，保持不动。不升级任何依赖或替换产品程序。

## 准备产物和安全边界

[执行清单](P007B_R06_EXECUTION_CHECKLIST.md)列出实际单元、文件权限、启动门槛、时间预算、两轮判定与回收分类。新的 `deploy/rendezvous-probe/r06-ops` 四个脚本分别负责只读采集、纯计划生成、限定服务/文件清理及既有观察器的时间/数量约束。没有改应用协议、Rust/Vue 主链路、ICE 白名单、feature gate 或产品架构，因此无需新增架构 ADR。

计划从实际开始部署记 T0，授权截止 T0+20 分钟；T0+19 分钟独立 guard 开始收尾，WSS/观察器附剩余 RuntimeMax。T0+8 分钟停止新建 Allocation，以留足实际 600 秒租约和宽限。未能取得完整释放证据时按截止停服，记录证据不足；清理回执明确 `not_inferred_by_cleanup`，不将停止整个 TURN 当作正常释放。

guard 只作用于三个具名专用服务及本轮目录内固定私密文件白名单。普通停止失败才对该具名单元请求强制终止。路径越界/链接/非 root 或权限不当即拒绝；观察器连目录都未验证时不能停服务。命令与错误的私密原文不进入报告。旧长期 TURN 密钥、原配置、业务、防火墙、安全组及系统信任根均不在变更范围。

## 本轮验证

实际命令：`python scripts/r06-preflight-checks.py`。记录以 [verification.json](../artifacts/r06-preflight/verification.json) 及三个独立测试文本为准：

| 验证 | 通过项目 | 范围 |
| --- | --- | --- |
| `python -m unittest discover -s scripts -p test_r06_preflight.py -v` | 11 | 只读命令边界、20 分钟固定截止/缩减时限、越界/过期、具名停服、卡住/查询失败仍清理、失败不冒充释放、观察器非法路径无副作用、无参数固定错误、隐私自检。全部服务调用使用测试替身。 |
| `python -m unittest discover -s scripts -p test_network_privacy.py -v` | 12 | 内存生成网络负例、源码转义/CLI 错误不回显、扫描器自检；未采用真实端点 denylist。 |
| `python -m unittest discover -s scripts -p test_p007b_delivery.py -v` | 9 | 既有 v2 交付/摘要负例，不联网、不打开原生实验窗口。 |
| 新源码语法与通用隐私检查 | 7 个 Python 文件、2 份本轮文档和脱敏证据 | 编译检查在内存进行，不启动服务；检查扫描/采集/清理脚本本身。最终文件数以 verification 为准。 |

准备中发现并修正测试替身的参数解析，以及只读脚本 Windows 无参运行提前导入 Linux `pwd` 导致回溯的问题；最终全套重新执行。修正没有更改已部署服务。`git diff --check` 在提交前执行。

本轮未改产品源码/锁文件，未重复旧 41 项完整产品构建回归；用户已接受的该轮结果仍只属于原交付。这里的 32 项是离线运维/隐私/交付测试，**不是**新的云媒体或物理双机测试。未执行公开 WSS TLS 探测、新 PeerConnection、合成传输、独立 WebView2 终态或实际 TURN 释放测试，均为 NOT_RUN。

## 客户端交付身份

继续使用已接受且无有效配置的 [R06 v2 GitHub Release](https://github.com/Bravepping/RemoteHub/releases/tag/r06-diag-v2-20261122)，[实际 ZIP](https://github.com/Bravepping/RemoteHub/releases/download/r06-diag-v2-20261122/RemoteHub-R06-Diagnostic-v2-20261122.zip)及 [SHA256SUMS](https://github.com/Bravepping/RemoteHub/releases/download/r06-diag-v2-20261122/SHA256SUMS.txt)。本轮没有重打包、改程序或声称重新下载。

```text
ZIP  6cf08d04670fab5068ac1f288daf96ab55e7d912d0e25de0e2fe6a1a1a669706
EXE  7deac839ee2e8fcc0e3ad47433f2431a0ce242bd3a297e90da0ef717effd79c0
```

保持 synthetic-only、8 秒、原生人工同意、DirectPreferred 与严格候选校验；不采集真实桌面、不执行实际输入。配对/配置须下一次授权后重新签发，旧包不能自行恢复服务。

## 结论与待办

启动前只读材料及离线清单已准备；**新云端启动授权 PENDING**。先请用户批准清单第 6 节的具名服务/观察器/guard 和 20 分钟范围，才做实际部署及启动。若授权后发现哈希、权限、单元或端口变化，停止并报告差异。无需当前立即操作两台电脑。

随后若获授权，最多两轮 8 秒，并在截止内分类释放后停服、清临时凭据、核对不自启及基线。历史失败未复现时如实写“本次未复现”，不直接宣布根因已修复；缺文件或租约等待不足均保留限制。测试后提交脱敏结果，等待 R06/Phase 0 最终审查，不扩展开发。
