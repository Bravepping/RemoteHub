# TURN Allocation 生命周期独立测试报告

状态：**六例真实云端生命周期验证通过**。2026-11-21，观察窗口北京时间2026-11-21T22:39:14.792+08:00至2026-11-21T23:05:59.221+08:00。本轮无需用户操作Windows或重复双机串流；不采集画面、不执行输入、不修改coturn配置/业务服务/防火墙/安全组。

## 授权与基线

最新main基线ed6ff20022d17186b1c38379aff8c00eee1934d1，开工工作树干净；ADR0008先提交dde4649。用户明确决定跳过接收端主动停止补测，保留P006b原日志证据限制，不将其继续作为阻塞。用户另行批准仅临时启动既有remotehub-turn-g6、WSS关闭、最多两个分配、最长60分钟、最终停止且不自启。本轮实际最大并发中继套接字为2；未通过停止服务制造单例释放成功。

## 方法与资源关联

复用G6协议工具、server-only短票以及webrtc/rtc0.21.1原生探针；锁定包归档与实际close源码校验一致，见dependency-source-audit.json，未修改或升级第三方库。云端同一turnserver PID的控制包元数据与relay UDP端口以事务散列/私有标签关联，0.5秒采样。原始捕获仅含Allocate/Refresh消息，不含媒体；只导出无IP/票据/nonce/realm/密钥的events.jsonl。

每例均保存分配前基线、成功Allocate和实际授予租约、客户端关闭时间点、close后的socket消失、原授予租约到期及15秒宽限后的状态。按最后一次成功正lifetime响应计算deadline；实际六例授予均600秒，即使协议请求60秒也不提前判定。短票到期与Allocation租约到期分开；不是看到凭据过期便认回收。

Refresh(0)请求+对应成功lifetime0回应+3秒内socket消失，归类即时释放；没有成功0释放而在租约到期附近消失，归类超时；deadline+15秒后仍在为异常残留；无法关联、缺样本或PID变化为证据不足，不默认为PASS。数值为**首次观察到消失**的延迟，真实释放时刻位于相邻约0.5秒快照之间，并非硬实时保证。

| 场景 | 实际授予租约 | 分类 | ACK后/到期后首次观察消失 | 原租约到期+宽限仍无socket |
|---|---|---|---|---|
| 协议主动Refresh(0) | 600秒 | refresh_zero_immediate | 1.464秒 | 是 |
| 协议停止续租 | 600秒 | lease_timeout | 0.318秒 | 是 |
| Windows原生gather后close | 600秒 | refresh_zero_immediate | 1.110秒 | 是 |
| Windows Edge gather后close | 600秒 | refresh_zero_immediate | 1.135秒 | 是 |
| 已连接Rust（Edge先关后本端close） | 600秒 | refresh_zero_immediate | 1.257秒 | 是 |
| 已连接Edge（本端先close） | 600秒 | refresh_zero_immediate | 1.224秒 | 是 |

协议主动释放的客户端还验证了Refresh0响应消息完整性。协议超时例故意直接关UDP socket、没有发Refresh0，在600秒实际deadline后约0.318秒首次观察到消失。整个六例判定期间服务PID未变、未停止，详见results.json和observer-events.jsonl。

## Windows自动测试的实际范围

原生webrtc0.21.1与Edge Chromium分别进行真实gather/close；另由本机私有stdio broker连接两者，只建data-channel application，不建视频轨，实际SCTP ping/pong成功且双方实际selected pair均relay/relay UDP。已连接轮次先Edge close，约1.5秒后Rust close，两者各自的0请求/响应与独立中继端口消失均匹配。

这证明本轮环境下gather-only和已连接data-only的PC关闭均实际触发主动释放；不是模拟SDK返回值。**Edge不是嵌入WebView2，本轮不是新的双机视频/负载/弱网测试**，不能无限推广到所有负载和丢包场景。close返回表示本地PC退出，不等同同步等待服务端释放确认；仍需真实云端响应和资源状态佐证。本轮未复现或归因上轮停服务前那1个未关联socket，保留历史记录，不追填它的具体来源。

客户端和服务器分别保留原始Unix时间/单调计时；一次SSH查询测得服务器比Windows快约1.859秒，偏差区间[1.777, 1.942]秒。释放延迟均用服务器同一时钟计算；Edge时钟分辨率1ms，未伪装纳秒精度。

## 自动检查与收尾

19项检查均达到预期：61 Rust、旧桌面3、16前端、3 Edge、11 Python，fmt/lint/default及all-feature clippy、完整桌面/probe构建、旧feature独立编译、显式CLI无授权拒绝。3条拒绝检查期望退出码2/1/2；私有MF样本1项仍ignored。原始结果见automatic，离线fixture与真实云数据分开。

所有单例在停服务前已经完成分类和原deadline+宽限观察；停止前relay socket为0。最终专用coturn inactive/disabled，WSS始终未启动，原配置/unit和INPUT/OUTPUT/归一化nft策略hash均未变，两个业务容器启动时间不变。原始控制捕获和短票按本轮私有数据清理，保留捕获SHA256及脱敏事件；具体收尾差异见shutdown.json，不将整服务清零当成单例通过。

## 结论与限制

已区分并实测Refresh0即时释放与租约到期释放；本轮未观察到异常残留。异常分类仅由离线负例覆盖，不捏造真实残留案例。close仍是网络上的best-effort，不能保证丢包时每次都即时释放，600秒服务器租约是另一回收边界。样本数量有限，无压力/切网/服务重启/新媒体负载测试，无长期身份或输入注入。

本报告及代码先独立提交GitHub，再按用户要求进入P006c；不以配置中存在TURN或ICE策略字符串代替P2P/回退实测。新云端服务启动/变更范围仍需先具体授权。
