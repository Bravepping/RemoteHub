"""Audit local manual UI evidence; no app launch, UI operation or network access."""
import datetime
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
from network_privacy import check_files, check_text

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / 'artifacts/p106-manual'
BASE = 'ddbe191424c2a7379b43d81ac58f746ea32a6997'
ANSWERS = {
    'A1': '本轮实例已打开，保持窗口中',
    'A2': '已点 X，窗口已关闭，未重新打开',
    'B1': '已重新打开，驻留默认未勾选，保持窗口中',
    'B2': '窗口已隐藏，托盘图标仍在，未恢复或退出',
    'B3': '窗口正常恢复，重复显示后仍只有一个窗口',
    'B4': '取消驻留后窗口自动恢复，菜单已取消勾选',
    'B5': '已点 X，窗口关闭，未重新打开',
    'C1': '五页正常，重新检测成功，空态真实，保持窗口中',
    'C2': '隐藏正常，真正退出后窗口和托盘图标均消失',
}
DOCS = ['README.md', 'TASKS.md', 'docs/P106_MANUAL_ACCEPTANCE_PLAN.md', 'docs/P106_MANUAL_TEST_REPORT.md',
        'docs/PHASE1_FINAL_EXIT_REVIEW.md', 'docs/DEVELOPMENT_GUIDE.md', 'docs/PHASE1_DESKTOP_IMPLEMENTATION_PLAN.md',
        '.ai-memory/handoff.md', '.ai-memory/project_memory.md']
SOURCES = ['scripts/p106-manual-build.ps1', 'scripts/p106-manual-observe.ps1',
           'scripts/p106-manual-evidence.py', 'scripts/test_p106_manual_evidence.py']


def require(value):
    if not value: raise ValueError('MANUAL_EVIDENCE_REJECTED')


def load_bundle():
    def rows(name): return [json.loads(line) for line in (OUT/name).read_text(encoding='utf-8-sig').splitlines() if line]
    return dict(build=json.loads((OUT/'build.json').read_text(encoding='utf-8-sig')),
                users=rows('user-observations.jsonl'),
                processes={case: rows('case-'+case+'-process.jsonl') for case in 'ABC'})


def validate(bundle):
    build = bundle['build']
    require(build['sourceCommit'] == BASE and re.fullmatch('[0-9a-f]{64}', build['sha256']))
    require(build['desktopCargoFeatures'] == ['custom-protocol'] and build['ordinaryDesktop'] is True)
    require(build['buildStartedApplication'] is False and build['mediaOrCloudConfigIncluded'] is False)
    users = bundle['users']
    require(len(users) == len(ANSWERS) and {r['step'] for r in users} == set(ANSWERS))
    require(all(r['source'] == 'USER_RESPONSE' and r['answer'] == ANSWERS[r['step']] and r['case'] == r['step'][0] for r in users))
    instances = {}
    for case in 'ABC':
        records = bundle['processes'][case]
        require(not any(r['kind'] in ['OBSERVER_ERROR', 'OBSERVER_TIMEOUT'] for r in records))
        bound = [r for r in records if r['kind'] == 'BOUND']
        exits = [r for r in records if r['kind'] == 'EXIT_OBSERVED']
        require(len(bound) == len(exits) == 1)
        bound, exited = bound[0], exits[0]
        require(bound['namedInstanceCount'] == 1 and bound['sha256'] == build['sha256'])
        require(isinstance(bound['processId'], int) and bound['processId'] > 0)
        require(exited['startedUtc'] == bound['startedUtc'])
        require(all(r['case'] == case and r['processId'] == bound['processId'] for r in records))
        require(exited['remainingNamedInstanceCount'] == 0 and exited['agentTerminatedApplication'] is False)
        require(exited['exitCode'] is None or exited['exitCode'] == 0)
        # A's original schema lacked this field. EXIT_OBSERVED was emitted only after WaitForExit.
        require(exited.get('processWaitSignaled', True) is True)
        if 'exitCodeAvailable' in exited:
            require(exited['exitCodeAvailable'] == (exited['exitCode'] is not None))
        after = [r for r in records if r['kind'] == 'SNAPSHOT' and r['stage'] == 'after-exit']
        require(len(after) == 1 and after[0]['state'] == 'NOT_FOUND_IN_PROCESS_ENUMERATION' and after[0]['namedInstanceCount'] == 0)
        require(datetime.datetime.fromisoformat(bound['recordedAt']) < datetime.datetime.fromisoformat(exited['recordedAt']) <= datetime.datetime.fromisoformat(after[0]['recordedAt']))
        instances[case] = dict(processId=bound['processId'], startedUtc=bound['startedUtc'],
                               exitedAt=exited['recordedAt'], exitCode=exited['exitCode'],
                               exitCodeVerdict='UNAVAILABLE' if exited['exitCode'] is None else 'AVAILABLE',
                               exitSignalObserved=True, remainingInstances=0)
    require(len({r['processId'] for r in instances.values()}) == 3)
    for stage in ['hidden', 'restored', 'residency-disabled']:
        rows = [r for r in bundle['processes']['B'] if r['kind'] == 'SNAPSHOT' and r['stage'] == stage]
        require(len(rows) == 1 and rows[0]['state'] == 'RUNNING_BOUND_INSTANCE' and rows[0]['namedInstanceCount'] == 1)
    pages = [r for r in bundle['processes']['C'] if r['kind'] == 'SNAPSHOT' and r['stage'] == 'five-pages']
    require(len(pages) == 1 and pages[0]['state'] == 'RUNNING_BOUND_INSTANCE' and pages[0]['namedInstanceCount'] == 1)
    return dict(nativeLifecycle='USER_MANUAL_PASS_WITH_PROCESS_EXIT_EVIDENCE', lifecycleScenarios=7,
                nativeBasicPages='USER_MANUAL_PASS', pages=['home', 'devices', 'connect', 'sessions', 'settings'],
                readonlyRefresh='USER_MANUAL_PASS', instances=instances,
                windowVisibilitySource='USER_RESPONSE_ONLY; process metadata is not visibility proof',
                exitCodeZeroClaimed=False, allNativeErrorStatesTested=False,
                phase1Exit='AWAITING_USER_REVIEW', phase2Started=False)


