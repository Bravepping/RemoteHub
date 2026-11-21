# P0-03 媒体格式与时间契约

本轮接口为本机实验，不能直接授权远程访问。

## 输入与色彩

`hub-capture-win::DxgiCapture` 返回真实主屏紧凑 BGRA8。CPU 双线性缩放到 1280×720，再按 BT.601 limited-range 整数矩阵转换 NV12。Y stride=1280，Y 平面 921600 字节；交错 UV stride=1280，460800 字节；每帧 1382400 字节。UV 取每个 2×2 RGB 平均值，忽略桌面 alpha。输入宽高/stride/缓冲长度、输出偶数尺寸均校验。无 GPU NV12/零拷贝声明。

MFT 的输入 NV12，与 ffprobe 解码输出显示的 `yuv420p` 不是同一个处理阶段。颜色以八组标准色条端到端检验，不仅以枚举值推断。

## 编码与 AU

`mf::MfEncoder` 先设置 H.264 输出，再设置 NV12 输入。硬件 MFT 使用 NeedInput / HaveOutput / DrainComplete 事件，等待有期限；软件 MFT 使用同步 ProcessInput / ProcessOutput。记录每个候选实际激活/编码预检结果；硬件失败后 auto 才尝试同步软件 MFT，显式 hardware 模式不悄悄回退。

`h264::EncodedVideoFrame` 字段：

| 字段 | 语义 |
|---|---|
| `annex_b` | 拥有所有权的单个 AU；每个 NAL 归一化为 4 字节起始码 |
| `pts_100ns` / `duration_100ns` | 原生 MFT 输出 sample 的时间与时长 |
| `nal_types` / `is_idr` | 实际解析的 NAL 类型，type 5 表示 IDR |
| `encode_elapsed_ms` | 对应输入提交到本机取出并解析该 AU 的观察耗时，含异步等待，不是纯 GPU kernel 时间 |

支持 Annex B 的 3/4 字节起始码和明确 1..4 字节 length size 的 AVCC。AVCDecoderConfigurationRecord 的 SPS/PPS 会保留，必要时在 IDR 前补充。空 NAL、截断、非法头、过大长度、超过 256 个 NAL 或单 AU 8MiB 均拒绝。AVCC 分支有向量测试，本机 AMD/Microsoft 实测输出为 Annex B，未声明实际测过 AVCC MFT。

## 时钟、静态桌面与背压

`PTS(n)=floor(n*10000000/30)`；duration 为相邻 PTS 差，交替 333333/333334，300 帧恰好 10 秒。首次硬件预检用真实第一帧，不进入交付样本；COM/MF/编码器/GPU 计数器准备全部完成后，正式视频时钟才开始。

没收到 DXGI 更新时复用最近真实 NV12，不制造黑帧。落后半个周期时暂缓新 capture/转换，复用最近 NV12；最多允许一个 tick 的短暂延迟，落后超过两个 tick 时丢旧视频 tick 并计数。MFT 在途输入最多 64 个、一次取出最多 32 个 AU、drain 最多 64 帧且最长 3 秒，输出累计最多 128MiB，不建无界队列。

分别报告 desktop updates、distinct desktop images、repeated frames、late repeats、missed video ticks、encoded frames。GPU/CPU/内存采样和驱动调用会影响计时；没有恒定实时或端到端低延迟保证。

## 裸流与 MP4

`.h264` 不含容器 packet PTS。MF 输出逐帧时间保存到私有 `encode-report.json`，其非画面副本可提交。开发验证先检查全部时间戳严格 30Hz、零丢 tick、300 帧/10 秒，再用 FFmpeg **不重编码**封装 MP4；检查每个 MP4 packet 的 PTS 与原时钟对应，及裸流/MP4 解码帧哈希逐帧一致。已有 MP4 不覆盖，重新验证时必须通过一致性检查。

本机 raw ffprobe 的 `r_frame_rate` 为 60/1，不能将其当作实际输出 FPS；MF PTS、SPS、MP4 和实际计数需一起检查。FFmpeg 官方将这个裸流速率字段说明为估计值：[AVStream 文档](https://ffmpeg.org/doxygen/trunk/structAVStream.html)。本机 SPS 为 progressive、num_units_in_tick=1、time_scale=60、fixed_frame_rate_flag=1，MP4 的实际 packet/avg/r_frame_rate 均已验证为 30fps。后续 WebRTC 使用 AU PTS 转 90kHz RTP 时间戳，不能采用裸流探测速率代替媒体时钟；浏览器互通仍待 P0-04 实测。
