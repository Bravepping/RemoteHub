# P0-03 Media Foundation H.264 本轮测试报告

日期：2026-11-21，Asia/Shanghai。基线：`e232216`；ADR 先行提交：`57fe0a5`。仅实现 P0-03，不代表整个 Phase 0 通过。

## 结论与代码

**真实 DXGI 主屏 → CPU BGRA 缩放/NV12 → AMD Media Foundation 硬件 MFT → H.264 → 标准解码已通过本机 10 秒 720p30 验证。**软件 H264 Encoder MFT 另做 2 秒短时试跑并解码通过。没有调用 Sunshine、外部视频源或 FFmpeg 编码器；FFmpeg 仅开发期解码、无重编码封装和校验。

新增 `crates/hub-encode-win`：颜色转换、NAL/AVCC 解析与 AU 接口、COM/MF 生命周期、硬件异步/软件同步 MFT、限时 CLI、进程内/跨进程互斥、停止和性能统计。原 capture crate、原 probe、Tauri/Vue 源码未改动。当前桌面窗口尚未集成新编码器。

## 真实环境与样本

Windows 10 Pro 22H2 build 19045.6466，AMD Radeon RX 6750 GRE 10GB，驱动 32.0.21045.16010。Rust/Cargo 1.100.0 MSVC，MSVC 14.44.35207，Windows SDK 10.0.26100.0；沿用锁定的 windows crate 0.62.2，仅新增 MF/COM/性能计数等特性。

主屏 2560×1440。开发脚本显示真实的 Windows 运动窗口（八色条、移动白块、动态计时），编码输入仍来自 DXGI 主屏，没有直接把测试图送给编码器。最终解码的 300 帧具有 300 个不同帧哈希；实际输入桌面图像 288 种，输出差异亦受到 H.264 编码影响，故两者不能直接等同。

私有样本：`artifacts/private/p003-accepted/desktop.h264`、`desktop.mp4`、`encode-report.json`。**均未提交视频/截帧到 Git 或 GitHub**。提交的证据只含参数、时间戳、NAL 类型、计数和测试结果，见 `artifacts/p003/`。

## 最终实测数值

| 项目 | 结果 |
|---|---|
| 实际选中编码器 | `AMDh264Encoder`，来自硬件类别枚举，实际编码预检和正式编码通过 |
| 输入/输出 | 2560×1440 BGRA → CPU 双线性/BT.601 limited NV12 1280×720 → Annex B H.264 |
| profile / level | Baseline，SPS profile_idc=66 / level_idc=31（3.1），无 B 帧 |
| 正式样本 | 300 个 AU，媒体时间 10.000 秒，0 丢 tick，停止标志 false |
| 正式墙钟 / 观察吞吐 | 9995.4651ms / 30.01361 帧/秒；从首帧时刻起计，最后一帧 PTS 9.9667 秒，不含约 588.885ms 准备时间 |
| 桌面更新 / 复用 | 293 次（含准备首帧）；正式循环复用 8 帧，其中因落后复用 0 帧 |
| 码率 | 请求 4Mbps；实际 4,001,605.6 bit/s；裸流 5,002,007 字节 |
| IDR / GOP | AU 索引 0、60、120、180、240；每 60 帧/2 秒，首帧实际含 SPS/PPS/IDR |
| 采集耗时 p50 / p95 / max | 3.723 / 4.379 / 13.827 ms |
| CPU 缩放/转换 p50 / p95 / max | 6.882 / 12.051 / 14.571 ms（292 次新帧转换） |
| 编码提交调用 p50 / p95 / max | 5.263 / 11.547 / 24.117 ms |
| 输入提交至 AU 取出 p50 / p95 / max | 4.915 / 13.395 / 36.320 ms；含异步等待，非纯 GPU kernel 或端到端操作延迟 |
| CPU | 4.59375 CPU 秒；相对单逻辑核 45.958%，按本进程可用 16 线程归一 2.872% |
| 内存 | 进程峰值 Working Set 118,788,096 字节，约 113.285MiB；非长期泄漏基线 |
| GPU | PDH 1 秒采样共 9 次；进程 Video Codec 0 均值 4.125%，3D 均值 2.760%；多引擎之和 6.885% 不是全 GPU 总利用率 |

驱动没有命名为 VideoEncode 的独立计数器，通用 VideoEncode 汇总值是 0；实际应查看报告中的 `Video Codec 0`，不能把通用字段 0 解读成没有使用硬件。硬件路径结论来自真实 MFT 类别/名称与实际编码，并与驱动计数一并记录。

## 独立解码、颜色、NAL、时间

- ffprobe：H.264 / Baseline / 1280×720 / yuv420p / TV range，读取 300 帧；ffmpeg `-xerror` 完整解码，0 错误。
- 八色条经实际屏幕、缩放、NV12、H.264 和 RGB 解码后全部通过，容许误差 12；此次最大通道误差 2。详见 `decoder-validation.json` 的实际 RGB 对。
- 所有 MF 输出 PTS/duration 与精确 30Hz 时钟一致，IDR 间隔 60。Annex B 实机验证；AVCC 及配置记录转换只有单元向量测试，未冒充实际 AVCC MFT 实测。
- raw `.h264` 无容器时间，ffprobe raw 速率估计为 60/1；SPS 记录 progressive=1、num_units_in_tick=1、time_scale=60、fixed_frame_rate_flag=1。**实际 FPS 用计数与 sample PTS，不使用这个估计字段**。
- 先断言零丢 tick、精确 30Hz sample PTS，再从同一裸流不重编码封装 MP4；packet PTS、r_frame_rate、avg_frame_rate=30fps、duration=10.000 秒均通过。MP4 与裸流解码帧哈希逐帧相同。详细口径见 `docs/P003_FORMAT.md`。