def main():
    result = validate(load_bundle())
    git = lambda *args: subprocess.check_output(['git', '-c', 'safe.directory='+ROOT.as_posix(), *args], cwd=ROOT, stderr=subprocess.PIPE)
    require(not git('diff', '--name-only', BASE, '--', 'apps', 'crates', 'Cargo.toml', 'Cargo.lock', 'pnpm-lock.yaml',
                    'package.json', 'scripts/p106-evidence.py', 'artifacts/p106', 'artifacts/p105',
                    'docs/P106_TEST_REPORT.md', 'docs/P105_TEST_REPORT.md', 'docs/PHASE1_EXIT_REVIEW_PREPARATION.md').strip())
    changed = set(git('diff', '--name-only', BASE).decode('utf-8').splitlines())
    changed.update(git('ls-files', '--others', '--exclude-standard').decode('utf-8').splitlines())
    require(all(n in DOCS+SOURCES or n.startswith('artifacts/p106-manual/') for n in changed))
    binary = ROOT/'artifacts/private/p106-manual/RemoteHub-P106-Manual.exe'
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    require(digest == load_bundle()['build']['sha256'])
    cleanup = json.loads((OUT/'cleanup.json').read_text(encoding='utf-8-sig'))
    require(cleanup['remainingTestProcesses'] == 0 and cleanup['observerSessionsCompleted'] == 3)
    require(cleanup['agentTerminatedApplication'] is False and cleanup['syntaxErrors'] == 0)
    regression = json.loads((OUT/'regression/results.json').read_text(encoding='utf-8-sig'))
    require(regression['passed'] and all(r['exitCode'] == 0 for r in regression['results']))
    tasks = (ROOT/'TASKS.md').read_text(encoding='utf-8')
    require(len(re.findall(r'^- \[x\] `P1-0[1-6]`', tasks, re.M)) == 6)
    require(not re.search(r'^- \[x\] `P2-', tasks, re.M))
    review = (ROOT/'docs/PHASE1_FINAL_EXIT_REVIEW.md').read_text(encoding='utf-8')
    require('READY_FOR_USER_REVIEW / NOT_APPROVED' in review and 'OPEN_NOT_REPRODUCED' in review)
    for p in [ROOT/n for n in SOURCES if n.endswith('.py')]: check_files([p], source=True)
    check_files([ROOT/n for n in DOCS+SOURCES if not n.endswith('.py')])
    files = [p for p in OUT.rglob('*') if p.suffix in ['.json', '.jsonl', '.txt'] and p.name != 'verification.json']
    check_files(files)
    links = 0
    for name in DOCS:
        path = ROOT/name
        for url in re.findall(r'\[[^\]]+\]\(([^)]+)\)', path.read_text(encoding='utf-8')):
            if re.match(r'^[a-z]+:', url) or url.startswith('#'): continue
            target = (path.parent/url.strip('<>').split('#')[0]).resolve(); target.relative_to(ROOT)
            require(target.exists() or target == OUT/'verification.json'); links += 1
    git('diff', '--check')
    result.update(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),
                  passed=True, testedApplicationCommit=BASE, programSha256=digest,
                  applicationSourcesPermissionsMediaAndLocksUnchanged=True, historicalEvidenceUnchanged=True,
                  agentStartedApplication=False, desktopCapture=False, remoteInput=False, cloud=False,
                  twoPcStreaming=False, thirdPartyBackendStarted=False, systemConfigChanged=False,
                  oldProbeRootCause='NOT_DETERMINED', R06='OPEN_NOT_REPRODUCED', c0='PENDING_USER_DECISIONS',
                  relativeLinksChecked=links, evidenceHashFormat='SHA256 of UTF-8 LF text without BOM',
                  textEvidenceSha256={p.relative_to(OUT).as_posix(): hashlib.sha256(p.read_text(encoding='utf-8-sig').encode('utf-8')).hexdigest() for p in files})
    check_text(json.dumps(result))
    (OUT/'verification.json').write_text(json.dumps(result, indent=2)+'\n', encoding='utf-8', newline='\n')
    print(json.dumps({k: result[k] for k in ['passed', 'nativeLifecycle', 'nativeBasicPages', 'phase1Exit', 'phase2Started']}))


if __name__ == '__main__':
    try: main()
    except Exception:
        print('MANUAL_EVIDENCE_FAILED_NO_PRIVATE_ECHO', file=sys.stderr)
        raise SystemExit(1) from None
