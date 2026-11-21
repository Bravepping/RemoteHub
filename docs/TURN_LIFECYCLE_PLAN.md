# TURN Allocation 生命周期执行计划

1. 核对main与已有报告；记录用户对接收端停止补测的跳过决定，不改写历史。ADR0008先行。
2. 复用既有STUN工具增加授予租约/本地关闭时间点；提供离线解析与分类测试。无有效私有目标参数不联网。
3. 现有原生TurnProbe增加明确的gather/close时间事件；自动Edge只建PC和DataChannel触发gather，无屏幕媒体，关闭后退出。
4. 按授权只启动原coturn；在专用临时目录观察本次控制消息和socket。先主动释放，再无Refresh超时；之后Windows原生与Edge。实际lifetime决定观察窗口，最多60分钟/两分配；不改配置缩短试验。
5. 每例独立区分Refresh0/lease expiry/residual/inconclusive，完整保留时间线；最终停服务和socket清零只作收尾。验证业务服务/容器及配置策略未改。
6. 独立报告、README/TASKS、自动测试与真实云证据、GitHub提交；此关交付完成才进入P006c。
