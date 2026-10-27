"""Contract tests; fake cargo proves orchestration, not Rust/archive portability."""
import importlib.util
from contextlib import redirect_stdout
import io
import json
import os
import subprocess
import sys
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name('nextest_archive.py')
# Actual nextest --version stdout in PR727 job 122301891430, 2026-10-27.
REAL_NEXTEST_VERSION = '''cargo-nextest 0.9.149 (3fe4ef319 2026-10-20)
release: 0.9.149
commit-hash: 3fe4ef31972f24e0c94cfe291718eab0fe678260
commit-date: 2026-10-20
host: x86_64-unknown-linux-gnu'''


class ArchiveContract(unittest.TestCase):
    def setUp(self):
        self.assertTrue(SCRIPT.is_file(), 'archive helper must exist')
        spec = importlib.util.spec_from_file_location('nextest_archive', SCRIPT)
        self.helper = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.helper)
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        (self.root / 'src-tauri/.config').mkdir(parents=True)
        (self.root / 'src-tauri/.config/nextest.toml').write_text('[profile.ci]\nretries = 2\n')
        self.env = dict(GITHUB_RUN_ID='12', GITHUB_RUN_ATTEMPT='1', GITHUB_SHA='a'*40,
                        RUNNER_OS='Linux', RUNNER_ARCH='X64', CI_RUNNER_LABEL='ubuntu-22.04',
                        ImageOS='ubuntu22', ImageVersion='20261020.1')
        self.calls = []
        self.version = 'cargo-nextest 0.9.100'
        def execute(args, **kwargs):
            self.calls.append(args)
            if args == ['git', 'rev-parse', 'HEAD']:
                return self.env['GITHUB_SHA']
            if args == ['cargo', 'nextest', '--version']:
                return self.version
            if args[:3] == ['cargo', 'nextest', 'archive']:
                Path(args[args.index('--archive-file')+1]).write_bytes(b'fake archive')
            return ''
        self.addCleanup(patch.stopall)
        patch.object(self.helper, 'execute', execute).start()
        self.out = self.root / 'artifact'

    def build(self):
        self.helper.build(self.root, self.out, self.env)

    def run_archive(self, shard=1, shards=2):
        self.helper.run(self.root, self.out, self.env, shard, shards)

    def test_build_default_gui_and_run_without_rebuild(self):
        self.build()
        self.run_archive()
        build = next(c for c in self.calls if c[:3] == ['cargo','nextest','archive'])
        self.assertIn('--manifest-path', build)
        self.assertNotIn('--no-default-features', build)
        self.assertNotIn('--all-features', build)
        run = self.calls[-1]
        self.assertEqual(run[:3], ['cargo','nextest','run'])
        self.assertIn('--archive-file', run)
        self.assertNotIn('--manifest-path', run)
        self.assertEqual(run[run.index('--partition')+1], 'count:1/2')
        self.assertEqual(run[run.index('--profile')+1], 'ci')
        self.assertEqual(run[run.index('--workspace-remap')+1], str(self.root/'src-tauri'))
        self.assertEqual(sum(c[:3] == ['cargo','nextest','archive'] for c in self.calls), 1)

    def test_identity_mismatch_rejected_before_tests(self):
        self.build()
        for key in self.env:
            if key == 'ImageVersion':
                continue  # Separately tested: reject archive, rebuild locally.
            with self.subTest(key=key):
                changed = dict(self.env, **{key: self.env[key]+'wrong'})
                with self.assertRaises(ValueError):
                    self.helper.run(self.root, self.out, changed, 1, 2)
        self.assertFalse(any(c[:3] == ['cargo','nextest','run'] for c in self.calls))

    def test_real_image_drift_rejects_archive_and_rebuilds_original_partition(self):
        # Actual producer/consumer images from run40538744469 Ubuntu shard1.
        self.env['ImageVersion'] = '20261025.340.1'
        self.build()
        self.env['ImageVersion'] = '20261012.322.1'
        log = io.StringIO()
        with redirect_stdout(log): self.run_archive(2, 2)
        self.assertEqual(self.calls[-1], ['cargo','nextest','run','--profile','ci',
                         '--manifest-path',str(self.root/'src-tauri/Cargo.toml'),
                         '--partition','count:2/2'])
        self.assertIn('mode=local_rebuild', log.getvalue())
        self.assertIn('reason=ImageVersion', log.getvalue())
        self.assertIn('20261025.340.1', log.getvalue())
        self.assertIn('20261012.322.1', log.getvalue())

    def test_matching_image_reports_archive_reuse(self):
        self.build()
        log = io.StringIO()
        with redirect_stdout(log): self.run_archive()
        self.assertIn('mode=reuse', log.getvalue())
        self.assertIn('--archive-file', self.calls[-1])
        self.assertNotIn('--manifest-path', self.calls[-1])

    def test_image_drift_never_hides_other_identity_mismatches(self):
        self.build()
        for key in self.env:
            if key == 'ImageVersion': continue
            changed = dict(self.env, ImageVersion='20261012.322.1')
            changed[key] += '-wrong'
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.helper.run(self.root, self.out, changed, 1, 2)
        self.env['ImageVersion'] = '20261012.322.1'
        self.version += '-different'
        with self.assertRaises(ValueError): self.run_archive()
        self.assertFalse(any(c[:3] == ['cargo','nextest','run'] for c in self.calls))

    def test_image_drift_never_hides_incomplete_or_extra_identity(self):
        self.build()
        manifest_path = self.out/'manifest.json'
        original = json.loads(manifest_path.read_text())
        for missing in [True, False]:
            altered = json.loads(json.dumps(original))
            if missing: del altered['identity']['ImageVersion']
            else: altered['identity']['unexpected'] = None
            manifest_path.write_text(json.dumps(altered))
            with self.subTest(missing=missing), self.assertRaises(ValueError):
                self.run_archive()
        self.assertFalse(any(c[:3] == ['cargo','nextest','run'] for c in self.calls))

    def test_image_drift_never_hides_corrupt_archive(self):
        self.build()
        self.env['ImageVersion'] = '20261012.322.1'
        (self.out/'tests.tar.zst').write_bytes(b'tampered')
        with self.assertRaises(ValueError): self.run_archive()
        self.assertFalse(any(c[:3] == ['cargo','nextest','run'] for c in self.calls))

    def test_digest_mismatch_rejected(self):
        self.build()
        (self.out/'tests.tar.zst').write_bytes(b'tampered')
        with self.assertRaises(ValueError): self.run_archive()

    def test_nextest_version_mismatch_rejected(self):
        self.build()
        self.version = 'cargo-nextest different'
        with self.assertRaises(ValueError): self.run_archive()

    def test_compile_time_source_path_mismatch_rejected(self):
        self.build()
        other = self.root/'other'
        other.mkdir()
        with self.assertRaises(ValueError): self.helper.run(other, self.out, self.env, 1, 2)

    def test_invalid_partition_rejected(self):
        self.build()
        for shard, shards in [(0,2), (3,2), (1,4), (1,0)]:
            with self.subTest(shard=shard, shards=shards), self.assertRaises(ValueError):
                self.run_archive(shard, shards)

    def test_missing_ci_identity_rejected(self):
        for key in self.env:
            env = dict(self.env)
            del env[key]
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.helper.build(self.root, self.out, env)

    def test_all_platforms_keep_original_partition_counts(self):
        for os, arch, label, total in [('Linux','X64','ubuntu-22.04',2),
                                      ('Windows','X64','windows-latest',4),
                                      ('macOS','ARM64','macos-14',2)]:
            self.env.update(RUNNER_OS=os, RUNNER_ARCH=arch, CI_RUNNER_LABEL=label)
            self.build()
            for shard in range(1,total+1): self.run_archive(shard,total)

    def test_real_multiline_nextest_version_pins_release_and_preserves_provenance(self):
        output = self.root/'github-output'
        self.env['GITHUB_OUTPUT'] = str(output)
        for newline in ['\n', '\r\n']:
            with self.subTest(newline=repr(newline)):
                output.unlink(missing_ok=True)
                self.version = REAL_NEXTEST_VERSION.replace('\n', newline)
                self.build()
                self.assertEqual(output.read_text(), 'nextest-version=0.9.149\n')
                manifest = json.loads((self.out/'manifest.json').read_text())
                self.assertEqual(manifest['identity']['nextest_version'], self.version)
                self.run_archive()
                self.version = self.version.replace('3fe4ef31972f24e0c94cfe291718eab0fe678260', 'a'*40)
                with self.assertRaises(ValueError): self.run_archive()

    def test_multiline_version_rejects_malformed_or_conflicting_metadata(self):
        for bad in [REAL_NEXTEST_VERSION+'\nother-output=injected',
                    REAL_NEXTEST_VERSION.replace('release: 0.9.149','release: 0.9.148'),
                    REAL_NEXTEST_VERSION+'\nhost: other-host',
                    REAL_NEXTEST_VERSION.replace('commit-date: 2026-10-20','commit-date: invalid')]:
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                self.version = bad
                self.build()

    def test_build_exports_verified_exact_nextest_version(self):
        output = self.root/'github-output'
        self.env['GITHUB_OUTPUT'] = str(output)
        self.build()
        self.assertTrue(output.is_file(), 'builder must export exact nextest version')
        self.assertIn('nextest-version=0.9.100\n', output.read_text())

    def test_invalid_version_cannot_inject_job_outputs(self):
        self.env['GITHUB_OUTPUT'] = str(self.root/'github-output')
        self.version = 'cargo-nextest 0.9.100\nother-output=injected'
        with self.assertRaises(ValueError): self.build()
        self.assertFalse((self.root/'github-output').exists())

    def test_checkout_mismatch_rejected(self):
        original = self.helper.execute
        def wrong_head(args, **kwargs):
            return 'b'*40 if args[0] == 'git' else original(args, **kwargs)
        with patch.object(self.helper, 'execute', wrong_head), self.assertRaises(ValueError):
            self.build()

    def test_failed_rebuild_invalidates_old_manifest(self):
        self.build()
        original = self.helper.execute
        def fail(args, **kwargs):
            if args[:3] == ['cargo','nextest','archive']: raise RuntimeError('failed rebuild')
            return original(args, **kwargs)
        with patch.object(self.helper, 'execute', fail), self.assertRaises(RuntimeError):
            self.build()
        self.assertFalse((self.out/'manifest.json').exists())

    @unittest.skipUnless(os.name == 'posix', 'POSIX fake executables; CLI contract only')
    def test_real_cli_process_with_fake_cargo_and_failure_propagation(self):
        fakebin = self.root/'bin'
        fakebin.mkdir()
        log = self.root/'calls.jsonl'
        program = f'''#!{sys.executable}
import json, os, pathlib, sys
args = sys.argv[1:]
with open(os.environ['FAKE_LOG'], 'a') as stream:
    stream.write(json.dumps([pathlib.Path(sys.argv[0]).name] + args) + "\\n")
if pathlib.Path(sys.argv[0]).name == 'git': print(os.environ['GITHUB_SHA'])
elif args == ['nextest', '--version']: print(os.environ['FAKE_VERSION'])
elif args[:2] == ['nextest', 'archive']:
    pathlib.Path(args[args.index('--archive-file')+1]).write_bytes(b'process fake archive')
elif args[:2] == ['nextest', 'run']: sys.exit(int(os.environ.get('FAKE_EXIT', '0')))
else: sys.exit(91)
'''
        for name in ['git', 'cargo']:
            path = fakebin/name
            path.write_text(program)
            path.chmod(0o755)
        env = dict(os.environ, **self.env, FAKE_LOG=str(log), FAKE_VERSION=REAL_NEXTEST_VERSION)
        env['PATH'] = str(fakebin)+os.pathsep+env['PATH']
        base = [sys.executable, str(SCRIPT.resolve())]
        build = subprocess.run(base+['build','--output',str(self.out)], cwd=self.root,
                               env=env, capture_output=True, text=True)
        self.assertEqual(build.returncode, 0, build.stderr)
        args = base+['run','--output',str(self.out),'--shard','1','--shards','2']
        success = subprocess.run(args, cwd=self.root, env=env, capture_output=True, text=True)
        self.assertEqual(success.returncode, 0, success.stderr)
        failed = subprocess.run(args, cwd=self.root, env=dict(env, FAKE_EXIT='17'),
                                capture_output=True, text=True)
        self.assertNotEqual(failed.returncode, 0)
        drift_env = dict(env, ImageVersion='20261012.322.1')
        rebuilt = subprocess.run(args, cwd=self.root, env=drift_env, capture_output=True, text=True)
        self.assertEqual(rebuilt.returncode, 0, rebuilt.stderr)
        self.assertIn('mode=local_rebuild', rebuilt.stdout)
        rebuild_failed = subprocess.run(args, cwd=self.root, env=dict(drift_env, FAKE_EXIT='17'),
                                        capture_output=True, text=True)
        self.assertNotEqual(rebuild_failed.returncode, 0)
        calls = [json.loads(line) for line in log.read_text().splitlines()]
        self.assertEqual(sum(c[:3] == ['cargo','nextest','archive'] for c in calls), 1)
        self.assertEqual(sum(c[:3] == ['cargo','nextest','run'] for c in calls), 4)
        self.assertIn('--manifest-path', calls[-1])
        self.assertNotIn('--archive-file', calls[-1])

    def test_archive_failure_never_creates_manifest(self):
        def fail(args, **kwargs):
            if args[:3] == ['cargo','nextest','archive']: raise RuntimeError('compile failed')
            return self.env['GITHUB_SHA'] if args[0] == 'git' else self.version
        with patch.object(self.helper, 'execute', fail), self.assertRaises(RuntimeError): self.build()
        self.assertFalse((self.out/'manifest.json').exists())


if __name__ == '__main__': unittest.main()
