import importlib.util
from pathlib import Path
import tempfile
import sys
import json
import os
from unittest.mock import patch
import time
import unittest

SCRIPT = Path(__file__).with_name('linux_deps.py')
spec = importlib.util.spec_from_file_location('linux_deps', SCRIPT)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)

AZURE = 'http://azure.archive.ubuntu.com/ubuntu'
ERROR = f'Err:1 {AZURE} jammy/main amd64 example 1\n  Connection timed out\nE: Failed to fetch {AZURE}/pool/example.deb Connection timed out\n'

class Runner:
    def __init__(self, results=()):
        self.results = list(results)
        self.calls = []
    def __call__(self, args, timeout=None):
        self.calls.append((args, timeout))
        return self.results.pop(0) if self.results else (0, '')

class LinuxDepsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'sources.list.d').mkdir()
        self.original = f'deb [arch=amd64 signed-by=/key] {AZURE} jammy-updates main universe\n# {AZURE}\ndeb https://vendor.example/ubuntu stable main\n'
        (self.root / 'sources.list').write_text(self.original)

    def test_normal_path_never_rewrites_sources_and_installs_offline(self):
        runner = Runner()
        mod.install(self.root, runner)
        self.assertEqual((self.root / 'sources.list').read_text(), self.original)
        calls = runner.calls
        self.assertIn('--error-on=any', calls[0][0])
        self.assertEqual(sum('--download-only' in a for a, _ in calls), 3)
        self.assertEqual(sum('--no-download' in a for a, _ in calls), 3)
        self.assertTrue(all(t is None for a, t in calls if '--no-download' in a))
        self.assertTrue(all(t is None for a, t in calls))
        self.assertFalse(any('Dir::Etc::sourcelist=' in ' '.join(a) for a, _ in calls))
        self.assertTrue(all('Acquire::Retries=2' in a for a, _ in calls))

    def test_azure_failure_retries_on_official_mirror_without_mutating_original(self):
        runner = Runner([(100, ERROR)])
        mod.install(self.root, runner)
        self.assertEqual((self.root / 'sources.list').read_text(), self.original)
        self.assertEqual(sum('update' in a for a, _ in runner.calls), 2)
        self.assertTrue(any('Dir::Etc::sourcelist=' in ' '.join(a) for a, _ in runner.calls))

    def test_fallback_failure_is_not_green(self):
        with self.assertRaises(RuntimeError):
            mod.install(self.root, Runner([(100, ERROR), (100, ERROR)]))

    def test_foreign_failure_never_switches_sources(self):
        runner = Runner([(100, ERROR.replace('azure.archive.ubuntu.com', 'vendor.example'))])
        with self.assertRaises(RuntimeError):
            mod.install(self.root, runner)
        self.assertEqual(len(runner.calls), 1)

    def test_unrelated_transport_error_cannot_authorize_azure_fallback(self):
        output = f'Err:1 {AZURE} jammy InRelease\n  Unexpected local error\nErr:2 https://vendor.example/repo stable InRelease\n  Connection timed out\n'
        self.assertFalse(mod.azure_network_failure(output))

    def test_missing_preferred_library_preserves_original_compatibility_alternative(self):
        runner = Runner([(0, ''), (0, ''), (0, ''), (100, 'E: Unable to locate package libwebkit2gtk-4.1-dev')])
        mod.install(self.root, runner)
        downloads = [a for a, _ in runner.calls if '--download-only' in a]
        self.assertEqual(downloads[2][-1], 'libwebkit2gtk-4.0-dev')
        self.assertEqual(sum('update' in a for a, _ in runner.calls), 1)

    def test_download_fallback_refreshes_indices_before_retry(self):
        runner = Runner([(0, ''), (100, ERROR)])
        mod.install(self.root, runner)
        self.assertEqual([('update' if 'update' in a else 'download' if '--download-only' in a else 'install') for a, _ in runner.calls][:5],
                         ['update', 'download', 'update', 'download', 'install'])

    def test_authorization_and_certificate_errors_never_switch_mirrors(self):
        for reason in ['401 Unauthorized', '403 Forbidden', '407 Proxy Authentication Required',
                       'Certificate verification failed', 'The certificate is NOT trusted']:
            with self.subTest(reason=reason):
                output = f'Err:1 {AZURE} jammy InRelease\n  {reason}\n'
                self.assertFalse(mod.azure_network_failure(output))

    def test_signature_failure_never_switches_sources(self):
        runner = Runner([(100, ERROR + 'E: The repository is not signed.\n')])
        with self.assertRaises(RuntimeError):
            mod.install(self.root, runner)
        self.assertEqual(len(runner.calls), 1)

    def test_external_interruption_does_not_guess_failed_mirror(self):
        runner = Runner([(124, 'externally interrupted')])
        with self.assertRaises(RuntimeError):
            mod.install(self.root, runner)
        self.assertEqual(len(runner.calls), 1)

    def test_dpkg_failure_does_not_retry_or_install_alternative(self):
        runner = Runner([(0, ''), (0, ''), (100, 'dpkg failed')])
        with self.assertRaises(RuntimeError):
            mod.install(self.root, runner)
        self.assertEqual(len(runner.calls), 3)

    def test_source_rewrite_preserves_options_suite_components_comments_and_foreign(self):
        result = mod.rewrite_sources(self.original, '.list')
        expected = self.original.replace(f'] {AZURE} ', '] https://archive.ubuntu.com/ubuntu ')
        self.assertEqual(result, expected)
        source = f'Types: deb\nURIs: {AZURE} https://vendor.example/repo\nSuites: jammy jammy-security\nComponents: main universe\nArchitectures: amd64\nSigned-By: /key\n'
        self.assertEqual(mod.rewrite_sources(source, '.sources'), source.replace(AZURE, 'https://archive.ubuntu.com/ubuntu'))
        deceptive = 'deb http://azure.archive.ubuntu.com.evil/ubuntu jammy main\n'
        self.assertEqual(mod.rewrite_sources(deceptive, '.list'), deceptive)

