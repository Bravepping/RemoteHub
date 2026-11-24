#!/usr/bin/env bash
# Explicit, disposable operator test only. Never enables a system service.
set -euo pipefail
umask 077
die() { printf '%s\n' "$1" >&2; exit 1; }
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
turn=remotehub-turn-g6.service
turn_config=/etc/remotehub-turn-g6/turnserver.conf
turn_override_dir="/run/systemd/system/$turn.d"
turn_override="$turn_override_dir/99-remotehub-batch-runtime.conf"
[[ $EUID == 0 && $root =~ ^/root/[A-Za-z0-9._/-]+$ ]] || die 'Run as root from a private test directory under /root.'
cd -- "$root"
[[ $(stat -c '%u:%a' .) == 0:700 ]] || die 'Test directory must be owned by root with mode 700.'
valid_dir() { [[ $1 =~ ^/root/[A-Za-z0-9._/-]+$ && $(realpath -- "$1") == "$1" ]]; }
valid_unit() { [[ $1 =~ ^rh-pctest-[0-9]{8}-[0-9]{6}-[0-9]+\.service$ ]]; }
override_contents() { printf '# RemoteHub test: %s\n[Service]\nRuntimeMaxSec=1800s\n' "$1"; }

owns_override() {
    [[ ! -L $turn_override && -f $turn_override ]] &&
        cmp -s "$turn_override" <(override_contents "$1")
}

remove_own_override() {
    if owns_override "$1"; then
        rm -- "$turn_override"
        systemctl daemon-reload
    fi
}