## 边界、生命周期与回归

| 检查 | 结果与证据 |
|---|---|
| 无同意、31 秒、非法目录名 | 真实 CLI 拒绝，退出 1，无对应输出目录 |
| 已有输出目录 | 真实 CLI 拒绝覆盖，退出 1 |
| 同时启动第二个进程 | 命名 mutex 拒绝第二实例，退出 1，无第二输出目录；在第二次运动实验中实测 |
| 主动停止 | 同一 StopToken 路径，`--stop-after-ms 2000`；输出 43 帧、stopped=true、正常 drain/退出 0，残留样本解码通过；未人工按 Ctrl+C |
| 软件回退 | `--encoder software --seconds 2`，真实 `H264 Encoder MFT`、60 帧、退出 0、解码 0 错误；本机硬件可用，未禁用驱动强迫 auto 回退 |
| 无 MFT | 测试替身为空枚举，生产选择路径返回 Unsupported；该测试前/后检查线程 COM context 已初始化/释放 |
| 设备丢失/尺寸变化 | 生产 capture 检查路径注入 AccessLost/新尺寸，错误传播及租约释放后重启通过；**没有实际拔显卡、重置驱动或切换安全桌面** |
| 参数、stride、NV12、NAL、AVCC、PTS、短暂/严重落后 | 新 crate 11 项单元测试通过 |
| `pnpm verify` | 7 项命令全部退出 0，含 17 项 Rust（原 6 + 新 11）、3 项前端测试，fmt/clippy/typecheck/build 通过；原始输出 `artifacts/p003/verification.txt` |
| 原始 probe | 30 帧、2560×1440、1045ms、0 超时，退出 0；源码无修改，样本私有 |
| 真实 Edge 回归 | `pnpm test:e2e` 1/1，通过并正常退出；日志 `artifacts/p003/e2e.txt`；不代表本轮浏览器视频互通已完成 |

## 发生的失败与修复

1. 初次扩展 windows 特性缺 Ole/Security、BOOL 命名位置不符，编译失败；按本地锁定绑定修复，fmt/clippy/测试及 release 编译通过。
2. 第一段运动样本 296 帧/4 丢 tick；增加有界短暂落后时的真实帧复用，保留严格丢帧计数和回归用例。
3. 第二段 298 帧/2 丢 tick，第一帧 PTS=666666；实际原因是 GPU 计数器初始化落在视频时钟起点之后。将全部准备移到媒体时钟之前并单列 setup_ms，随后 300 帧/PTS0/零丢 tick。前两次计数证据保留在 first/second-attempt-report.json。
4. 首个 300 帧样本的色条校验失败，实际是测试窗口 DPI 虚拟坐标与物理屏幕不一致；将测试窗口设为进程 DPI aware、关闭自动缩放，按真实物理 client bounds 重新实验，最终颜色全部通过。没有修改颜色容差掩盖失败。

## 复现与隐私

在项目根目录，先调用 `scripts/use-toolchain.ps1`，然后：

```powershell
cargo build -p hub-encode-win --bin encode-probe --release --locked
# 在一个终端启动可见运动窗口（自动关闭）；另一个终端立即运行编码
powershell -NoProfile -STA -ExecutionPolicy Bypass -File scripts/p003-motion.ps1 -Seconds 16
./target/release/encode-probe.exe --consent --seconds 10 --encoder auto --name new-private-run
# 将 REMOTEHUB_FFMPEG / REMOTEHUB_FFPROBE 指向已安装的开发验证工具
node scripts/p003-validate.mjs artifacts/private/new-private-run
# 加第三个参数为运动窗口 stdout 的 bounds JSON 文件，可额外验证八色条
$env:REMOTEHUB_VERIFY_DIR='artifacts/p003'
pnpm verify
```

输出目录必须全新。原始样本和预览都位于 Git 忽略的 `artifacts/private/`；它们仍在本机，没有擅自删除。用户可按保留策略手动清理该目录中的指定实验子目录；交付与推送只使用 git ls-files 的已审查文件，归档再次排除 private/、target/、.tools/ 与依赖缓存。

## 本轮限制与下一步

未做 Windows 11、20 分钟稳定性、真实硬件丢失、HDR、多 GPU、其他驱动、GPU 零拷贝；软件路径只有 2 秒短测，不能宣称长期软件 720p30 兼容性。没有执行完整 RustSec 审计或发布安装器。同步系统调用不提供硬实时可中断保证；异步等待/输出和实验总时长有界。

本轮 P0-03 本机编码关卡通过；P0-04、P0-05、P0-06 仍未完成，未开放远程访问，也未进入 P1。下一步是受授权的本机 webrtc-rs → WebView2 真正视频传输，随后才做 DataChannel 和两机/TURN。

📊 完成度: Phase 0 已验收 3/7（42.9%） | 主线: 进行中 | 产出: 本机 MF 编码、独立解码和性能证据 | 待办: WebRTC/DataChannel/双机/TURN | 下一步: P0-04
