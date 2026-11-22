# P0-07-B 准备报告：证据例外决议与 R06 诊断包

日期2026-11-22（北京时间）；开工main/origin/main均为dbea0e3，工作树干净；先提交ADR0013/契约9c0b87e后实施。**只完成离线准备和本机合成验证，未启动或登录阿里云WSS/TURN、未操作两台物理机、未采集真实桌面、未进入Phase1。**

## 当前决议

用户已明确批准P004用归档统计与本人现场观看替代录屏，范围限定于本阶段已有测试。[追加决议](PHASE0_P007B_DECISION.md)记录批准原意，P004按例外done、父任务5/7；原P004/P007A等旧报告、原规范和原统计均未修改。R06未获风险豁免，历史两次拒绝未归因，正式Phase0退出继续HOLD。

## 诊断包实现

新增默认关闭的`p007b-diag`，复用Tauri/WebView2、DirectPreferred Session、认证WSS、DIAG候选/终态和现有operator synthetic NV12/MF源。原生固定8秒、仅DirectPreferred；加载函数固定`load_operator_synthetic(..., false)`，仅走已校验公网目标/真实云ICE配置，不启用本地绕过或drop_direct。JSON字段不能选择桌面源，旧原始采集handler在该feature组合不注册。运行源始终合成图案，仍需UI本次同意+原生确认+一次性配对/HMAC/路径授权，媒体/控制按原精确候选校验开放。

独立配置名r06-private.json，邀请剩余期60..1200秒；不读取旧p006c-private.json，不附有效配置。源码仍包含原项目捕获crate供其他构建使用，不能将本包“无可达桌面源入口”误称整二进制/全工程完全没有捕获代码。没有Windows输入注入。

程序界面标明合成源/8秒，保留本次勾选、双角色/物理网卡/确切私有peer输入；隐藏relay-only选择，不添加授权/白名单旁路。显示发送端分侧固定reason；原生报告新增实际Unix毫秒时间、专用buildTag及artifacts/private/p007b-r06目录。接收端独立pre-close/final与缺失回退仍由既有受限p006_save及journal处理，没有新增IPC权限。

`--r06-self-check`仅输出固定构建能力，不开窗口、不读配置、不联网；其他CLI参数拒绝。交付包附SHA核对与PowerShell白名单摘要工具，可在无Python/Node的另一台Windows运行。导出固定原因/枚举、计数/采样时刻/清理布尔；未知错误归一，隐私/超限/错误类型拒绝。缺记录保留null/last_known，不把周期样本补成final，也不由摘要直接判定旧故障已解决。

## 实际安全回归

41项命令全部达到预期，完整输出在[automatic](../artifacts/p007b/automatic/results.json)：类型/build/lint、3个Edge、fmt、默认及all-feature clippy `-D warnings`、workspace all-feature测试、旧P004a/LAN/P005/RelayOnly/DirectPreferred独立检查/构建、操作员CLI缺授权/占位目标拒绝，以及专用feature clippy/test/优化构建/自检/非法参数拒绝。82 Rust、34前端、12 Python生命周期测试通过；旧桌面3项与专用桌面3项单测另列，非新增6项重复累计；真实私有MF1项ignored，不重采集。未升级依赖或改变原UDP白名单、候选授权/TLS校验。

新增9项离线交付工具测试在[delivery-tests.txt](../artifacts/p007b/delivery-tests.txt)：真实PowerShell正向导出、未知错误脱敏、缺最终证据null、last_known原采样时刻、清理未知不写true、合法诊断分侧原因保留、私有枚举/数字字符串/旧桌面报告拒绝、程序SHA篡改拒绝、打包非诊断身份/不安全目的地拒绝。测试fixture不作为新现场数据。Rust专用两项检查模式/时长和邀请范围/过期/loopback/未知source字段；前端两项检查新鲜同意和旧普通UI兼容。原Session权限、SCTP非法消息/重放/大小/频率、SDP与scope、独立journal越权/顺序/隐私负例在完整回归中继续执行。

