"""Explicit GitHub prerelease upload; credentials stay in process memory, never in reports."""
import argparse
import hashlib
import json
import os
import pathlib
import subprocess
import urllib.error
import urllib.parse
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
REPO = 'Bravepping/RemoteHub'
TAG = 'r06-diag-v2-20261122'
API = 'https://api.github.com/repos/' + REPO


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def credentials():
    env = dict(os.environ, GIT_TERMINAL_PROMPT='0', GCM_INTERACTIVE='never')
    p = subprocess.run(['git', '-c', 'safe.directory=' + ROOT.as_posix(), 'credential', 'fill'], cwd=ROOT,
                       input='protocol=https\nhost=github.com\n\n', capture_output=True, text=True, env=env, timeout=20)
    values = dict(line.split('=', 1) for line in p.stdout.splitlines() if '=' in line)
    if p.returncode or not values.get('password'):
        raise ValueError('GITHUB_EXISTING_CREDENTIAL_UNAVAILABLE')
    return values['password']


def main(inspect_only=False):
    token = credentials()
    opener = urllib.request.build_opener(NoRedirect)

    def request(url, data=None, accept='application/vnd.github+json', content='application/json'):
        parsed = urllib.parse.urlparse(url)
        if parsed.scheme != 'https' or parsed.hostname not in ['api.github.com', 'uploads.github.com']:
            raise ValueError('GITHUB_HOST_POLICY')
        headers = {'Authorization': 'Bearer ' + token, 'Accept': accept, 'User-Agent': 'RemoteHub-R06-Delivery',
                   'X-GitHub-Api-Version': '2026-03-10', 'Content-Type': content}
        return opener.open(urllib.request.Request(url, data=data, headers=headers), timeout=60)

    repo = json.load(request(API))
    if inspect_only:
        print(json.dumps({'repository': REPO, 'private': repo['private'], 'pushPermission': repo.get('permissions', {}).get('push', False)}))
        return
    delivery = json.loads((ROOT / 'artifacts/p007b-privacy/delivery.json').read_text())
    archive = ROOT.parent / delivery['archiveName']
    if archive.parent != ROOT.parent or hashlib.sha256(archive.read_bytes()).hexdigest() != delivery['archiveSha256']:
        raise ValueError('GITHUB_ARCHIVE_HASH_REQUIRED')
    verification = json.loads((ROOT / 'artifacts/p007b-privacy/package-verification.json').read_text())
    if verification['archiveSha256'] != delivery['archiveSha256'] or not verification['crcVerified']:
        raise ValueError('GITHUB_PACKAGE_VERIFICATION_REQUIRED')
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    remote = subprocess.check_output(['git', '-c', 'http.sslBackend=openssl', 'ls-remote', 'origin', 'refs/heads/main'], cwd=ROOT, text=True).split()[0]
    if remote != revision:
        raise ValueError('GITHUB_SOURCE_PUSH_REQUIRED')
    try:
        release = json.load(request(API + '/releases/tags/' + TAG))
    except urllib.error.HTTPError as e:
        if e.code != 404:
            raise
        release = json.load(request(API + '/releases', json.dumps(dict(
            tag_name=TAG, target_commitish=revision, name='R06 diagnostic v2 - inactive synthetic-only privacy fix',
            body='Inactive diagnostic preparation only. Synthetic NV12, 8 seconds, native manual consent and strict authenticated ICE checks retained. No cloud or physical two-PC test authorized or executed. R06 remains OPEN; Phase 0 HOLD.\n\nZIP SHA256: ' + delivery['archiveSha256'] + '\nProgram SHA256: ' + delivery['programSha256'],
            draft=False, prerelease=True, make_latest='false')).encode()))
    sums = (delivery['archiveSha256'] + '  ' + archive.name + '\n' + delivery['programSha256'] + '  RemoteHub-R06.exe\n').encode()
    assets = []
    for name, data, content in [(archive.name, archive.read_bytes(), 'application/zip'), ('SHA256SUMS.txt', sums, 'text/plain')]:
        existing = [a for a in release.get('assets', []) if a['name'] == name]
        if existing:
            asset = existing[0]
            if asset.get('digest') != 'sha256:' + hashlib.sha256(data).hexdigest():
                raise ValueError('GITHUB_EXISTING_ASSET_DIFFERS_NO_OVERWRITE')
        else:
            url = release['upload_url'].split('{', 1)[0] + '?' + urllib.parse.urlencode({'name': name})
            asset = json.load(request(url, data, content=content))
        # Download actual bytes; on signed redirect never forward the GitHub token.
        try:
            response = request(asset['url'], accept='application/octet-stream')
        except urllib.error.HTTPError as e:
            if e.code != 302:
                raise
            location = e.headers.get('Location', '')
            parsed = urllib.parse.urlparse(location)
            if parsed.scheme != 'https' or not (parsed.hostname == 'github.com' or parsed.hostname.endswith('.githubusercontent.com')):
                raise ValueError('GITHUB_DOWNLOAD_REDIRECT_POLICY')
            response = opener.open(urllib.request.Request(location, headers={'User-Agent': 'RemoteHub-R06-Delivery'}), timeout=60)
        downloaded = response.read()
        if hashlib.sha256(downloaded).digest() != hashlib.sha256(data).digest():
            raise ValueError('GITHUB_DOWNLOAD_HASH_MISMATCH')
        if not repo['private']:
            # Verify the actual public URL given to the user, without a token.
            public_url = asset['browser_download_url']
            for _ in range(4):
                parsed = urllib.parse.urlparse(public_url)
                if parsed.scheme != 'https' or not (parsed.hostname == 'github.com' or parsed.hostname.endswith('.githubusercontent.com')):
                    raise ValueError('GITHUB_PUBLIC_DOWNLOAD_HOST_POLICY')
                try:
                    public_data = opener.open(urllib.request.Request(public_url, headers={'User-Agent': 'RemoteHub-R06-Delivery'}), timeout=60).read()
                    break
                except urllib.error.HTTPError as e:
                    if e.code not in [301, 302, 303, 307, 308]:
                        raise
                    public_url = e.headers.get('Location', '')
            else:
                raise ValueError('GITHUB_PUBLIC_DOWNLOAD_REDIRECT_LIMIT')
            if hashlib.sha256(public_data).digest() != hashlib.sha256(data).digest():
                raise ValueError('GITHUB_PUBLIC_DOWNLOAD_HASH_MISMATCH')
        assets.append(dict(name=asset['name'], assetId=asset['id'], url=asset['browser_download_url'],
                           bytes=len(downloaded), sha256=hashlib.sha256(downloaded).hexdigest(), actualDownloadVerified=True,
                           publicUrlWithoutCredentialsVerified=not repo['private']))
    result = dict(repository=REPO, private=repo['private'], tag=TAG, releaseUrl=release['html_url'],
                  sourceRevision=revision, assets=assets, prerelease=True, newCredentialFileCreated=False,
                  cloudServicesStarted=False, physicalTwoPcTest='NOT_RUN')
    (ROOT / 'artifacts/p007b-privacy/published.json').write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(result))


if __name__ == '__main__':
    try:
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument('--inspect', action='store_true')
        args = parser.parse_args()
        main(args.inspect)
    except Exception:
        raise SystemExit('GITHUB_RELEASE_OPERATION_FAILED_NO_SECRET_ECHO') from None
