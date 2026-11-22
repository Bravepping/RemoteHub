# 本轮手动双机云操作归档

此处为本轮实际执行脚本的最终副本，目标固定/opt/remotehub-p006c-manual-20261122；不包含有效配置、IP、邀请、SSH密码或密钥。本轮授权已结束，不能直接运行或沿用旧授权重开服务。新二进制复用自动云阶段已核对的SHA，不重新部署旧WSS或修改coturn配置。

manual-prepare先核对停用/端口、保存配置/业务/防火墙基线，创建独立受限目录并复制已验证二进制；上传本目录其余脚本及公开observer/wire/analyze模块后，manual-activate签发新临时证书/邀请、设置最长90分钟到期cleanup timer、临时启专用TURN和低权transient WSS、不自启。

manual-observe仅抓Allocate/Refresh，合法短票用户名在私有内存分配匿名标签，分配按各实际UDP tuple独立归类，公开不输出地址/用户名。用户在两台电脑亲自确认画面采集，云脚本不发起采集。

用户确认两轮结束后实际运行manual-finish-now：必须先从真实控制事务/socket证明每项Refresh0即时释放、当前socket0和同PID，再调用manual-cleanup停服务、删本轮私钥/配置/labels/rawpcap、取消timer并核对业务基线。expiry兜底直接调用cleanup时，若还存在分配，beforeStop端口和分类必须如实保留，不能用强停服务当释放成功。正常收尾的完整证据在artifacts/p006c-manual/cloud。

本机额外删除私有配置与带凭据ZIP，保留程序和统计；第二台的配置/ZIP由用户删除，不能以这些服务端脚本冒充已操作第二台文件系统。原临时包仅私有分发，永不提交Git。未来运维必须先取得新具体范围授权并核对实际主机状态。
