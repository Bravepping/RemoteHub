"""Package only an already-tested Phase3 executable and instructions. Never launches it."""
from pathlib import Path
import datetime,hashlib,json,os,shutil,subprocess,zipfile

ROOT=Path(__file__).resolve().parent.parent
OUT=ROOT/'artifacts/p301'
PRIVATE=ROOT/'artifacts/private/p3-delivery-v3'
README='''RemoteHub · Phase3 局域网最小远控 v3（Windows 10）

本包在产品五页界面提供一次性局域网主屏视频+实际基本键鼠控制。
每轮最长30秒，1280×720/30fps；只能指定一个私有IPv4对端，不接云、不自动重连。
普通用户运行EXE；不要求以管理员运行，不关闭防火墙，不改变系统设置。

准备：两台同局域网电脑都解压相同包。确认SHA256SUMS.txt所列EXE哈希一致。
IP只在两台本机界面填写，不发配对码或全量日志给聊天。先关闭旧实验程序。

第一轮（真实视频+鼠标+基本键盘）：
1. 在被控电脑先打开一个全新的空白Windows记事本，使用英文输入法，放在主屏可见区域。
   打开RemoteHub-P3-LAN.exe，选择“被控端：共享本机主屏”。
   填控制电脑的局域网IPv4，点“检测本机地址”，核对本机地址无误。
   勾选本次同意、点击等待配对，并亲自确认原生授权框。
2. 将本次配对码仅复制到你自己的控制电脑；被控端最多等120秒。
   在被控电脑把新空白记事本重新放到主屏前景，RemoteHub仍保持运行。
3. 控制电脑打开相同EXE，选择“控制端”，填被控电脑IPv4，检测/核对本机地址，
   填本次配对码，勾选同意、连接并亲自确认原生框。
4. 看到实时画面和“视频正在播放”后，主动点“启用本次键鼠控制”。
   在画面里的记事本文本区点击，输入 remotehubtest（不要输入密码或个人内容）。
   确认第二台真实记事本出现相同文字，鼠标点击位置对应；可滚轮一次。
   不要切换到其他程序/UAC/管理员或安全桌面；Win/Alt/F键组合本轮不支持。
5. 等本轮自动结束，核对两端资源回收已完成；保存程序旁artifacts/private/p3里的统计。
   不保存或发送实际桌面截图/录像；报告只需要脱敏统计与现场确认。

控制规则：点击启用后只有远程画面操作区获得焦点才接收基本按键。
Esc、失焦、移出画面或暂停会释放本次按住的键/按钮。显示区域黑边不发送坐标。
被控端随时可点“停止并回收”、关闭窗口或在本机按Ctrl+Alt+F12紧急结束。
有任何异常先停止；不要重复配对掩盖失败。拒绝/断线/超时不会改为后台采集。
若弹出系统网络访问/防火墙提示，先取消并报告；本包不自动安装放行规则。

后续短轮次由Agent按结果安排：控制端停止、被控端应急键、播放中关窗/断网，
每轮都重新配对并亲自确认。不要自行无限重试或连续采集。

本轮无持久信任库/正式服务端登录；一次码HMAC认证信令，真实视频/输入走WebRTC，
信令TCP固定端口48604，媒体UDP只允许选定对端。短ID不是认证凭据。
不支持公网上线、剪贴板/文件传输、Unicode文本批量注入、全系统快捷键或多显示器切换。
此包待真实双机验收，不因编译或自动测试就宣称远控已实测通过。
'''

