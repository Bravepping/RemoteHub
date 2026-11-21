# TURN Allocation 自动生命周期验证

仅为明确授权后的操作者工具，默认不联网、不采集屏幕，不是客户端长期后台。先核对main、专用unit和业务基线，取得具体启停/观察范围授权；不改coturn配置、Linux防火墙或安全组。server observer依赖目标已安装的tcpdump/ss和Python标准库，不安装代理或日志服务。

`turn_lifecycle_observer.py --private-config PRIVATE_JSON --private-dir PRIVATE_DIR --seconds 3500`只观察服务端本次Allocate/Refresh控制消息与该turnserver PID的relay socket。PRIVATE_JSON={labels:{short_username:case},turn_pid}，0600/root私有目录，不提交。原始pcap只在私有目录暂存；events.jsonl导出时间、测试标签、事务hash、授予lifetime、relayPort与socket存在性，不包含IP/username/nonce/realm/凭据。信令WSS无需启动。

先后执行：`python scripts/turn_lifecycle_client.py --approved-target PRIVATE_JSON --case protocol-release`和`--case protocol-expiry`。输入为{target,local,tickets,approval_expires_at}，由已授权操作者在服务端签发短票，必须公网上的批准IPv4与RFC1918本机地址；不能把TURN共享secret给Windows。release确认正确消息完整性的Refresh(0)回应；expiry刻意直接关闭UDP套接字，不发Refresh0。观察期限采用成功响应的实际授予值，不采用请求值或短票期限。

协议超时回收确证后再执行Windows原生CLI：显式`p006-turn-probe`构建的`turn-allocate-probe --approved-target PRIVATE_JSON --lifecycle`，复用既有探针，输出gather及PC close调用/返回时间。`node scripts/turn_lifecycle_edge.mjs --approved-target PRIVATE_JSON`在真实Windows Edge headless创建实际PC和DataChannel以触发TURN，随后close。Chrome的default_public_interface_only仅为此测试进程限制gather接口，不改系统网络配置；一次只运行一个浏览器PC、最多两个分配。Edge不是嵌入式WebView2，gather-only没有remote SDP、connected selected pair或媒体，报告须明确。

补强connected测试：`node scripts/turn_lifecycle_connected.mjs --approved-target PRIVATE_JSON --native-config PRIVATE_NATIVE_JSON`启动同一原生探针的`--connected-lifecycle`模式，仅经本机进程stdin/stdout私下交换data-only SDP，公开输出过滤该行。收到真实SCTP ping/pong、双方实际relay selected pair校验后，Edge close，再由操作者broker让Rust close。候选仅批准relay范围、单application媒体、有效SHA256指纹；不启用视频轨或输入注入。自动测试仍不能替代嵌入式WebView2或真实双机媒体场景。

`python scripts/turn_lifecycle_analyze.py --events EVENTS_JSONL --out RESULT_JSON`关联事务、relay端口、同一coturn PID和连续快照。支持Refresh0即时释放、lease超时、异常残留、尚未到期、证据不足五类。缺少ACK、socket未观察到、进程更换或提前消失都不会默认为PASS；最后一次成功正lifetime Refresh会更新deadline。即时窗口3秒、采样约0.5秒、租约宽限15秒，原始时间均保留；客户端与服务器时钟分开并保存偏差区间。

脱敏日志与独立报告归档后再停止观察器及专用服务。整体服务停止、兜底timer触发后的socket清零不能充当单Allocation通过条件。记录配置/unit hash、业务基线与策略hash，清理本轮短票/原始捕获、保持专用unit disabled。离线用`python -m unittest discover -s scripts -p 'test_*turn*.py' -v`验证解析和分类，明确fixture不是真实云端结果。
