"""Build an unchanged, published stable Hubu release in a separate private cache.

The release identity comes from GitHub's published release metadata, never from
the caller: the release must be a stable (non-draft, non-prerelease) release
whose notes name its full source commit, and the cloned tag must resolve to
exactly that commit. Each release gets its own source and build cache.
"""
import json
import fcntl
import os
from pathlib import Path
import re
import subprocess
import tempfile
import urllib.request

REPOSITORY = 'https://github.com/hacker-no-ice/hubu.git'
RELEASES_API = 'https://api.github.com/repos/hacker-no-ice/hubu/releases/tags/'
# Executor contracts these kits' requests are written against. A release that
# reports another contract is not silently exercised with mismatched requests.
SUPPORTED_CONTRACTS = frozenset({'hubu-spend-executor-v4.5'})
STABLE_TAG = re.compile(r'v\d+\.\d+\.\d+')
SOURCE_COMMIT = re.compile(r'Source commit: `([0-9a-f]{40})`')


def build(release, cache=None):
    if not STABLE_TAG.fullmatch(release or ''):
        raise ValueError(f'--release must name a stable tag such as v0.2.5; got {release!r}')
    cache = Path(cache or Path(tempfile.gettempdir()) / f'break-hubu-build-{os.getuid()}').resolve()
    cache.mkdir(mode=0o700, parents=True, exist_ok=True)
    # Separate invocations may share the source/build cache, never their servers or data.
    with (cache / 'build.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        return _build(release, cache / release)


def published_release(release):
    headers = {'User-Agent': 'break-hubu-public-proof-kit', 'Accept': 'application/vnd.github+json'}
    token = os.environ.get('GITHUB_TOKEN')
    if token:
        headers['Authorization'] = 'Bearer ' + token
    req = urllib.request.Request(RELEASES_API + release, headers=headers)
    with urllib.request.urlopen(req, timeout=30) as response:
        return json.load(response)


def pinned_commit(published, release):
    if published.get('tag_name') != release or published.get('draft') or published.get('prerelease'):
        raise RuntimeError(f'{release} is not a published stable release')
    commits = set(SOURCE_COMMIT.findall(published.get('body') or ''))
    if len(commits) != 1:
        raise RuntimeError(f'Published {release} notes must name exactly one full source commit')
    return commits.pop()


def _build(release, cache):
    cache.mkdir(mode=0o700, parents=True, exist_ok=True)
    source, target = cache / 'source', cache / 'target'
    log = cache / 'build.log'
    # Keep download/build environment separate from all active Hubu/Gongbu settings.
    env = {k: v for k, v in os.environ.items() if k in
           ('PATH', 'HOME', 'TMPDIR', 'RUSTUP_HOME', 'CARGO_HOME', 'SDKROOT', 'DEVELOPER_DIR')}
    def run(args, cwd=None):
        with log.open('a') as stream:
            stream.write('$ ' + ' '.join(map(str, args)) + '\n'); stream.flush()
            subprocess.run(list(map(str, args)), cwd=cwd, env=env, stdout=stream,
                           stderr=subprocess.STDOUT, check=True)
    published_path = cache / 'published-release.json'
    if not published_path.exists():
        published = published_release(release)
        pinned_commit(published, release)
        pending = published_path.with_suffix('.pending')
        pending.write_text(json.dumps(published, indent=2) + '\n')
        pending.replace(published_path)
    published = json.loads(published_path.read_text())
    commit = pinned_commit(published, release)
    if not source.exists():
        run(['git', 'clone', '--branch', release, '--single-branch', REPOSITORY, source])
    def git(*args):
        return subprocess.check_output(['git', '-C', str(source), *args], env=env, text=True).strip()
    if git('rev-parse', 'HEAD') != commit or git('rev-parse', f'{release}^{{commit}}') != commit:
        raise RuntimeError(f'Cache is not the exact {release} release checkout at {commit}: {source}')
    if git('status', '--porcelain', '--untracked-files=all'):
        raise RuntimeError(f'Release source cache is dirty: {source}')
    toolchain = re.search(r'channel\s*=\s*"([\d.]+)"', (source / 'rust-toolchain.toml').read_text()).group(1)
    # Do not silently install a toolchain. This also gives a useful prerequisite error.
    run(['rustup', 'run', toolchain, 'cargo', '--version'])
    env.update(HUBU_PRODUCT_VERSION=release, HUBU_SOURCE_COMMIT=commit,
               GONGBU_PRODUCT_VERSION=release, GONGBU_SOURCE_COMMIT=commit)
    command = ['rustup', 'run', toolchain, 'cargo', 'build', '--release', '--locked',
               '--target-dir', str(target), '--bin', 'hubu', '--bin', 'hubu-server']
    run(command, source)
    versions = {}
    for name in ('hubu', 'hubu-server'):
        versions[name] = json.loads(subprocess.check_output([str(target / 'release' / name), '--version'], env=env))
        reported = versions[name]
        if reported.get('product_version') != release or reported.get('source_commit') != commit:
            raise RuntimeError(f'{name} did not report the pinned release and source commit')
        if reported.get('executor_contract') not in SUPPORTED_CONTRACTS:
            raise RuntimeError(f'{name} reports executor contract {reported.get("executor_contract")!r}; '
                               f'these kits support {sorted(SUPPORTED_CONTRACTS)}')
    if versions['hubu'] != versions['hubu-server']:
        raise RuntimeError('hubu and hubu-server report different identities')
    return {'release': release, 'source_commit': commit, 'source_directory': str(source),
            'binary_directory': str(target / 'release'), 'binary_versions': versions,
            'executor_contract': versions['hubu']['executor_contract'],
            'toolchain': toolchain, 'build_command': command, 'build_log': str(log),
            'rustc_version': subprocess.check_output(['rustup', 'run', toolchain, 'rustc', '--version'], env=env, text=True).strip(),
            'git_version': subprocess.check_output(['git', '--version'], env=env, text=True).strip(),
            'published_release_url': published['html_url'], 'published_at': published['published_at']}
