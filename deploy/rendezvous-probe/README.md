# P006b 临时 WSS 服务

只用于用户已授权的短时双机实验，不是正式设备身份或长期信令服务。先核对目标、现有服务/容器、专用端口与备份。服务需非root用户；不设置开机自启，最长运行2小时。操作完成后停止本次 WSS 和实验专用 TURN unit，不操作其它业务。

从仓库锁文件构建 `cargo build -p hub-rendezvous-probe --release --locked`；网络受限时可在隔离的三crate workspace中 `cargo vendor` 并校验 `.cargo-checksum.json`，服务器 `--offline --locked` 构建。隔离workspace只改变成员清单，不改变源码或依赖版本。完整工程不依赖这份vendor归档运行。

模板不能直接运行。`issuer.env` 仅包含服务端 `REMOTEHUB_TURN_SHARED_SECRET`，与既有coturn私有配置一致，权限0600；客户端包绝不包含该文件。server.json中realm必须匹配，角色邀请各32字节随机hex、互不相同、期限60..7200秒；配置/私钥root:专用服务组0640。手工批准的双方入房才签发会话短票，不提供额外HTTP签票API。

无域名实验使用专用CA签发含批准IPv4 SAN的服务证书。客户端只在自己的rustls根仓中载入该CA，校验有效期/IP，不安装系统根证书。CA私钥/服务私钥留在服务器，客户端只有CA公证书与限时邀请。到期或撤销邀请需重新生成配置和包，重启专用服务前确认无活动会话。

端口为配置指定TCP8443（TLS/WSS）和既有TURN UDP3478、49160..49259。未获准时不修改安全组/防火墙；发现入站拦截应交付具体最小规则给用户。不能关闭证书验证、使用明文ws或扩大旧Loopback/LanPinned范围绕过错误。

回退：停止 `remotehub-p006b.service` 与本次获准开启的 `remotehub-turn-g6.service`，核实无监听/会话，保持disabled。保留服务配置以供审查，撤销本轮邀请；再次使用需显式授权与新期限。本模板不安装依赖、不开放网络规则、不采集桌面。
