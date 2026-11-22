# ADR0022：本机长期身份与CurrentUser DPAPI

安全审查补充：在独占锁文件中持久化固定版本提交标记和完整公钥摘要，先确认密文可恢复再提交标记、最后返回身份。单独丢失密文或替换其他可解密身份须fail closed；不靠文件不存在就自动换号。完整目录删除/合法旧状态整体回滚仍须后续注册/撤销机制处理。

日期2026-11-22；P2-01，基线d7ce2e0。

选择hub-identity作为Rust私钥所有者。Ed25519使用固定ed25519-dalek2.2.0（MSRV1.81低于项目1.90），启用zeroize/std/fast，不启hazmat/legacy/serde/PKCS8导出；sha2/zeroize/Windows API版本复用现有锁。选择成熟2.x是为控制依赖/现有hash生态差异，不把库版本选择当安全保证；记录增量核验。

密钥用BCrypt系统随机产生，Windows DPAPI CurrentUser+UI_FORBIDDEN与固定应用用途entropy封装，禁止LOCAL_MACHINE或明文后备。生产仅使用KnownFolder LocalAppData下RemoteHub/identity-v1；目录当前用户DACL，已存在对象检查ACL/owner/reparse，密钥文件只含版本封装和DPAPI密文。跨进程锁与不覆盖发布避免并发生成不同身份；损坏/解密失败/未知格式不创建替代身份。

普通启动初始化，前端读取既有runtime IPC的公开状态（短ID/完整指纹），不增加capability、私钥导出或签名IPC；实验构建不触发身份文件。启动读取后桌面只缓存公开状态，私钥及时释放。现有Pinia代次和严格合同扩展为可选identity字段，不用旧fixture假充已生成。

不用机器作用域，因为会扩大可解密用户；不用用户名/MAC/硬件序列号作设备ID；不用明文/自制加密；不用通用签名IPC，避免前端成为签名oracle。未加入配对、信任库、云challenge、信令签名或恢复/旋转UI。

同用户恶意代码、管理员/内存读取、合法密文完整回滚不在本轮保证内；完整公钥才是后续信任绑定基础，短ID不认证。普通身份失败保持可用UI并明确不可用，不伪称远控可用。正常提交可回退，身份数据不能自动删。

依据：[DPAPI保护](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata)、[解密/完整性](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptunprotectdata)、[dalek2.2文档](https://docs.rs/ed25519-dalek/2.2.0/ed25519_dalek/)、[旧版本签名oracle通告](https://rustsec.org/advisories/RUSTSEC-2022-0093.html)、[curve修复门槛](https://rustsec.org/advisories/RUSTSEC-2024-0344.html)。公开资料核验仅本地实现设计依据，不是云授权。
