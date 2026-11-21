# P0-04a 交付前代码与证据审查

范围：相对 7fcbb51 的新增实时生产者、webrtc 适配、Tauri 原生授权/ACL、Vue 生命周期与测试；未称全仓完整安全审计。

| 要求 | 实现/证据 | 结论 |
|---|---|---|
| 实时源而非文件 | stream.rs 持有 DxgiCapture/MfRuntime；不调用文件 probe；实际原生解码 450/900 帧 | 本机通过 |
| COM 线程归属 | std::thread 内构造和销毁，Tokio 只收拥有字节的 EncodedVideoFrame | 通过静态与实机清理检查 |
| 队列与时钟 | capacity 3，第四 AU 明确失败；90kHz 精确转换/单 AU marker/FU-A 复原测试 | 通过 |
| 原生同意与会话 | Config、MessageBox、reserve、窗口 URL/label、sessionId、feature 默认关闭 | 通过；对话框点否只代码路径，未人工复测 |
| 同机约束 | 127 UDP、无 ICE server、mDNS disabled；原生独立 SDP 限制；实际选中 remote 127 | 本机通过，不能推广其他环境 |
| 停止与关闭 | stop 幂等/线程 join/PC close；主动停止 47 帧、关窗 33 帧/cleanup=true，进程消失 | 通过；GUI OS 退出码未捕获 |
| 前端取消竞态 | 已停止后不创建新 PC；迟到授权原生立即撤销；异步 stats 返回后再次检查 stop | 新回归通过 |
| 帧统计真值 | native 输入/编码/发送与 JS received/decoded/rendered 分开；未测保持 null | 通过；关窗 JS 数值不猜测 |
| 隐私 | 无视频 IPC/网络外发；程序只写固定私有统计目录；归档按 git 文件审查 | 打包时再次检查 |

发现并修复：SPS 严格字节比较错误、停止产生的 play AbortError 覆盖原错、启动结果迟到导致短暂残留授权、库未测 RTT 默认 0 的误导。对应真实失败/单元测试在报告中，不隐藏。

已知限制：AMD SPS 保留位瑕疵，按标准解码忽略规则兼容且原字节不改；尚无 GPU/网络故障实机压力测试、公网身份授权或完整 RustSec。源/队列/IPC 上游与旧 probe 调用通过 rg/实际源码核对，不依赖未部署的知识图谱。没有引入生产发布或云资源变更。
