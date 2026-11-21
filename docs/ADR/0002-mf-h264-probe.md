# ADR 0002：P0-03 使用真实 DXGI + Media Foundation H.264 MFT

日期：2026-11-21。状态：采用，硬件与帧率结论以实测报告为准。

## 本轮范围

独立 `hub-encode-win` crate 与本机编码 CLI；不修改已有捕获 probe，不增加 Vue/IPC 像素传输，不开放远程访问。通过现有 `DxgiCapture` 得到主屏真实 BGRA，先用可测试的 CPU 缩放与 BT.601 limited-range BGRA→NV12 转换，输入 Windows H.264 MFT。GPU 零拷贝、WebRTC、DataChannel、TURN 不属于本轮。

## 选择与备选

直接调用 IMFTransform 而不是把 FFmpeg 当作编码器：可观测所选 MFT、输入/输出 sample 时间戳、NAL 和编码耗时，并保留后续 WebRTC 的 `EncodedVideoFrame` 接口。MF Sink Writer 的自动编码路径更简便，但不适合本轮逐个访问编码 AU；FFmpeg 只作为开发期独立解码/封装验证工具，不是客户端运行依赖。

枚举硬件和软件 MFT，优先尝试可用硬件实现，记录真实激活/格式/事件处理失败原因后才回退软件。硬件 MFT 是异步模型，不能按同步 MFT 的 NEED_MORE_INPUT 规则调用。只有真实编码通过，才能声明某个编码器可用。软件通过不代表硬件已验收。

## 契约

- 1280×720，30fps，默认 4Mbps，Baseline profile，目标 GOP 60；具体生效参数记录在报告。
- 单调 100ns PTS 与 duration，CFR 30Hz；无新桌面更新时复用最近真实帧，分别计数 desktop updates、repeats、missed ticks 和 encoded frames。超过帧周期不积压帧，跳过旧 tick 并记录丢帧。
- 缩放采用 CPU 双线性 BGRA，色彩转换为 BT.601 limited-range NV12，显式设置色彩属性；输出有界，NV12 stride=width，Y 后接交错 UV。颜色和缓冲边界有独立向量测试。
- `EncodedVideoFrame` 保存标准化 Annex B AU、PTS、duration、NAL 类型与 IDR 标志；支持 Annex B 和明确长度字段的 AVCC 解析，拒绝截断、过大或无效数据，不靠猜测吞掉坏帧。
- CLI 必须 `--consent`；输出只能位于 `artifacts/private/` 的新目录；样本、像素和原始 NAL 不进入日志、Git 或网络。
- 最长 30 秒、最多 900 帧、单 AU ≤8MiB、输出 ≤128MiB；单帧同步处理，不设无界队列；异步事件等待与 drain 有期限；停止/错误均释放 MFT、MF 和 COM，软件同步系统调用的最坏阻塞不作为硬实时保证。
- 诊断记录不包含画面：编码器类型/名称、格式、时间戳、帧数、SPS/PPS/IDR、耗时分位数、丢帧、CPU/GPU/内存可用实测。

## 验证、恢复和撤销条件

先验证纯转换/NAL/授权与生命周期，再在现有 Windows 10 + AMD 实机输出至少 10 秒的运动桌面，独立 ffprobe/ffmpeg 解码检查；运行 pnpm verify 和旧 probe 回归。对无法实际注入的设备丢失与无 MFT 情况，区分可控测试和硬件实测，不能都称 PASS。

新模块失败不影响旧 probe；样本保留在私有目录供本机复核。必要时停止本机 CLI 即释放资源，不修改系统或云配置。性能瓶颈实测后可用 GPU NV12/纹理输入替换 CPU 路径；只有明确的兼容性实证才能重新评估媒体架构，不改变既定 WebRTC 路线。

参考：[Microsoft H.264 MFT](https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-encoder)、[MFT 枚举与类别](https://learn.microsoft.com/en-us/windows/win32/api/mfapi/nf-mfapi-mftenumex)、[异步 MFT](https://learn.microsoft.com/en-us/windows/win32/medfound/asynchronous-mfts)。
