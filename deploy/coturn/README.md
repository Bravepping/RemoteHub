# RemoteHub P0-06a coturn准备包

状态：离线可审查准备，不是已部署服务。当前没有核实阿里云ECS地址、发行版、EIP映射、域名/证书、已有服务或SSH/安全组权限；没有执行任何云命令。只有明确确认目标和授权后，才允许按此runbook操作。初始版本支持TURN UDP；原生webrtc0.21.1源码跳过TURN TCP/TLS，即便服务器开端口也不能声称客户端支持。P006父项、双机relay与自动回退均未完成。

## 文件与离线检查

- `turnserver.conf.example`：带占位符，故意不可直接启动。`.env.example`：所有部署身份/地址/密钥留空；不能source执行。
- `render.py`：只读私有env、校验、排他创建0600配置，不覆盖、不安装、不发网络请求；Linux env必须0600。Windows需另核对用户目录ACL。
- `verify.sh --template` / `verify_config.py --template`：检查本项目安全基线；不是coturn官方语法检查器，也不证明安装包接受配置。
- `verify.sh --config PRIVATE_CONF`：离线检查已渲染配置，不打印秘密/地址。`--inventory`仅在目标Linux读取OS和coturn版本/工具存在，不安装/修改，不网络探测。Windows Git Bash可用环境变量REMOTEHUB_PYTHON指定已安装Python，不能把本机结果当目标Linux审计。
- `remotehub-coturn.service.example`：独立systemd单元示例，非root、无capability、只读系统、限内存/线程；账户和路径必须按目标包核实，不替换已有服务。
- `probe-input.json.example`：只有占位符/到期0，不能用于连网；只放短期用户名/凭据，没有共享密钥。

本地离线检查：`python deploy/coturn/verify_config.py --template`、`python -m unittest discover -s deploy/coturn -p test_config.py -v`、`bash -n deploy/coturn/verify.sh`。`python3 render.py --env PRIVATE_ENV --output NEW_PRIVATE_CONF`需先填写经核对的实际资料，不允许把实际env/config提交Git。建议在仓库外私有目录，仓库内deploy/coturn/private也已忽略。

## 真实云端开工前的只读核对

1. 用户确认ECS实例/地址、系统版本、登录方式、是否授权只读核对及后续变更。不要把私钥/控制台token发到聊天；使用已配置的本地认证。没有授权就停在implementation-ready。
2. 核对官方包来源和已安装版本：目标机`/etc/os-release`、`command -v turnserver`、`turnserver --version`、发行版包管理器policy/version。coturn Wiki自身声明过时，以目标包man/help和当前官方源核对每项选项；此包未安装或自动升级coturn。
3. 核对本地接口/路由、实例控制台EIP映射、现有3478/5349/49160..49259 UDP占用及443上既有服务。只读命令`ip -brief addr`、`ip route`、`ss -lntup`结果保持私有，摘要只记录是否满足；不要复制真实IP、进程资料或原日志到Git。确认现有coturn服务，不能在同端口叠加第二个实例。
4. 直接持有公网IPv4时，TURN_LISTEN_IP/TURN_RELAY_IP填对应本地接口地址，映射两字段留空。EIP映射RFC1918私网时，分别填本地监听/relay地址与已核实PUBLIC/PRIVATE映射；渲染器要求private=relay接口。不能把EIP误填为不存在的本地接口，也不能在私网relay上漏external-ip。
5. 核对双方测试网络与时间同步；本轮不必安排跨NAT视频，P006b再用两个不同网络强制relay。阿里云约50Mbps只是文档背景，当前实例实际带宽/计费/配额未知。

## 用户批准后的手工部署（示例，不在本轮执行）

以下以Debian/Ubuntu官方包与systemd为例，其他发行版先改成实际包名/账户/路径。先检查和记录当前状态、备份可能受影响配置/单元/规则ID，再提交具体变更清单获批准，不默认修改现有服务。

1. 通过官方仓库安装coturn（如`apt-get install coturn`），记录安装版本与系统默认自动启动行为；若默认服务可能自启并占3478，先规划批准后的受控停用，避免在受限模板安装前短暂暴露默认服务。不盲目开启发行版默认unit。
2. 为本次实验产生新的独立realm，生成至少32字节强随机共享密钥（例如服务端`openssl rand -hex 32`），只写本机0600私有env/密钥存储。不要将输出留在终端录屏、shell历史、仓库、客户端或消息中。issuer的REMOTEHUB_TURN_SHARED_SECRET与coturn配置使用同一秘密；本模块不带HTTP接口。
3. 用render.py创建新的私有配置并离线校验。核实实际服务账户（示例turnserver）与/usr/bin/turnserver路径。专用目录可采用root:服务组 0750，配置root:服务组 0640供非root服务读取，env保持root 0600；不能world-readable。模板默认丢弃流量日志，禁止verbose/CLI/username标签。
4. 审查systemd单元后安装为新remotehub-coturn.service，并`systemd-analyze verify`。3478为非特权端口，不需root常驻或额外capability；若目标包需写数据库/运行目录，只允许核实的专用路径，不放宽ProtectSystem到全局可写。
5. 经批准协调阿里云安全组与Linux防火墙：初测仅3478/udp，以及49160–49259/udp中继，限定已知测试来源如环境允许。回程UDP/conntrack策略也需核对。不要关防火墙、不要放开全部端口；5349/tcp和3478/tcp当前无需开放。TURN TLS以后必须实际证书/主机名验证与支持该传输的客户端，单开端口无效。
6. 仅批准的实例上启动新unit并核实实际监听/非root UID/资源限制。不要启用开机自启或无人值守作为本轮默认行为。模板是草案，只有目标coturn实际启动和功能负例通过才算运行配置验证。

