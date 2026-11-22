# AGENTS.md — RemoteHub V2 / Clean Core

你是 RemoteHub V2 的开发 Agent。请用中文交流；Rust/TypeScript 标识符、协议字段和提交信息可用英文。

## 唯一目标与工作范围

**尽快交付两台 Windows PC 上实际可用的远控软件**：在被控端明确批准后，控制端看到实时桌面，通过鼠标/键盘操作，并能随时安全断开；随后完善公网直连/TURN 和 Windows 安装器。

- **只在 `v2/clean-core` 开发**，不要提交到旧 `main` 或 `v2/fast-track`。先确认当前 Git 分支、工作区、HEAD 和锁文件。
- 本分支从 `v2/fast-track@4385454` 提取**全部 `apps/desktop` 与 `crates` 核心源码**及必要工具链；历史 `artifacts/`、旧方案和阶段报告未复制。本分支历史仍可追溯原版提交；它不是从零重写。
- 技术栈固定：Windows + Tauri v2 / Vue 3 / TypeScript / Pinia / Rust，采集 DXGI，编码 Media Foundation H.264，媒体 webrtc-rs/WebView2，输入可靠 DataChannel + Win32 SendInput；服务端 Axum WebSocket，后续 STUN/TURN。
- **Sunshine、Moonlight V+ PC、EasyTier 都不是 V2 必需依赖**。暂不引入多余服务/微服务、数据库、用户系统或第三方播放器。

## 当前任务：产品会话集成

- **当前验收项是两台 Windows 的「已验证设备（仅视频）」会话**。使用 `v2-verified-lan` 构建，两端分别核验对方完整 SHA-256 指纹、明确选择同一模式、提供当前临时配对码并作原生批准；验证画面与自然结束/手动停止。不经身份核验、签名不符、模式不同的连接必须结束，不能自动退回普通模式。该模式不启用键鼠，不把双机测试结果预先标为通过。
- 先保留及回归已经现场通过的普通 LAN TCP 模式；不要重写现有 DXGI/MF H.264、WebView2、输入与结束链路。设备配对不授予远控权限。本机配对记录不持久化；启动新会话时只读重载已提交的本机身份。
- Axum 的 WS/WSS 和签名 offer/answer 已有回环测试；**跨机 WSS 尚未接入产品**。下一步接入真实产品信令时必须同时验证双方设备、公钥/证书、原生同意、SDP 和实际候选路径。没有可信测试证据前，不开放公网、不启用 TURN，不把回环测试当作双机通过。

## 已有事实：禁止重复造轮子

- `apps/desktop/src/features/desktop/RemoteWorkspace.vue` 与 `apps/desktop/src-tauri/src/p004_lan.rs`：有界 LAN 会话、原生同意、停止和报告，沿用既有 IPC，不增加宽泛权限。普通远控以 `p3-lan` 启用，身份核验视频以 `v2-verified-lan` 显式启用。
- `crates/hub-capture-win` / `hub-encode-win`：DXGI + MF H.264。`hub-webrtc` 已有 LAN/ICE/PeerConnection、选中候选诊断、DataChannel、SendInput 与撤销清理。
- `crates/hub-identity` 有 Ed25519 和 CurrentUser DPAPI 本地身份、完整指纹核验和会话证明；这不等于云认证。`v2_signaling.rs` 已通过实际本机 Axum 测试，仍只监听回环。
- 原版 `main@d7dbc9b` 记录 853/900/900 帧和 29/81/132 条输入 ACK；这些计数是来源分支历史证据。精简分支 `77479f2` 的普通 LAN 双机视频、英文输入及结束已由用户确认正常，**P3-END-01 普通 LAN 回归已通过**。新模式、R06、DPI、公网与安装器尚未验收，实际结果详见根 `README.md`。

## 开发顺序（可连续推进）

