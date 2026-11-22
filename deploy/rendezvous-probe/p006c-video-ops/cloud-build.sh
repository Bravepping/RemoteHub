set -eu
python3 - <<'PY'
import pathlib,tarfile,json,subprocess,hashlib
f=pathlib.Path('/opt/remotehub-p006c-video-20261122');old=pathlib.Path('/opt/remotehub-p006b-build/vendored-source-v2')
assert not (f/'src').exists();src=f/'src';src.mkdir(mode=0o700)
with tarfile.open(f/'source.tar') as t:
 members=[m for m in t.getmembers() if m.name.startswith(('crates/hub-probe-wire/','crates/hub-turn-credentials/','crates/hub-rendezvous-probe/'))]
 t.extractall(src,members=members,filter='data')
(src/'Cargo.toml').write_bytes((old/'Cargo.toml').read_bytes());(src/'Cargo.lock').write_bytes((old/'Cargo.lock').read_bytes())
(src/'.cargo').mkdir();(src/'.cargo/config.toml').write_text('[source.crates-io]\nreplace-with="vendored-sources"\n[source.vendored-sources]\ndirectory="'+str(old/'vendor')+'"\n')
print(json.dumps(dict(sourceArchiveSha256=hashlib.sha256((f/'source.tar').read_bytes()).hexdigest(),offline=True)))
PY
cd /opt/remotehub-p006c-video-20261122/src
export PATH=/opt/remotehub-p006b-build/toolchain/bin:$PATH
export CARGO_HOME=/opt/remotehub-p006b-build/cargo-home
export CARGO_TARGET_DIR=/opt/remotehub-p006b-build/target
export CARGO_BUILD_JOBS=1
export CARGO_PROFILE_RELEASE_LTO=false
nohup sh -c 'cargo test -p hub-rendezvous-probe --locked --offline > ../server-tests.txt 2>&1 && cargo build -p hub-rendezvous-probe --release --locked --offline > ../server-build.txt 2>&1; echo $? > ../build-exit.txt' </dev/null >/dev/null 2>&1 &
echo OFFLINE_BUILD_STARTED
