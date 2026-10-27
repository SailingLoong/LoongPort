"""Build once per CI platform; validate a same-run archive before each shard.

This is an ephemeral artifact, never a dependency cache or cross-platform binary.
System runtime libraries and checked-out source/Node fixtures remain required.
"""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import subprocess

IDENTITY = ('GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT', 'GITHUB_SHA', 'RUNNER_OS',
            'RUNNER_ARCH', 'CI_RUNNER_LABEL', 'ImageOS', 'ImageVersion')
PLATFORMS = {'ubuntu-22.04': ('Linux', 2), 'windows-latest': ('Windows', 4),
             'macos-15': ('macOS', 2)}


def execute(args, *, cwd):
    result = subprocess.run(args, cwd=cwd, check=True, text=True,
                            stdout=subprocess.PIPE if args[0] == 'git' or '--version' in args else None)
    return (result.stdout or '').strip()


def identity(root, env):
    if any(not env.get(key) for key in IDENTITY):
        raise ValueError('Missing CI archive identity')
    result = {key: env[key] for key in IDENTITY}
    platform = PLATFORMS.get(result['CI_RUNNER_LABEL'])
    if platform is None or platform[0] != result['RUNNER_OS']:
        raise ValueError('Unsupported or mismatched runner label/OS')
    if execute(['git', 'rev-parse', 'HEAD'], cwd=root) != result['GITHUB_SHA']:
        raise ValueError('Checkout HEAD does not match workflow SHA')
    result['workspace'] = str(root.resolve() / 'src-tauri')
    result['nextest_version'] = execute(['cargo', 'nextest', '--version'], cwd=root)
    return result


def digest(path):
    checksum = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024*1024), b''):
            checksum.update(block)
    return checksum.hexdigest()


def exact_nextest_version(output):
    """Extract the release token without discarding full-version provenance."""
    lines = output.splitlines()
    version = re.fullmatch(r'cargo-nextest (\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)(?: [^\r\n]+)?',
                           lines[0] if lines else '')
    if version is None:
        raise ValueError('Unrecognized nextest version; cannot pin archive consumer')
    # Modern nextest --version includes these metadata lines. Accept that real
    # format, but never copy arbitrary multiline text into GITHUB_OUTPUT.
    fields = {'release': re.escape(version.group(1)), 'commit-hash': r'[0-9a-f]{40}',
              'commit-date': r'\d{4}-\d{2}-\d{2}', 'host': r'[A-Za-z0-9_.-]+'}
    seen = set()
    for line in lines[1:]:
        key, separator, value = line.partition(': ')
        if (not separator or key not in fields or key in seen
                or re.fullmatch(fields[key], value) is None):
            raise ValueError('Unrecognized or conflicting nextest version metadata')
        seen.add(key)
    return version.group(1)


def build(root, output, env):
    expected = identity(root, env)
    # install-action accepts tool@exact-version. Validate before writing outputs.
    version = exact_nextest_version(expected['nextest_version'])
    output.mkdir(parents=True, exist_ok=True)
    manifest = output / 'manifest.json'
    # A failed retry must not leave a previous successful manifest eligible.
    manifest.unlink(missing_ok=True)
    archive = output / 'tests.tar.zst'
    execute(['cargo', 'nextest', 'archive', '--profile', 'ci',
             '--manifest-path', str(root / 'src-tauri/Cargo.toml'),
             '--archive-file', str(archive)], cwd=root)
    data = {'schema': 1, 'identity': expected, 'sha256': digest(archive),
            'bytes': archive.stat().st_size}
    manifest.write_text(json.dumps(data, indent=2)+'\n', encoding='utf-8')
    if env.get('GITHUB_OUTPUT'):
        with Path(env['GITHUB_OUTPUT']).open('a', encoding='utf-8') as stream:
            stream.write(f"nextest-version={version}\n")
    print(f"nextest archive: {data['bytes']} bytes; sha256={data['sha256']}", flush=True)


def run(root, output, env, shard, shards):
    expected = identity(root, env)
    if shards != PLATFORMS[env['CI_RUNNER_LABEL']][1] or not 1 <= shard <= shards:
        raise ValueError('Partition does not match original platform coverage')
    data = json.loads((output / 'manifest.json').read_text(encoding='utf-8'))
    archived_identity = data.get('identity')
    if (data.get('schema') != 1 or not isinstance(archived_identity, dict)
            or archived_identity.keys() != expected.keys()
            or any(not isinstance(value, str) or not value for value in archived_identity.values())):
        raise ValueError('Archive identity schema mismatch')
    mismatches = {key for key in expected.keys() | archived_identity.keys()
                  if expected.get(key) != archived_identity.get(key)}
    if mismatches and mismatches != {'ImageVersion'}:
        # env!(CARGO_MANIFEST_DIR) paths cannot be fixed by workspace-remap.
        raise ValueError('Archive identity/version/source path mismatch: '
                         + ', '.join(sorted(mismatches)))
    archive = output / 'tests.tar.zst'
    if archive.stat().st_size != data.get('bytes') or digest(archive) != data.get('sha256'):
        raise ValueError('Archive size or SHA256 mismatch')
    if mismatches == {'ImageVersion'}:
        # Floating hosted labels can select different image revisions in one run.
        # Do not execute the archive on a different image: compile from this same
        # verified checkout, retaining the original GUI features/profile/shard.
        print('nextest execution: mode=local_rebuild reason=ImageVersion '
              + json.dumps({'archive': archived_identity['ImageVersion'],
                            'runner': expected['ImageVersion']}), flush=True)
        execute(['cargo', 'nextest', 'run', '--profile', 'ci',
                 '--manifest-path', str(root / 'src-tauri/Cargo.toml'),
                 '--partition', f'count:{shard}/{shards}'], cwd=root)
    else:
        print('nextest execution: mode=reuse', flush=True)
        execute(['cargo', 'nextest', 'run', '--profile', 'ci',
                 '--archive-file', str(archive), '--workspace-remap', expected['workspace'],
                 '--partition', f'count:{shard}/{shards}'], cwd=root)



def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['build', 'run'])
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--shard', type=int)
    parser.add_argument('--shards', type=int)
    args = parser.parse_args()
    root = Path.cwd().resolve()
    if args.mode == 'build':
        build(root, args.output.resolve(), os.environ)
    else:
        if args.shard is None or args.shards is None:
            parser.error('run requires --shard and --shards')
        run(root, args.output.resolve(), os.environ, args.shard, args.shards)


if __name__ == '__main__':
    main()