class CommandStubTests(unittest.TestCase):
    setUp = LinuxDepsTests.setUp
    # Exercise the real process runner with a fake apt-get executable, not the
    # host package manager; all sources and captured commands stay temporary.
    def stub_install(self, results):
        executable = self.root / 'apt-get'
        state = self.root / 'state.json'
        commands = self.root / 'commands.jsonl'
        state.write_text(json.dumps(results))
        executable.write_text(f"#!{sys.executable}\n" + r'''import json, os, sys
from pathlib import Path
state = Path(os.environ['APT_STUB_STATE'])
results = json.loads(state.read_text())
result = results.pop(0) if results else [0, '']
state.write_text(json.dumps(results))
args = sys.argv[1:]
sources = {}
for arg in args:
    if arg.startswith('Dir::Etc::sourcelist='):
        p = Path(arg.split('=', 1)[1])
        sources['list'] = p.read_text()
    if arg.startswith('Dir::Etc::sourceparts='):
        sources['parts'] = {p.name: p.read_text() for p in Path(arg.split('=', 1)[1]).iterdir()}
with open(os.environ['APT_STUB_COMMANDS'], 'a') as output:
    output.write(json.dumps({'args': args, 'sources': sources}) + '\n')
print(result[1], flush=True)
sys.exit(result[0])
''')
        executable.chmod(0o755)
        with patch.dict(os.environ, {'PATH': str(self.root) + os.pathsep + os.environ['PATH'],
                                   'APT_STUB_STATE': str(state), 'APT_STUB_COMMANDS': str(commands)}):
            try:
                mod.install(self.root)
                error = None
            except RuntimeError as exc:
                error = exc
        return error, [json.loads(line) for line in commands.read_text().splitlines()]

    def test_real_stub_normal_path(self):
        error, calls = self.stub_install([])
        self.assertIsNone(error)
        self.assertEqual(len(calls), 7)
        self.assertTrue(all(not c['sources'] for c in calls))
        self.assertEqual((self.root / 'sources.list').read_text(), self.original)

    def test_real_stub_fallback_preserves_third_party_and_source_constraints(self):
        deb822 = f'Types: deb\nURIs: {AZURE}\nSuites: jammy jammy-security\nComponents: main restricted\nArchitectures: amd64\nSigned-By: /key\n'
        (self.root / 'sources.list.d' / 'ubuntu.sources').write_text(deb822)
        error, calls = self.stub_install([(100, ERROR)])
        self.assertIsNone(error)
        self.assertEqual(calls[1]['sources']['list'], self.original.replace(f'] {AZURE} ', '] https://archive.ubuntu.com/ubuntu '))
        self.assertEqual(calls[1]['sources']['parts']['ubuntu.sources'], deb822.replace(AZURE, 'https://archive.ubuntu.com/ubuntu'))
        self.assertEqual((self.root / 'sources.list.d' / 'ubuntu.sources').read_text(), deb822)

    def test_real_stub_both_sources_fail(self):
        error, calls = self.stub_install([(100, ERROR), (100, ERROR)])
        self.assertIsNotNone(error)
        self.assertEqual(len(calls), 2)
        self.assertFalse(any('--no-download' in c['args'] for c in calls))

class ProcessTests(unittest.TestCase):
    def test_legal_slow_progress_is_not_terminated_by_helper_deadline(self):
        started = time.monotonic()
        code, output = mod.run([sys.executable, '-c', "import time; print('started', flush=True); time.sleep(.3); print('finished', flush=True)"])
        self.assertEqual(code, 0)
        self.assertEqual(output, 'started\nfinished\n')
        self.assertGreaterEqual(time.monotonic() - started, 0.3)

    def test_real_command_output_is_preserved_while_streaming(self):
        code, output = mod.run([sys.executable, '-c', "import time; print('first', flush=True); time.sleep(.2); print('second', flush=True)"])
        self.assertEqual(code, 0)
        self.assertEqual(output, 'first\nsecond\n')

    def test_real_failure_is_preserved(self):
        code, _ = mod.run([sys.executable, '-c', 'raise SystemExit(42)'])
        self.assertEqual(code, 42)

if __name__ == '__main__':
    unittest.main()
