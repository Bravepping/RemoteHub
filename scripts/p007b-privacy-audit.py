"""Read-only Git exposure audit. Suspected endpoints stay in memory, never in output."""
import ast
import ipaddress
import json
import pathlib
import subprocess
import zipfile
import datetime

ROOT = pathlib.Path(__file__).resolve().parent.parent
BASELINE = '47f395815b62c7b0fcf0ab3f28f640f849e0e74b'
OUT = ROOT / 'artifacts/p007b-privacy'


def git(args, data=None, allow_empty=False):
    p = subprocess.run(['git', *args], cwd=ROOT, input=data, capture_output=True)
    if p.returncode not in ([0, 1] if allow_empty else [0]):
        raise ValueError('EXPOSURE_GIT_READ_FAILED')
    return p.stdout


def suspected_values():
    # Recover only the old list in memory. Never log it or pass it as process arguments.
    tree = ast.parse(git(['show', BASELINE + ':scripts/p007b-evidence.py']).decode('utf-8'))
    for node in ast.walk(tree):
        if isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id == 'forbidden' for t in node.targets):
            values = ast.literal_eval(node.value)
            result = []
            for value in values:
                try:
                    address = ipaddress.ip_address(value)
                except ValueError:
                    continue
                result.append((value.encode(), 'private' if address.is_private else 'public_or_other'))
            return result
    raise ValueError('EXPOSURE_BASELINE_LIST_NOT_FOUND')


def safe_path(path):
    from network_privacy import check_text
    check_text(path)
    return path


def audit():
    values = suspected_values()
    patterns = b'\n'.join(v for v, _ in values) + b'\n'
    commits = git(['rev-list', '--all', '--reverse']).decode().splitlines()
    remote_main = git(['rev-parse', 'origin/main']).decode().strip()
    remote_commits = set(git(['rev-list', remote_main]).decode().splitlines())
    affected = []
    path_commits = {}
    for commit in commits:
        matches = git(['grep', '-I', '-l', '-F', '-f', '-', commit, '--'], patterns, True).decode().splitlines()
        if matches:
            paths = sorted(safe_path(m.split(':', 1)[1]) for m in matches)
            affected.append({'commit': commit, 'paths': paths, 'reachableFromFetchedOriginMain': commit in remote_commits})
            for path in paths:
                path_commits.setdefault(path, []).append(commit)
    current = []
    for raw in git(['ls-files', '-z']).split(b'\0'):
        if not raw:
            continue
        name = raw.decode()
        if any(value in (ROOT / name).read_bytes() for value, _ in values):
            current.append(safe_path(name))
    old = json.loads((ROOT / 'artifacts/p007b/delivery.json').read_text(encoding='utf-8'))
    archive = ROOT.parent / old['archiveName']
    zip_matches = []
    with zipfile.ZipFile(archive) as z:
        for member in z.namelist():
            if any(value in z.read(member) for value, _ in values):
                zip_matches.append(safe_path(member))
    result = dict(scope='locally reachable Git history + tracked worktree + previous inactive ZIP, exact baseline suspects only',
                  recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),
                  baselineCommit=BASELINE, fetchedOriginMainAtAudit=remote_main,
                  affectedSnapshotsReachableFromFetchedOriginMain=sum(a['reachableFromFetchedOriginMain'] for a in affected),
                  suspectedLiteralCount=len(values),
                  suspectedClasses={k: sum(c == k for _, c in values) for k in sorted({c for _, c in values})},
                  reachableCommitsChecked=len(commits), affectedCommits=affected,
                  affectedPaths=[dict(path=p, firstReachableCommit=cs[0], lastReachableCommit=cs[-1], affectedSnapshots=len(cs)) for p, cs in sorted(path_commits.items())],
                  currentTrackedPaths=sorted(current), previousZip=dict(archiveName=old['archiveName'], membersChecked=8, matchingMembers=zip_matches),
                  historyRewritten=False, oldObjectsStillReachable=bool(affected),
                  limitations=['not a complete credential or endpoint inventory', 'no hosting access logs/forks/clones/caches audit',
                               'absence from the previous ZIP does not remove Git exposure', 'no hashes or encoded forms of suspect values exported'])
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / 'exposure-audit.json').write_text(json.dumps(result, indent=2, ensure_ascii=False) + '\n', encoding='utf-8')
    print(json.dumps(dict(reachableCommitsChecked=len(commits), affectedCommitCount=len(affected), affectedPathCount=len(path_commits),
                          currentTrackedPathCount=len(current), previousZipMatchingMembers=len(zip_matches), historyRewritten=False)))


if __name__ == '__main__':
    try:
        audit()
    except Exception:
        raise SystemExit('EXPOSURE_AUDIT_FAILED_NO_INPUT_ECHO') from None