安全基线：use-auth-secret自动启用其所需的长认证机制，不叠加静态user/lt-cred-mech/no-auth；拒绝匿名Allocate，但普通STUN Binding可匿名，这不等于开放TURN。禁止内网/CGNAT（含常见云元数据地址）、loopback、link-local、组播/保留目标及本轮IPv6 peer；allowed-peer-ip会覆盖deny，因此检查器全部拒绝该覆盖。单用户4/全局8 allocations、单allocation 1,000,000 B/s、服务4,000,000 B/s，输入/输出分别计；不是账单硬封顶。实际带宽、丢弃与并发容量必须实测，不能推断4Mbps必过。

两端都relay在同一coturn时，peer可能是服务器本身的公网relay地址，不能一概拒绝自身公网IP而破坏中继；逻辑relay端口范围由客户端策略约束。TURN票据持有者仍能请求该服务器允许的其他公网UDP目的地，短TTL/配额/审批不能替代完整滥用审计。更严格出口规则需单独审核回程、relay自循环与其他既有服务，不能擅自改全机规则。

默认日志/dev/null且no-stdout-log，避免IP/用户名流量记录无界增长。若排障确需日志，另经批准启用仅本次的私有文件、0600/0640与≤1MiB轮转/短保留，不打开verbose/原始凭据；分享前脱敏。保留服务active/版本/非敏感错误计数摘要，不将原IP日志上传Git。

## 授权后的最小网络验证（G6，当前blocked）

构建：`cargo build -p hub-webrtc --bin turn-allocate-probe --features p006-turn-probe --profile probe --locked`。默认或`--help`不访问网络。此工具无屏幕采集、无远端SDP、无视频轨或模拟控制事件，只触发真实库TURN gather/Allocate并关闭；最长gather10秒+cleanup3秒。不是P006b视频验收。

仅在用户明确批准该端点测试后，准备私有probe JSON：复制示例并填已核对的本机接口、TURN服务器/relay地址和端口范围；server-only issuer经受认证且双方批准的context签发短期票据，expires/username/session UUID必须一致，approval期限不超过现在600秒。server-only模块不是公开认证接口；未实现WSS时只能由受信任实验操作者显式准备上下文，不能接受浏览器传来的auth=true作公网认证。Linux JSON需0600，Windows限制ACL；不把静态共享密钥放入该文件。

`turn-allocate-probe --approved-target PRIVATE_JSON`仅访问该文件明确批准的数字IPv4端点，返回脱敏relay候选数/null selected pair/cleanup状态。正确票据应实际得到relay候选并释放；分别用缺少、改错、过期票据运行应失败；保存固定错误码、候选数、耗时/退出码及服务摘要，不保存文件内容或完整SDP。纯算法验证成功不能代替这些真实coturn结果。可另在同一获授权目标用coturn自带`turnutils_stunclient -p 3478 TARGET_IP`验证STUN Binding，把输出作为私有资料只摘要成功/失败；不要探测第三方STUN或扫描端口。

realm限制：REST密码HMAC本身不含realm，原生库按服务器challenge协商realm；当前API不能指定一个固定expected realm。issuer会拒绝请求错realm，但这不等于实测coturn对所有跨realm复用都拒绝。同一静态secret不得跨realm复用，真实负例与限制需记录。REST凭据有效期内可复用、不可单票即时撤销；停止应用会话/签发限额与TURN allocation生命周期分别计，不能声称票据一用即废或用户停止即全球撤票。

## 回滚

停止并禁用本次专用unit（仅限本次实际创建的单元），撤销本次新建的安全组/防火墙精确规则ID，恢复本次明确变更过的备份，不清空全机规则或其他服务。撤销issuer会话批准、销毁短期私有JSON与env的访问权限并轮换本次secret；已签发REST票据不能瞬间单票吊销，观察短期TTL/allocations退出。移除专用服务文件前核对路径及所属，保留脱敏审计摘要。默认构建关闭p006-turn-probe即可回到原客户端；没有修改原LAN地址策略。

官方依据：[当前coturn配置](https://github.com/coturn/coturn/blob/master/examples/etc/turnserver.conf)、[TURN REST草案](https://datatracker.ietf.org/doc/html/draft-uberti-behave-turn-rest-00)。目标包版本与配置实际加载、ECS映射、安全组及G6输出尚未取得，本轮不能标infrastructure-verified，更不能P006 done。
