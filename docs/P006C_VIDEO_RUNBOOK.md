# P006c 视频/模拟控制集成复现

## 默认关闭与本地检查

普通构建没有网络实验；旧P004a/LAN/RelayOnly脚本保持原范围。新实验版显式构建：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p006c-video.ps1 -BuildOnly
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/p006c-video-checks.ps1
```

编译所得target/probe/remotehub-desktop.exe需有效的私有p006c-private.json才能开始实验，模板沿用deploy/rendezvous-probe/client.json.example的字段。不能复制占位地址直接运行。云端重新启动/部署须另行具体授权；当前历史服务均已停止。

## 无桌面采集的本机真实测试

显式p006-session-probe构建video-session-probe和authenticated-session-probe。其默认启动拒绝网络；合成源只在这些操作员入口启用，正式Tauri命令不能选择。保持本机无其它编码实验以避免MF独占冲突。

```powershell
node scripts/p006c-video-suite.mjs --local PRIVATE_INTERFACE_IPV4
# 真实本地TLS服务器 + 两原生WSS客户端 + 现有前端RelayProbe：
node scripts/p006c-wss-video-local.mjs --local PRIVATE_INTERFACE_IPV4 --case auto
node scripts/p006c-wss-video-local.mjs --local PRIVATE_INTERFACE_IPV4 --case sender-stop
node scripts/p006c-wss-video-local.mjs --local PRIVATE_INTERFACE_IPV4 --case receiver-stop
```

PRIVATE_INTERFACE_IPV4替换为本机实际RFC1918地址。WSS runner需要开发用Python cryptography（可用REMOTEHUB_PYTHON指定Python，REMOTEHUB_PYTHON_DEPS指定已有包目录）生成临时IP-SAN127.0.0.1证书，完整验证TLS/邀请/HMAC；无云端或实际TURN服务器。fixture的8.8.8.8只是未使用的配置形状，原生侧不创建TURN grant，测试PC工厂移除ICE服务器，不能以此证明公网可用。

有VPN时Chromium可能只公布虚拟接口；仅测试浏览器进程使用强制合成设备及临时媒体权限来公布物理接口，随即停止合成流。不读取摄像头、麦克风或桌面，也不更改系统网络。实际视频来自原生MF编码的运动NV12图案，均标synthetic_nv12；视频SRTP、control.v1 SCTP和真实selected pair不模拟。测试私有TLS文件结束即删除，脱敏统计位于artifacts/p006c-video。

standalone receiver-close直接销毁Edge页面，对应异常断线；PEER_CONNECTION_FAILED等实际错误保留并单独验证资源回收。它不等同于Tauri拦截关窗的有序停止，也不伪称无错误。

## 后续已授权云端自动测试入口

仅在具体云启动/部署范围已获授权、服务可用且新的限时私有配置已准备后执行：

```powershell
node scripts/p006c-wss-video-local.mjs --approved-wss PRIVATE_CONFIG --local PRIVATE_INTERFACE_IPV4 --case auto
node scripts/p006c-wss-video-local.mjs --approved-wss PRIVATE_CONFIG --local PRIVATE_INTERFACE_IPV4 --case fallback
```

此路径保留真实ICE服务器、完整TLS校验、短票和ICE All；fallback仅操作员合成源进程丢弃直连UDP，公开统计保留故障开关及真实丢包数，不能当成真实NAT阻断。每轮前后另用云端控制包/socket观察确认Allocation释放，禁止用停止服务替代。云端公开日志、源二进制SHA和本地结果分层保存。

## 两台物理电脑

两端使用同一新包，选择“优先直连，自动中继”。同LAN时各填写**对方实际选定网卡的IPv4**，双方原生确认会显示本次策略和确切私有对端；未填时只发布合规公网直连及relay候选。发送端生成一次性码，仅输入接收端，不发聊天。模式不一致会被拒绝，不能降级绕过授权。

每轮最长30秒，看到变化画面并核对100条发送/接收/ACK和实际路径，等待资源回收。跨网改手机热点、清空LAN对端字段，重新亲自批准和配对；实际可能直连也可能中继，以双方真实pair为准。新的物理双机/嵌入WebView2及真实DXGI结果另行验收，不能用上述单机合成测试替代。此前用户跳过的P006b主动停止补测不要求重做。