def main():
    q=json.loads((OUT/'quality-results.json').read_text(encoding='utf-8-sig'))
    checks=json.loads((OUT/'product-checks.json').read_text(encoding='utf-8-sig'))
    if not q['passed'] or len(checks)!=5 or any(r['exitCode']!=0 for r in checks):raise ValueError('PACKAGE_CHECKS_INCOMPLETE')
    env=os.environ.copy();env.update(CARGO_HOME=str(ROOT/'.tools/cargo'),RUSTUP_HOME=str(ROOT/'.tools/rustup'))
    metadata=json.loads(subprocess.check_output([str(ROOT/'.tools/cargo/bin/cargo.exe'),'metadata','--format-version','1','--locked','--offline','--no-default-features','--features','remotehub-desktop/p3-lan,remotehub-desktop/custom-protocol'],cwd=ROOT,env=env,stderr=subprocess.PIPE))
    desktop=next(p for p in metadata['packages'] if p['name']=='remotehub-desktop')
    features=next(p['features'] for p in metadata['resolve']['nodes'] if p['id']==desktop['id'])
    if set(features)!={'p3-lan','p005-control','p004-lan','p004-probe','custom-protocol'}:raise ValueError('PACKAGE_FEATURES_REJECTED')
    PRIVATE.mkdir(parents=True,exist_ok=True)
    exe=PRIVATE/'RemoteHub-P3-LAN.exe'
    shutil.copyfile(ROOT/'target/probe/remotehub-desktop.exe',exe)
    digest=hashlib.sha256(exe.read_bytes()).hexdigest()
    (PRIVATE/'开始这里.txt').write_text(README,encoding='utf-8-sig',newline='\r\n')
    (PRIVATE/'SHA256SUMS.txt').write_text(digest+'  RemoteHub-P3-LAN.exe\n',encoding='utf-8',newline='\n')
    files=[ROOT/n for n in ['Cargo.toml','Cargo.lock','apps/desktop/src-tauri/Cargo.toml','crates/hub-webrtc/Cargo.toml','apps/desktop/src-tauri/build.rs','apps/desktop/src-tauri/capabilities/default.json','apps/desktop/src-tauri/tauri.conf.json']]
    files += [p for folder in ['apps/desktop/src','apps/desktop/src-tauri/src','crates/hub-webrtc/src'] for p in (ROOT/folder).rglob('*') if p.suffix in ['.ts','.vue','.css','.rs']]
    hashes={p.relative_to(ROOT).as_posix():hashlib.sha256(p.read_text(encoding='utf-8').encode('utf-8')).hexdigest() for p in sorted(files)}
    source_hash=hashlib.sha256(json.dumps(hashes,sort_keys=True).encode()).hexdigest()
    archive=PRIVATE.parent/'RemoteHub-Phase3-LAN-v3.zip'
    with zipfile.ZipFile(archive,'w',compression=zipfile.ZIP_DEFLATED,compresslevel=6)as z:
        for file in [exe,PRIVATE/'开始这里.txt',PRIVATE/'SHA256SUMS.txt']:z.write(file,file.name)
    with zipfile.ZipFile(archive)as z:
        if z.namelist()!=['RemoteHub-P3-LAN.exe','开始这里.txt','SHA256SUMS.txt'] or z.testzip() is not None:raise ValueError('PACKAGE_CONTENTS_REJECTED')
        if hashlib.sha256(z.read('RemoteHub-P3-LAN.exe')).hexdigest()!=digest:raise ValueError('PACKAGE_HASH_MISMATCH')
    result=dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),baselineCommit='382aac026bafae3b5ad5c74b6ee4ebf7508c235f',
        programSha256=digest,archiveSha256=hashlib.sha256(archive.read_bytes()).hexdigest(),programRelativePath=exe.relative_to(ROOT).as_posix(),archiveRelativePath=archive.relative_to(ROOT).as_posix(),
        sourceTreeSha256=source_hash,sourceFilesSha256=hashes,desktopFeatures=features,
        packageFileCount=3,privateConfigOrCredentialsIncluded=False,programStarted=False,twoPcTest='PENDING_USER_ACTION')
    (OUT/'package.json').write_text(json.dumps(result,indent=2)+'\n',encoding='utf-8',newline='\n')
    print(json.dumps({k:result[k] for k in ['programSha256','archiveSha256','programRelativePath','archiveRelativePath','twoPcTest']}))

if __name__=='__main__':
    try:main()
    except Exception:raise SystemExit('PHASE3_PACKAGE_FAILED_NO_PRIVATE_ECHO')from None