# Do not acquire the operator lock in ExecStopPost: the operator may be
# waiting synchronously for our transient WSS unit to stop.
if [[ ${1:-} == __finish ]]; then
    [[ $# == 2 ]] && valid_unit "$2" || die 'Invalid test service.'
    [[ -f .public-control-unit && $(< .public-control-unit) == "$2" ]] || exit 0
    systemctl stop "$turn"
    remove_own_override "$2"
    exit 0
fi

# Internal systemd child only. Its env file was generated with an exclusive
# publication step and is never posted to either desktop.
if [[ ${1:-} == __serve ]]; then
    [[ $# == 2 && $2 =~ ^session-[0-9]{8}-[0-9]{6}-[0-9]+$ ]] || die 'Invalid private session.'
    [[ -f $2/wss.env && ! -L $2/wss.env && $(stat -c '%u:%a' "$2/wss.env") == 0:600 ]] ||
        die 'Private session unavailable.'
    (set -o noclobber; : > "$2/.started") 2>/dev/null || die 'This batch was already activated. Generate a fresh one.'
    source "$2/wss.env"
    if [[ -z ${REMOTEHUB_V2_PUBLIC_CONTROL_BATCH_FILE:-} ]]; then
        export REMOTEHUB_V2_PUBLIC_CONTROL_EXIT_AFTER_ROOM=1
    fi
    exec "$root/v2_signaling"
fi

exec 8>.public-control.lock
flock -n 8 || die 'Another start or stop is already in progress.'

stop_ours() {
    [[ -e .public-control-unit ]] || return 0
    local unit
    IFS= read -r unit <.public-control-unit
    valid_unit "$unit" || die 'Invalid test service record.'
    if [[ $(systemctl show "$unit" -p LoadState --value) == loaded ]]; then
        systemctl stop "$unit"
        systemctl stop "$turn"
    elif owns_override "$unit"; then
        systemctl stop "$turn"
    elif systemctl is-active --quiet "$turn"; then
        die 'TURN is active without a loaded test service; stop the previous test with its STOP_CMD first.'
    fi
    remove_own_override "$unit"
    rm -- .public-control-unit
}

case ${1:-} in
    configure)
        shift
        [[ $# = 9 ]] || die 'Usage: configure CERT_DIR PUBLIC_IP BIND_IP HOST_LOCAL HOST_EGRESS VIEWER_LOCAL VIEWER_EGRESS HOST_FP VIEWER_FP'
        valid_dir "$1" || die 'Use the exact absolute path of the private certificate directory.'
        for value in "${@:2:6}"; do
            [[ $value =~ ^[0-9]{1,3}(\.[0-9]{1,3}){3}$ ]] || die 'Use literal IPv4 addresses.'
        done
        for value in "${@:8:2}"; do
            [[ $value =~ ^[0-9a-f]{64}$ ]] || die 'Use the complete lowercase device fingerprints.'
        done
        tmp=$(mktemp .public-control-settings.XXXXXXXX)
        printf '%s\n' "$@" >"$tmp"
        mv -fT -- "$tmp" .public-control-settings
        echo 'Configured. For the 30-minute batch, explicitly opt in with: new --align-turn-runtime'
        exit 0
        ;;
    stop)
        [[ $# = 1 ]] || die 'Usage: stop'
        stop_ours
        echo 'Tracked test is stopped.'
        exit 0
        ;;
    new)
        [[ $# == 2 && $2 == --align-turn-runtime ]] ||
            die 'For the 30-minute batch, use new --align-turn-runtime to allow a temporary TURN runtime override.'
        ;;
    one)
        [[ $# == 1 ]] || die 'Usage: one (one round)'
        ;;
    *) die 'Choose configure, new, one, or stop.' ;;
esac

[[ -f .public-control-settings && ! -L .public-control-settings ]] || die 'Configure this test directory first.'
mapfile -t setting <.public-control-settings
[[ ${#setting[@]} == 9 ]] || die 'Invalid settings.'
cert=${setting[0]}
public_ip=${setting[1]}
bind_ip=${setting[2]}
valid_dir "$cert" || die 'Invalid certificate directory.'
[[ -x ./v2_signaling && -f ./prepare-public-video-test.py ]] || die 'Offline server bundle is incomplete.'
openssl verify -CAfile "$cert/ca.pem" -purpose sslserver -verify_ip "$public_ip" "$cert/wss.pem"
ttl=600
format=()
if [[ $1 == new ]]; then
    ttl=1800
    format=(--batch --all-ready)
fi
openssl x509 -in "$cert/wss.pem" -noout -checkend "$((ttl + 60))"

tag="$(date -u +%Y%m%d-%H%M%S)-$$"
p="session-$tag"
w="rh-pctest-$tag.service"
python3 ./prepare-public-video-test.py --purpose public-control "${format[@]}" --ttl 600 --out-dir "$root/$p" \
    --wss "$public_ip:8443" --bind "$bind_ip:8443" --turn "$public_ip:3478" --relay-ports 49160:49259 \
    --host-local-ip "${setting[3]}" --host-egress-ip "${setting[4]}" \
    --viewer-local-ip "${setting[5]}" --viewer-egress-ip "${setting[6]}" \
    --host-fingerprint "${setting[7]}" --viewer-fingerprint "${setting[8]}" \
    --ca "$cert/ca.pem" --tls-cert "$cert/wss.pem" --tls-key "$cert/wss.key" --turn-config "$turn_config"

stop_ours
case "$(systemctl show "$turn" -p ActiveState --value)" in
    inactive|failed) ;;
    *) die 'TURN is already running; stop the previous test with its STOP_CMD first.' ;;
esac
[[ -z $(ss -H -lntu '( sport = :8443 or sport = :3478 )') ]] || die 'A test port is occupied; nothing was started.'

cleanup() {
    systemctl stop "$w" 2>/dev/null || true
    if [[ $turn_started == yes ]]; then
        systemctl stop "$turn" 2>/dev/null || true
    fi
    remove_own_override "$w" || true
}
turn_started=no
trap cleanup EXIT
printf '%s\n' "$w" >.public-control-unit
if [[ $1 == new ]]; then
    mkdir -p -- "$turn_override_dir"
    (set -o noclobber; override_contents "$w" >"$turn_override") ||
        die 'Existing TURN runtime override was not changed.'
    systemctl daemon-reload
    [[ $(LC_ALL=C systemctl show "$turn" -p RuntimeMaxUSec --value) == 30min ]] ||
        die 'Effective TURN runtime is not 30 minutes; nothing was started.'
    [[ $(systemctl show "$turn" -p Restart --value) == no ]] ||
        die 'The dedicated TURN test service must not restart on its own.'
fi
turn_started=yes
systemctl start "$turn"
systemd-run --collect --unit="$w" --property=Type=exec --property="RuntimeMaxSec=$ttl" \
    --property=NoNewPrivileges=yes \
    --property="ExecStopPost=/bin/bash $root/public-control-test.sh __finish $w" \
    /bin/bash "$root/public-control-test.sh" __serve "$p"
sleep 1
printf 'WSS: '; systemctl is-active "$w"
printf 'TURN: '; systemctl is-active "$turn"
printf 'SESSION_DIR=%s\n' "$root/$p"
trap - EXIT
