# P007B R06 定向诊断契约 v1

本轮只离线准备和本地合成验证；不启动云WSS/TURN、不采集桌面、不自动运行两台物理电脑。未来定向验证须新授权。

专用构建：`custom-protocol,p007b-diag`，仅DirectPreferred、每轮合成视频8秒，双方仍本次勾选和原生确认。现有HMAC/配对/期限、真实selected pair精确匹配和白名单不变。不会自动选旧桌面程序作备用，不修改防火墙或网络类别。

配置：EXE旁`r06-private.json`，沿用PrivateConfig严格schema；仅批准公网IPv4/IP-SAN TLS，剩余邀请期限60..1200秒，无额外source/desktop/drop_direct/路径字段；拒绝过期/超期/不合法/目录或符号链接。包内只有`.example`无效占位符。

统计：EXE旁`artifacts/private/p007b-r06`。原生文件buildTag=`p007b-r06-v1`，`recordedAtMs`为实际本机Unix毫秒时间；原生source.source_kind应为`synthetic_nv12`。实际候选原因字段沿用direct_pair_diagnostic v1，失败侧、类型、范围、集合来源/计数及私有host发布批准布尔均脱敏，首失败保持。无pair时不编造原因，保留实际错误及空值。

接收端独立`UUID-receiver-pre-close.json`在PC关闭前读取；`UUID-receiver-final.json`保存最后可用采样与清理。capture=live/last_known/unavailable，原采样时刻不补写。原生资源真值覆盖前端；异常写盘/采样不阻止继续回收。原生窗口消息桥的实际两机效果未来需验证，本轮自动fixture和Edge不能冒充它。

`--r06-self-check`只返回构建能力JSON，不启动Tauri窗口、配置读取、网络或源；无参数才打开待确认界面，其他CLI参数拒绝。自检与SHA可以证明交付身份，不能证明已通过双机诊断。

只读工具：Verify-SHA256.ps1核对BUILD_INFO里的EXE SHA；Export-R06Summary.ps1只读当前包的固定目录，按指定/最新终态owner导出固定字段。未知字符串拒绝或归一安全标记，不导出IP、完整SDP、邀请、配对码、目录、画面或坐标。接收端缺记录明确null，不把周期样本当final。不得从导出的安全统计反推出新的放行规则。

未来目标：至多首次失败与一次成功对照，重点读拒绝side/reason和新版独立终态，不重跑完整30秒串流或旧接收端停止补测。只有新诊断可支持新现场归因；全成功只说明本次未复现。在线窗口、Allocation观察和收尾另见待授权操作方案。
