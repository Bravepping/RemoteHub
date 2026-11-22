"""Pure operation manifest generator: never runs commands or reads cloud credentials."""
import json
import re
import sys

WSS_BINARY = '/opt/remotehub-p006c-manual-20261122/hub-rendezvous-probe'


def plan(run_id, now_unix, authorized_at_unix):
    if not re.fullmatch(r'\d{8}T\d{6}Z', run_id) or type(now_unix) is not int or type(authorized_at_unix) is not int:
        raise ValueError('R06_PLAN_INPUT')
    deadline = authorized_at_unix + 1200
    guard_at = deadline - 60
    remaining = guard_at - now_unix
    if not 60 <= remaining <= 1140:
        raise ValueError('R06_PLAN_EXPIRED_OR_CLOCK_INVALID')
    folder = '/run/remotehub-p007b-r06-' + run_id
    guard = ['systemd-run', '--unit=remotehub-p007b-r06-expiry', '--on-active=' + str(remaining) + 's',
             '--timer-property=AccuracySec=1s', '--collect', '/usr/bin/python3', folder + '/expiry_cleanup.py',
             '--execute-after-authorization', folder, 'deadline']
    wss = ['systemd-run', '--unit=remotehub-p007b-r06-wss', '--collect', '-p', 'User=remotehub-p006b',
           '-p', 'Group=remotehub-p006b', '-p', 'EnvironmentFile=/etc/remotehub-p006b/issuer.env',
           '-p', 'RuntimeMaxSec=' + str(remaining) + 's', '-p', 'Restart=no', '-p', 'UMask=0077',
           '-p', 'NoNewPrivileges=yes', '-p', 'ProtectSystem=strict', '-p', 'ProtectHome=yes', '-p', 'PrivateTmp=yes',
           '-p', 'MemoryMax=192M', '-p', 'CPUQuota=100%', WSS_BINARY, '--private-config', folder + '/server.json']
    observer = ['systemd-run','--unit=remotehub-p007b-r06-observer','--collect','-p','RuntimeMaxSec='+str(remaining)+'s',
                '-p','Restart=no','-p','UMask=0077','/usr/bin/python3',folder+'/r06_observer.py','--execute-after-authorization',folder]
    return dict(version=1, runDirectory=folder, authorizationStart=authorized_at_unix, authorizationDeadline=deadline,
                guardTarget=guard_at, maxConcurrentAllocations=4, maxConcurrentSessions=1, maxRounds=2, syntheticSeconds=8,
                lastNewAllocationAdmission=authorized_at_unix + 480,
                commandsAreNotExecuted=True, order=['create_scoped_root_owned_directory_and_stage_guard',
                'arm_guard_and_verify_timer_before_credentials_or_services', 'generate_fresh_ip_san_tls_and_short_invitations',
                'start_existing_turn_and_same_pid_observer', 'start_transient_wss', 'verify_local_tls_and_give_config_to_two_users'],
                expiryTimerCommand=guard, startTurnCommand=['systemctl', 'start', 'remotehub-turn-g6.service'],
                startWssCommand=wss, manualCleanupCommand=['/usr/bin/python3', folder + '/expiry_cleanup.py', '--execute-after-authorization', folder, 'manual'],
                startObserverCommand=observer,
                observerUnit='remotehub-p007b-r06-observer.service', observerMaxAllocations=4, observerSplitSharedCredentials=True,
                regenerateRemainingBeforeEachStart=True,
                prerequisites=['explicit_user_cloud_start_authorization', 'read_only_baseline_rechecked',
                               'guard_hash_checked_and_timer_active', 'no_second_round_until_previous_allocations_zero'])


if __name__ == '__main__':
    try:
        if len(sys.argv) != 5 or sys.argv[1] != '--plan-only':
            raise ValueError('R06_PLAN_ONLY_ARGUMENTS')
        print(json.dumps(plan(sys.argv[2], int(sys.argv[3]), int(sys.argv[4])), indent=2))
    except Exception:
        raise SystemExit('R06_PLAN_REJECTED_NO_PRIVATE_ECHO') from None