开发期首次fmt检查因外层工具链脚本未加载导致未先格式化，修正为既有进程级Bypass入口后重跑全部41项；不改系统持久策略。首次导出工具正向测试暴露PowerShell `Enum`保留字解析错误，改名Safe-Enum后全部正负例通过；首次负例退出当时不能算有效覆盖，见development摘要。没有由这些开发失败推断网络/产品可靠性。

## 本机真实合成传输（单独证据层）

2026-11-22北京时间12:20:35–12:21:32，沿用真实localhost TLS/WSS、两个原生角色、现有Session/MF/RTP、共享RelayProbe/Edge/SCTP，源设8秒合成NV12。未用云TURN；测试PC工厂去掉未使用的ICE服务，只在这一旧本地夹具中使用临时合成设备权限，不接触真实设备。**这不是打包Tauri的原生GUI/确认框/嵌入WebView2关窗或新物理双机实测。**

| 场景 | 实际发送帧 / 最后可得解码帧 | 统计方式 | 结果 |
|---|---|---|---|
| 8秒自动结束 | 240 / 240（接收/解码/呈现均240） | live | 真实100发送/接收/ACK/确认、路径host/host、清理通过 |
| 发送端主动停止 | 172 / 172 | live | 100ACK后提前停止，两阶段独立文件及清理通过 |
| 接收端主动停止 | 160 / 159 | live | 实际短帧计数保留，不补成相等；清理通过 |
| getStats API拒绝 | 173 / 161 | last_known | 原采样时间保持，DIAG_GETSTATS_FAILURE保留，清理通过 |
| 本地WSS服务器断开 | 160 / 160 | live | 真实终止本地TLS服务器，P006_WSS_CLOSED、原生exit1保留；预期异常处理通过 |

各轮均有实际写盘后的独立两阶段记录，pre-close时PC仍未关闭，final中控制/PC/track/native清理状态核对；sender角色/foreign owner写入真实拒绝。详细逐轮数据在local，源码/程序hash和摘要在[summary.json](../artifacts/p007b/summary.json)。统计/API故障是有界条件注入，不冒充物理GPU/网络故障；正常/异常结果分开。源与接收计数不同仅描述真实采样和停止差异，不直接定性丢帧或填假0。

临时本地TLS私钥/邀请目录和工具测试临时目录已删除；本轮没有创建云端凭据。p007b-evidence.py离线复核旧报告/旧统计与dbea0e3一致，读取原始安全及本机结果、检查轮次退出码/完成时间防残留旧PASS，并执行脱敏扫描；不调用会改写历史summary的旧工具。

## 程序、包与复现

专用EXE：17,053,696字节；SHA256：`61b6e4815e7339ca0823c6c304593368f2d2a28ba8bd1b666a0eea4169df54c5`。实际构建命令：`cargo build -p remotehub-desktop --profile probe --features custom-protocol,p007b-diag --locked --offline`。无签名/安装器或干净Windows11验收，SHA不是签名或可重复构建证明。

交付ZIP及每成员SHA、程序源码提交和CRC检查在[delivery.json](../artifacts/p007b/delivery.json)；包内BUILD_INFO对应实际程序，代码提交先于打包，最终报告/包清单提交后另行交付。打包器拒绝未提交产品源码、与验证hash不一致的程序、非专用自检身份、未通过安全/本地测试及已有输出目录；只复制明确白名单成员，无有效邀请/私钥/SSH信息。README、TASKS与[操作说明](P007B_R06_RUNBOOK.md)/[待授权20分钟方案](P007B_R06_ONLINE_PLAN.md)已同步。

复现：准备既有Rust/MSVC/Node/pnpm后运行`powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p007b-checks.ps1`（不启动云或采集）；显式本机合成测试`node scripts/p007b-local.mjs --local PRIVATE_INTERFACE_IPV4`；工具单元`python -m unittest discover -s scripts -p test_p007b_delivery.py -v`；离线证据核对`python scripts/p007b-evidence.py`。本轮关闭dev/test debug符号与增量以避免缓存膨胀，未删测试省空间。

当前：**P0-07-B prepared；新云端/物理双机/原生窗口终态NOT_RUN，R06 OPEN，Phase0退出HOLD。** 后续最多20分钟验证仍需用户单独授权；包不自动激活，不要求现在配合，不重复完整串流或旧已跳过停止补测。准备交付后停止扩展等待审查。
