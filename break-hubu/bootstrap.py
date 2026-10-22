"""Build an unchanged, pinned Hubu release in a separate private cache."""
import json
import fcntl
import os
from pathlib import Path
import re
import subprocess
import tempfile
import urllib.request

RELEASE = 'v0.2.4'
COMMIT = '5344ab565171e1aa3d3ec628641cdbfce18372af'
REPOSITORY = 'https://github.com/hacker-no-ice/hubu.git'


def build(release, cache=None):
    cache = Path(cache or Path(tempfile.gettempdir()) / f'break-hubu-build-{os.getuid()}').resolve()
    cache.mkdir(mode=0o700, parents=True, exist_ok=True)
    # Separate invocations may share the source/build cache, never their servers or data.
    with (cache / 'build.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        return _build(release, cache)


def _build(release, cache):
    if release != RELEASE:
        raise ValueError(f'This card qualifies {RELEASE} only; got {release}')
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
    if not source.exists():
        run(['git', 'clone', '--branch', release, '--single-branch', REPOSITORY, source])
    def git(*args):
        return subprocess.check_output(['git', '-C', str(source), *args], env=env, text=True).strip()
    if git('rev-parse', 'HEAD') != COMMIT or git('rev-parse', f'{release}^{{commit}}') != COMMIT:
        raise RuntimeError(f'Cache is not the exact {release} release checkout: {source}')
    if git('status', '--porcelain', '--untracked-files=all'):
        raise RuntimeError(f'Release source cache is dirty: {source}')
    published_path = cache / 'published-release.json'
    if not published_path.exists():
        req = urllib.request.Request('https://api.github.com/repos/hacker-no-ice/hubu/releases/tags/' + release,
                                     headers={'User-Agent': 'break-hubu-public-proof-kit'})
        with urllib.request.urlopen(req, timeout=30) as response:
            published = json.load(response)
        if published['tag_name'] != release or published['draft'] or published['prerelease'] or COMMIT not in published['body']:
            raise RuntimeError('Published stable release metadata does not confirm the pinned source')
        pending = published_path.with_suffix('.pending')
        pending.write_text(json.dumps(published, indent=2) + '\n')
        pending.replace(published_path)
    published = json.loads(published_path.read_text())
    if published['tag_name'] != release or published['draft'] or published['prerelease'] or COMMIT not in published['body']:
        raise RuntimeError('Cached release metadata does not confirm pinned source')
    toolchain = re.search(r'channel\s*=\s*"([\d.]+)"', (source / 'rust-toolchain.toml').read_text()).group(1)
    # Do not silently install a toolchain. This also gives a useful prerequisite error.
    run(['rustup', 'run', toolchain, 'cargo', '--version'])
    env.update(HUBU_PRODUCT_VERSION=release, HUBU_SOURCE_COMMIT=COMMIT,
               GONGBU_PRODUCT_VERSION=release, GONGBU_SOURCE_COMMIT=COMMIT)
    command = ['rustup', 'run', toolchain, 'cargo', 'build', '--release', '--locked',
               '--target-dir', str(target), '--bin', 'hubu', '--bin', 'hubu-server']
    run(command, source)
    versions = {}
    for name in ('hubu', 'hubu-server'):
        versions[name] = json.loads(subprocess.check_output([str(target / 'release' / name), '--version'], env=env))
        if versions[name] != dict(product_version=release, source_commit=COMMIT,
                                  executor_contract='hubu-spend-executor-v4.5'):
            raise RuntimeError(f'{name} did not report the pinned release and contract')
    return {'release': release, 'source_commit': COMMIT, 'source_directory': str(source),
            'binary_directory': str(target / 'release'), 'binary_versions': versions,
            'toolchain': toolchain, 'build_command': command, 'build_log': str(log),
            'rustc_version': subprocess.check_output(['rustup', 'run', toolchain, 'rustc', '--version'], env=env, text=True).strip(),
            'git_version': subprocess.check_output(['git', '--version'], env=env, text=True).strip(),
            'published_release_url': published['html_url'], 'published_at': published['published_at']}
