# ADR 0010：将DirectPreferred接入现有视频与control.v1

状态：接受，按用户要求分三步实施。基线main e020051，工作树干净；先Rust Session，再WebView2与认证WSS，优先本地测试。旧阿里云授权窗口已结束，不自行启动或变更云服务。

## Session与兼容性

新增默认关闭p006-direct，依赖现有p006-relay及DirectGrant/DirectRuntime。新增Session::new_direct，复用原H264轨、RTP packetizer、DXGI/MF生产者、3 AU有界队列、control.v1 Endpoint和清理；旧new/new_lan/new_relay保持原限制，不放宽P004a网络范围。

SDP仍必须一video一application、单BUNDLE、SHA256指纹及原H264/方向约束。完整验证后一次性安装peer端点集；本端只对外发布UDP component1，记录本地与远端经验证候选集合；真实selected pair两侧都须属于这些集合，任一侧relay则中继，否则为direct。控制通道默认关闭权限，只有原生真实pair验证通过才允许处理；画面生产也在此后启动，运行时重复核验，停止/过期立即拒绝。

为覆盖已观察到的34.9秒自动回退，DirectPreferred使用单独、有界协商预算与会话总时限，不延长旧实验。信令生成应答不能阻塞WSS读写/心跳；后台任务仍归属同一Session并受停止及join约束。close先于撤销socket边界，以保留Refresh0释放机会；本地close不代表云Allocation释放。

## WebView2与WSS

新增版本化的path-policy协商，使用现有会话HMAC/nonce/递增序号封装，双方各发一次且必须在offer之前完成。Hello声明模式，服务端拒绝双方模式不一致；RelayOnly默认字段省略，保持旧包兼容。旧协议帧不增加媒体/控制代理。

每端原生批准本次模式及可选确切LAN对端IPv4（最多2个私有地址），经HMAC交换允许清单。只有对方明确允许本机接口时才对外发布私有host候选；否则只发布合规public srflx/host和已批准relay。不得自动信任远端自报的任意私有地址。SDP/地址/票据只在私有会话内存，公开仅路径类型和脱敏统计。公网端点/UDP范围/短票/证书验证沿用P006b，仍拒绝危险地址、DNS、TCP、SDP外prflx及重放。

WebView2创建ICE All的视频+控制同一PC，旧RelayOnly工厂保持严格。前端真实getStats的两侧候选都匹配本地/认证应答集合后才发送100条模拟事件；path显示从真实pair导出，不能由模式按钮推断。默认不暴露实际输入，不添加输入API。

## 本地验证与后续云授权

新增默认关闭p006-session-probe，专用于操作员合成视频源：内存生成运动NV12图案，经真实MF H264、原Session/RTP发送和浏览器真实解码，与真实control.v1并行。该入口没有DXGI调用、没有桌面内容、不写视频文件，不出现在正式Tauri命令中；统计必须注明synthetic，不能冒充桌面采集或物理双机。

本地层包括策略/SDP负例、Session生命周期、真实Windows原生→Edge合成视频与100条模拟控制、WSS TLS/HMAC/模式及策略防重放。原生嵌入WebView2、真实DXGI及最终物理双机另列实测。需要阿里云时，先完成可审阅代码和具体计划，再申请独立启动/部署范围授权；最终服务停止、不自启，不改业务服务、防火墙或安全组。
