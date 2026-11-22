# P1-04 已实现的状态与IPC合同

日期2026-11-22；仅本轮只读产品能力，不替代P2身份或P3会话合同。原P101合同/旧报告保持历史原文。

## 真实接口与边界

`DesktopApi.readRuntime({signal?}) -> Promise<RuntimeInfo>`是唯一产品接口。默认适配器调用现有`invoke('get_runtime_info')`且不传参数，不接受命令字符串/URL/会话票。Rust handler、manifest和capability未增加。原bridge.getRuntimeInfo同路兼容导出；runCaptureProbe及原实验接口不改、不在本轮调用有效采集。

| 响应字段 | 要求 | 含义 |
| --- | --- | --- |
| app_version/os/arch/capture_backend | 必填字符串，分别最长64/32/32/192字符，非空，无控制字符 | 本机核心编译/平台信息，不是远端身份/在线证明。 |
| remote_access_enabled | 必填boolean | 现有原生字段，不能替代本次对端批准。 |
| p004_enabled/p004_lan_enabled/p005_enabled/p006_enabled/p006c_enabled/p007b_diag_enabled | 存在则必须boolean；缺失保持缺失 | 编译实验标记；产品不能因标记存在绕过实验同意/授权。 |

仅接受普通或null-prototype对象和已知字段；数组/未知字段/错型/超长拒绝，返回冻结的字段副本。静态TS类型不替代此校验。没有DeviceInfo、SessionStats或事件数据源，设备/会话服务保持尚未接入；不返回空数组冒充已成功查询远端，也不生成虚构ID或0ms/FPS。

## 状态所有权与生命周期

App安装createDesktopState：每应用一个Pinia，服务用Vue依赖注入，默认真实Tauri adapter。store拥有runtime、idle/loading/ready/error和固定错误，只有界面刷新动作读取。多个页面共享本次状态，导航不重新创建store；重复刷新合并。失败清旧值，手动重试才新读。

原生只读读取有5秒上限，无自动重试。取消/应用卸载/Pinia scope销毁清计时器与AbortSignal监听，并使代次失效；旧成功/错误/finally不能覆盖新读取。Tauri invoke本身无法撤销已发出的读取，取消是停止等待/展示，**不声称取消了Rust任务或媒体会话**。不存在产品事件源，所以本轮没有注册Tauri事件订阅；无需虚构解除订阅成功记录。

| 固定错误码 | 用户反馈 |
| --- | --- |
| BROWSER_UNAVAILABLE | 浏览器预览不能读取本机核心。 |
| IPC_FAILED | 本机读取失败，可手动重新检测。 |
| IPC_RESPONSE_INVALID | 核心返回信息不符合合同。 |
| IPC_TIMEOUT | 超过5秒读取上限。 |
| IPC_CANCELLED | 本次读取已取消；界面主动取消回idle。 |

未知原生错误不回显原文、cause、SDP、地址或票据；即使已知错误对象的message被修改，也按固定码重新生成消息。Pinia仅内存、不存密钥/画面，不使用localStorage或持久化插件；它不是安全授权边界。

## 测试替身与未来接线

单元/集成通过createDesktopState(testApi)显式注入同一DesktopApi，仍使用真实Pinia及响应校验。替身仅测试文件定义；默认程序无演示选择器、URL开关或自动回退mock。Edge的SDK transport fixture明确标识，不能当真实原生/云结果。

真实设备/配对/会话命令与事件仍未实现，不注册空操作或模拟返回值。后续接线必须由Rust维护会话/批准/代次和幂等停止，真实selected pair/getStats按live/last_known/unavailable呈现；R06拒绝不得变成功或触发绕校验。P1-05/06不随本轮开工。

依赖与接线参考：[Pinia官方固定版本源码](https://github.com/vuejs/pinia/tree/v3.0.4)、[应用实例与store使用](https://pinia.vuejs.org/core-concepts/outside-component-usage.html)、[官方测试说明](https://pinia.vuejs.org/cookbook/testing.html)。本轮实际API以安装的3.0.4声明及执行结果为准。
