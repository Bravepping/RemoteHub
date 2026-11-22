# P2-01 Ed25519/DPAPI设备身份交付报告

日期2026-11-22；基线main/origin均`d7ce2e0ffd580a9bb29a7252311c4f3e2676cfbd`，开工工作区干净。用户已批准Phase 1限定退出并只授权P2-01，见[追加决议](PHASE1_EXIT_DECISION.md)。本轮不启动阿里云/新双机/实际桌面采集/键鼠注入/第三方后端，也不进入P2-02。先写[计划](P201_PLAN.md)、[ADR0022](ADR/0022-local-ed25519-dpapi-identity.md)、[契约](P201_IDENTITY_CONTRACT.md)，保留原工程和技术栈。

## 实现与权限

新增hub-identity：Windows BCrypt系统随机生成Ed25519 seed，固定dalek2.2.0、strict验签及zeroize；不使用hazmat/legacy或自制曲线。CurrentUser DPAPI/UI_FORBIDDEN和固定用途entropy保护版本化seed/派生公钥，拒绝机器范围和明文后备。专用DACL、owner/reparse/hardlink检查、16KiB读取上限、独占锁、同步落盘及不覆盖原文件的发布约束保护存储。

普通版启动初始化LocalAppData下身份，桌面只留下公开ID/完整公钥SHA256指纹或固定错误，私钥对象及时释放。原get_runtime_info增加可选严格identity分支，普通来源继续检查；没有新命令、capability、CSP或任意签名/私钥/文件系统权限。首页/设置显示真实公开状态，未读/错误保持明确空态；Pinia代次/清旧值与固定错误不变。P0实验构建（含纯TURN探测）不初始化生产身份，媒体/ICE/原生同意与托盘清理源码不改。

短RH标识仅公钥摘要的可读截断，不作认证/唯一性凭据。固定用途local-proof只用于本地自检，不是登录/会话签名协议；无云身份服务、信任库或输入功能。见[开发与运行说明](P201_IDENTITY_GUIDE.md)。

## 丢失/替换边界的真实修复

自审补到“单独删除已确认密钥文件会被当新建而换号”的边界。先加安全负例，正常Windows环境真实执行得到0通过/1失败，见[修复前日志](../artifacts/p201/marker-before.txt)。没有隐藏这个失败。

随后在独占锁内增加RHCOM001+完整公钥摘要提交标记，校验持久化密文可恢复后刷新标记，再首次返回身份。标记确认后缺失密文返回INCOMPLETE；另一份能被同用户DPAPI解开的身份也须匹配标记，否则INVALID。坏长度/版本标记拒绝且不覆盖；首次保存中断的空锁只允许恢复已有同一合法密文。新增丢失/合法替换负例、坏标记和空锁恢复断言均通过。整体删除/合法旧状态整体回滚仍不在本轮保证内。

## 本轮执行证据

| 检查 | 最终结果 | 实际范围 |
| --- | --- | --- |
| 完整`pnpm verify:p201` | 47主检查全部符合预期 | 原有安全负例预期非零仍为正确拒绝，见[汇总](../artifacts/p201/quality-results.json) |
| Rust全部feature | 95通过/1私有MF ignored | 包含13项身份测试；不执行未批准的真实桌面样本 |
| Rust默认 | 77通过/1私有MF ignored | 包含8桌面/来源/生命周期测试及13核心；与上组重叠不相加 |
| 身份核心 | 13通过 | RFC8032已公开向量、签名/错nonce/错/弱公钥/篡改、真实DPAPI往返/错entropy/损坏、密文无seed/公钥原文、稳定重开/并发、私有目录/硬链接、锁/pending、文件/提交标记损坏、丢失/合法密文替换拒绝 |
| 独立Windows进程 | prepare/restore均exit0 | 实际生成临时身份、第二进程恢复同一公钥并验证签名，密文不变；[原始结构化结果](../artifacts/p201/native-restart.json)，不靠模拟DPAPI |
| 前端单元/集成 | 100通过 | 新增严格身份字段/一致性/敏感额外字段拒绝、冻结、加载隐藏旧身份/固定失败信息；原Pinia生命周期保持 |
| 真实Edge | 20通过 | 既有18项及2项显式SDK身份替身；替身醒目标注，不冒充原生身份页面验收 |
| 构建/格式/类型/clippy | 适用组合通过 | 默认Windows EXE、8实际Cargo配置/9隔离测试、12 Python生命周期 |
| 权限与隐私/交付 | 7权限、12隐私、9交付通过 | 有效允许命令仍114→80/新增0，JS托盘/菜单0；[权限](../artifacts/p201/permissions.json)、[附加回归](../artifacts/p201/privacy/results.json) |

