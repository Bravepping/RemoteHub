# P006c 首步云端自动路径测试操作范围

状态：用户于2026-11-22明确授权本范围，已执行并收尾。北京时间07:13:41至07:17:31临时运行，最终inactive/disabled、WSS保持关闭；脱敏结果见PHASE0_P006C_TEST_REPORT.md和artifacts/p006c/cloud。本记录不构成后续重新启动授权。

1. 只在既有阿里云ECS临时启动remotehub-turn-g6.service，不启用自启、不启动remotehub-p006b/WSS，不修改coturn配置、业务服务、Linux防火墙、安全组、路由或系统代理。沿用UDP3478和relay49160–49259。
2. 新建root-only目录/opt/remotehub-p006c-path-test-20261121，上传现有脱敏观察器、分析器、G6协议解析工具及本轮执行/收尾脚本。只捕获该端点Allocate/Refresh控制包，不捕获媒体。基线/原始捕获/票据受限；公开只含脱敏计数、时间、事务散列、relay端口和分类。
3. 服务端读取原有共享密钥签发4组各2个短票，最长540秒；密钥不离开服务器。逐例真实运行本机Windows两个原生PeerConnection（无屏幕采集、无输入、无视频轨），两端ICE All、每个可连通场景100条固定序号及ACK。四例：正常直连；测试进程丢弃非TURN端点UDP以迫使ICE自动选中中继；丢弃TURN端点但直连可达；两者全丢弃，预期显式失败。故障不修改服务器或系统网络。
4. 最大同时2个Allocation，前一例PC.close后观察服务PID及对应relay端口确实消失才进行下一例。若Refresh0未确认则等待实际租约+15秒并另列超时/残留/证据不足，不能停服务冒充释放；TURN不可达的两例须证明无对应Allocate成功。记录实际selected pair及100/100/100，不能把host候选存在或relay配置当成功。
5. 总窗口最多40分钟，临时非自启到期timer兜底；提前完成即停止专用coturn，保持disabled，取消timer，核对业务服务/容器启动时间、原配置/unit及防火墙策略。原始捕获和短票删除，保留SHA及脱敏证据。服务停止前单例未过的如实标失败/未完成。

测试仍属同一物理Windows PC上的两个真实端点，不是新的物理双机/跨ISP/NAT穿透或WebView2视频验收。本步不接入产品视频入口；下一步才复用现有视频会话进行兼容性验证。
