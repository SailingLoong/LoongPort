#!/usr/bin/env python3
"""Retry CI APT transport failures without changing system sources or trust.

Ubuntu jammy apt supports Acquire::{http,https}::Timeout, Acquire::Retries,
and update --error-on=any. Native timeouts cover connection/idle failures;
there is deliberately no new total deadline for downloads making progress.
Only explicit Azure transport errors allow a one-time official-mirror fallback.
Run this helper with sudo on the CI runner; existing CI job timeouts still apply.
"""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time

AZURE = r'https?://azure\.archive\.ubuntu\.com/ubuntu/?'
OFFICIAL = 'https://archive.ubuntu.com/ubuntu'
OPTIONS = ['-o', 'Acquire::http::Timeout=30', '-o', 'Acquire::https::Timeout=30',
           '-o', 'Acquire::Retries=2', '-o', 'DPkg::Lock::Timeout=60']
PACKAGES = [
    ['build-essential', 'pkg-config', 'libssl-dev', 'libgtk-3-dev',
     'librsvg2-dev', 'libayatana-appindicator3-dev'],
    ['libwebkit2gtk-4.1-dev'],
    ['libsoup-3.0-dev'],
]
ALTERNATIVES = {'libwebkit2gtk-4.1-dev': 'libwebkit2gtk-4.0-dev',
                'libsoup-3.0-dev': 'libsoup2.4-dev'}


def rewrite_sources(text, suffix):
    """Change only active source URI tokens; leave third-party entries intact."""
    lines = []
    for line in text.splitlines(keepends=True):
        if suffix == '.list':
            line = re.sub(r'^(\s*deb(?:-src)?\s+(?:\[[^\]\n]*\]\s+)?)' + AZURE + r'(?=\s)',
                          lambda m: m[1] + OFFICIAL, line)
        elif suffix == '.sources' and re.match(r'^URIs:', line, re.I):
            line = re.sub(r'(?<=\s)' + AZURE + r'(?=\s|$)', OFFICIAL, line)
        lines.append(line)
    return ''.join(lines)


def azure_network_failure(output):
    if re.search(r'NO_PUBKEY|BADSIG|EXPKEYSIG|not signed|Hash Sum mismatch|certificate|not valid yet|expired|\b(?:401|403|407)\b|Unauthorized|Forbidden|Proxy Authentication', output, re.I):
        return False
    # Require both an exact Azure error URI and a transport failure, not merely
    # an earlier successful Azure download or a package-resolution failure.
    transport = r'timed? out|Could not connect|Could not resolve|Temporary failure resolving|Connection failed|Connection reset|\b(?:500|502|503|504)\s'
    lines = output.splitlines()
    for index, line in enumerate(lines):
        if re.match(r'^(?:Err:\d+\s+|E: Failed to fetch\s+)' + AZURE + r'(?=[/\s])', line):
            # apt prints Err details on indented continuation lines. Do not
            # combine an Azure error with another repository's network error.
            details = line
            for continuation in lines[index + 1:]:
                if not continuation.startswith((' ', '\t')):
                    break
                details += '\n' + continuation
            if re.search(transport, details, re.I):
                return True
    return False


def run(args):
    """Stream APT output; do not interrupt healthy slow downloads or dpkg."""
    started = time.monotonic()
    output = []
    with subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                          text=True, env={**os.environ, 'LC_ALL': 'C'}) as proc:
        for line in proc.stdout:
            output.append(line)
            print(line, end='', flush=True)
        code = proc.wait()
    print(f'APT phase finished: exit={code}, seconds={time.monotonic() - started:.1f}', flush=True)
    return code, ''.join(output)


def install(apt_dir=Path('/etc/apt'), runner=run):
    source_options = []
    fallback_used = False
    with tempfile.TemporaryDirectory(prefix='loongport-apt-') as temporary:
        work = Path(temporary)
        # apt's unprivileged download user must be able to read source files.
        work.chmod(0o755)

        def fallback(output):
            nonlocal fallback_used, source_options
            if fallback_used or not azure_network_failure(output):
                return False
            parts = work / 'sources.list.d'
            parts.mkdir()
            changed = False
            files = [(apt_dir / 'sources.list', work / 'sources.list')]
            files += [(p, parts / p.name) for p in sorted((apt_dir / 'sources.list.d').glob('*'))
                      if p.suffix in ('.list', '.sources') and p.is_file()]
            for original, destination in files:
                content = original.read_text() if original.exists() else ''
                replaced = rewrite_sources(content, original.suffix)
                changed |= replaced != content
                destination.write_text(replaced)
            if not changed:
                return False
            fallback_used = True
            source_options = ['-o', f'Dir::Etc::sourcelist={work / "sources.list"}',
                              '-o', f'Dir::Etc::sourceparts={parts}']
            print('Azure Ubuntu transport failed; retrying once with archive.ubuntu.com (temporary sources).', flush=True)
            return True

        def apt(arguments):
            return runner(['apt-get', *OPTIONS, *source_options, *arguments])

        def update():
            code, output = apt(['update', '--error-on=any'])
            if code and fallback(output):
                code, output = apt(['update', '--error-on=any'])
            if code:
                raise RuntimeError(f'APT update failed (exit {code})')

        update()
        for requested in PACKAGES:
            packages = requested.copy()
            code, output = apt(['install', '-y', '--no-install-recommends', '--download-only', *packages])
            if code and fallback(output):
                update()
                code, output = apt(['install', '-y', '--no-install-recommends', '--download-only', *packages])
            # Preserve the existing older-library compatibility path only when
            # the preferred package is unavailable, never for a network failure.
            if code and len(packages) == 1 and packages[0] in ALTERNATIVES:
                name = re.escape(packages[0])
                if re.search(rf'Unable to locate package {name}$|Package .{name}. has no installation candidate', output, re.M):
                    packages = [ALTERNATIVES[packages[0]]]
                    code, output = apt(['install', '-y', '--no-install-recommends', '--download-only', *packages])
            if code:
                raise RuntimeError(f'APT download failed (exit {code}): {packages}')
            code, _ = apt(['install', '-y', '--no-install-recommends', '--no-download', *packages])
            if code:
                raise RuntimeError(f'APT install failed (exit {code}): {packages}')


if __name__ == '__main__':
    install()