身份最终clippy/单元/示例构建记录见[identity-final-native](../artifacts/p201/identity-final-native.txt)；其中13核心与完整workspace统计重叠。两个隔离进程的真实PID/时间在native-restart记录中，未打印实际身份、公钥、私钥或密文。本轮只使用临时测试身份，没有创建/读取生产RemoteHub identity-v1目录或启动身份GUI；测试目录经绝对路径边界核对后删除。

## 依赖与许可证

Cargo新增hub-identity及11个注册表包，所有原锁定包版本保持；pnpm锁不变。ed25519-dalek2.2.0/curve4.1.3满足已查通告修复门槛，解析特性未启hazmat/legacy/serde/PKCS8导出。新增包版本/许可证/来源、LICENSE文件SHA/注册表checksum和OSV批量查询记录见[依赖证据](../artifacts/p201/dependencies.json)。该11个精确版本本轮查询匹配0通告，只是增量查询，不是全项目或独立密码审计。THIRD_PARTY_NOTICES同步；部分新锁项是未启用可选依赖，正式分发完整SBOM/许可证附件仍需按最终二进制整理。

## 初次失败与可观察性

- 受限执行身份下，初轮clippy通过，但5项真实DPAPI路径返回Protect失败，其他5项通过；初始完整工具输出保留在对话，仓库保存[脱敏摘要](../artifacts/p201/identity-initial.json)，不冒称它是原始stdout。正常Windows用户环境同源码10项全部通过，之后扩展至最终13项。执行上下文/加密profile差异未进一步定位，生产不要求管理员后备或关闭DPAPI，也不提供明文后备。4个已知失败测试临时目录已按解析绝对路径/明确名称核对清理。
- 初次依赖获取遇到当前受限环境TLS凭据错误；正常用户环境获取成功，未关闭TLS验证。正常用户回归首次被Git目录所有者检查阻止，测试尚未启动；仅给本轮子进程追加已知仓库信任，不改全局Git配置、文件owner或系统策略，不公开SID。
- 收尾原生工具格式检查曾失败，原差异保留[format-initial](../artifacts/p201/identity-final-format-initial.txt)，格式化后通过。继承脚本的Node固定命令调用弃用提示保留，不当作程序网络或原生功能通过/失败。

## 限制及收尾

DPAPI不防同Windows用户恶意程序、管理员/内存读取，zeroize不等于内存防护保证。没有执行独立账户/机器迁移、真实junction创建、原生身份页面手工查看、注册/撤销、P2网络认证；错entropy和目录/硬链接测试不冒充这些结果。完整合法状态回滚/全部删除后重建需要后续注册与撤销约束，不宣称已解决。

旧P1报告及原探针失败、P0媒体/网络证据均未改；Phase 1批准只追加记录。R06继续OPEN_NOT_REPRODUCED/根因未知，C0 D1–D6/C1继续待决，自研WebRTC主线不替换。当前无新增云资源或临时云凭据，无第三方后台软件/系统配置/路由/驱动变更。

`python scripts/p201-evidence.py`检查计数、实际DPAPI/重开、修复前后证据、锁/权限/历史及隐私/链接/程序SHA，结果见[verification](../artifacts/p201/verification.json)。它仅做离线审计，不启动客户端或网络。P2-01源码/本地自动验证交付后提交GitHub等待审查，不进入P2-02。