1. **产品会话集成**：优先完成已验证设备视频双机验收，再接入产品 WSS；输入需单独的原生明确许可和撤销验收，保留普通 LAN TCP 为手动选择，不允许无提示降级。
2. **公网连接**：在设备、加密和来源授权准备齐全后，先测 STUN/ICE 直连，再测 TURN 回退。以真实 selected candidate pair 判断选路，不用模拟状态代替；限制凭据、会话和中继资源。
3. **Windows 安装交付**：可安装、可卸载、真实双机验收；再逐步扩展 1080p、更长会话和多 DPI。

完成一个可验证增量后直接进入下一个**已批准的代码开发与离线测试任务**；只在存在实际阻塞、改变产品安全边界或需要真实外部操作时询问用户。

## 敏捷与 Git 规则

- **不强制撰写 Plan / ADR / Contract / Evidence 或每项单独报告**。根 `AGENTS.md` 写当前任务和执行边界，根 `README.md` 记录实际进度、测试命令与结果、未完成项和风险。保留必要自动测试源码与简短有意义的注释。
- 每次修改尽量可编译/可回滚；按功能提交小 commit。不要强制推送、改写原仓库历史或删除用户数据。
- 若测试失败，报告精确错误及已定位范围，优先修复再推进；不得把 fixture、纯单元测试、build success 记作真实双机远控通过。
- 默认先在 Windows 环境执行：
  ```powershell
  pnpm install --frozen-lockfile
  pnpm typecheck
  pnpm test
  cargo fmt --all -- --check
  cargo test -p hub-webrtc --features v2-verified-lan --locked
  cargo test -p remotehub-desktop --features v2-verified-lan --locked
  cargo test -p hub-rendezvous-probe --bin v2_signaling --locked
  cargo build -p remotehub-desktop --profile probe --features v2-verified-lan,custom-protocol --locked
  ```
  开始前确认可用 MSVC、Windows SDK、WebView2、Rust 和 pnpm；缺依赖如实说明，不自行关闭 TLS、提权或改安全设置。真实硬件操作不属于上述自动测试。
- 不运行会实际采集/注入输入的程序来“自动测试”；先列出精确 EXE 路径/SHA256、目标电脑、动作与停止方式，请用户操作/同意，Agent 只做脱敏统计与取证。
- 保留前端 TS 严格字段校验与 Rust 权限边界。公共源代码不提交真实服务器地址、令牌、证书私钥、用户名路径、配对码、SDP、桌面图像、键盘输入内容。临时输出写到被 `.gitignore` 忽略的目录。

## 必须保留的远控安全边界

- **对端主动确认本次画面共享和本次键鼠操作**，不能仅凭 IP、固定 token 或短设备 ID 获得无人值守访问；观看与输入权限分别受控。
- 严格绑定具体会话/对端、有效期、认证信令、SDP/ICE 候选；拒绝重放、错序、越界输入、越权消息与意外网络目标。
- 输入默认关闭、用户可见；失焦/暂停/断开/退出/超时撤销控制并释放**本会话注入**的键鼠状态。被控端必须有显著停止入口，紧急停止快捷键不得移除。
- 不实现隐形控制、驻留后台采集、权限提升/UAC 绕过、凭据读取、未授权访问；不擅自安装驱动、改系统路由或防火墙。
- 用户已表示愿意配合双机验收，但**未授权 Agent 自行启动云服务器、执行真实屏幕采集、注入输入、部署公网服务或修改系统设置**。需要时提出具体、最少的人工步骤，由用户现场批准。

## 每次回答保持简短

**本轮完成：** 真实修改的文件及 commit。  
**运行验证：** 实际执行的命令、PASS/FAIL 和覆盖范围。  
**未完成 / 阻塞：** 一个明确的下一步问题或风险。  
**紧接着做：** 下一个最小可运行增量；没有外部操作/安全阻塞就继续开发。

根 README 为验收进度入口，不要求逐任务生成长文档。
