"""Stdlib-only static DAG checks plus execution of the real Required Checks gate."""
from pathlib import Path
import re
import subprocess
import unittest

WORKFLOW = Path(__file__).resolve().parents[2] / '.github/workflows/ci.yml'
PLATFORMS = [('linux', 'ubuntu-22.04', 2), ('windows', 'windows-latest', 4),
             ('macos', 'macos-14', 2)]


def job(text, name):
    match = re.search(r'^  '+re.escape(name)+r':\n(.*?)(?=^  [\w-]+:|\Z)', text, re.M | re.S)
    if not match:
        raise AssertionError('Missing job: '+name)
    block = match[1]
    # Only CI's two whole-step-array aliases; this is not a general YAML parser.
    alias = re.search(r'^    steps: \*([a-z_]+)$', block, re.M)
    if alias:
        anchor = alias[1]
        if anchor not in {'backend_archive_steps', 'backend_test_steps'}:
            raise AssertionError('Unexpected steps alias: '+anchor)
        definition = re.search(r'^    steps: &'+anchor+r'\n(.*?)(?=^  [\w-]+:|\Z)',
                               text, re.M | re.S)
        if not definition:
            raise AssertionError('Missing steps anchor: '+anchor)
        block = block[:alias.start()]+'    steps:\n'+definition[1]
    return block


class ArchiveWorkflow(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = WORKFLOW.read_text(encoding='utf-8')

    def test_only_two_shared_step_arrays_without_yaml_merge(self):
        self.assertEqual(re.findall(r'&([a-z_]+)', self.workflow),
                         ['backend_archive_steps', 'backend_test_steps'])
        for anchor in ['backend_archive_steps', 'backend_test_steps']:
            self.assertEqual(self.workflow.count('steps: *'+anchor), 2)
        self.assertNotRegex(self.workflow, r'(?m)^\s*<<:', 'YAML merge is unsupported')

    def test_independent_same_platform_dependencies(self):
        for key, label, count in PLATFORMS:
            build = job(self.workflow, 'backend-test-build-'+key)
            tests = job(self.workflow, 'backend-tests-'+key)
            self.assertIn('needs: changes', build)
            self.assertIn('needs: [changes, backend-test-build-'+key+']', tests)
            self.assertIn('os: ['+label+']', build)
            self.assertNotIn('needs: [changes, backend-test-build]', tests)
            for other, _, _ in PLATFORMS:
                if other != key:
                    self.assertNotIn('backend-test-build-'+other, tests)

    def test_original_eight_checks_and_timeouts(self):
        total = 0
        for key, label, count in PLATFORMS:
            tests = job(self.workflow, 'backend-tests-'+key)
            self.assertIn('name: Backend Tests (${{ matrix.os }} ${{ matrix.shard }}/${{ matrix.shards }})', tests)
            rows = re.findall(r'\{ os: ([^,]+), shard: (\d+), shards: (\d+), timeout: (\d+) \}', tests)
            expected_timeout = {'linux':75, 'windows':150, 'macos':90}[key]
            self.assertEqual(rows, [(label, str(i), str(count), str(expected_timeout)) for i in range(1,count+1)])
            total += len(rows)
        self.assertEqual(total, 8)

    def test_all_six_jobs_keep_backend_path_condition(self):
        for key, _, _ in PLATFORMS:
            for prefix in ['backend-test-build-', 'backend-tests-']:
                self.assertIn("if: github.event_name != 'pull_request' || needs.changes.outputs.backend == 'true'", job(self.workflow, prefix+key))

    def test_archive_names_match_and_are_fully_scoped(self):
        for key, _, _ in PLATFORMS:
            build = job(self.workflow, 'backend-test-build-'+key)
            tests = job(self.workflow, 'backend-tests-'+key)
            producer_name = re.search(r'name: (nextest-.*)', build).group(1)
            self.assertIn('name: '+producer_name, tests)
            for identity in ['github.run_id', 'github.run_attempt', 'github.sha', 'matrix.os', 'runner.os', 'runner.arch']:
                self.assertIn('${{ '+identity+' }}', producer_name)
            self.assertIn('compression-level: 0', build)
            self.assertIn('retention-days: 1', build)
            self.assertIn('if-no-files-found: error', build)
            self.assertNotIn('pattern:', tests)
            self.assertNotIn('run-id:', tests)

    def test_producer_compiles_consumer_runs_without_cargo_cache(self):
        for key, _, _ in PLATFORMS:
            build = job(self.workflow, 'backend-test-build-'+key)
            tests = job(self.workflow, 'backend-tests-'+key)
            self.assertIn('nextest_archive.py build ', build)
            self.assertIn('nextest_archive.py run ', tests)
            self.assertNotIn('cargo nextest run', tests)
            self.assertNotIn('Swatinem/rust-cache', tests)
            self.assertIn('Swatinem/rust-cache@v2', build)
            self.assertIn('save-if: "true"', build)
            for block in [build, tests]:
                self.assertIn('CI_RUNNER_LABEL: ${{ matrix.os }}', block)
                self.assertIn('tool: cargo-nextest', block)
                self.assertIn('mkdir -p dist', block)

    def test_consumer_installs_exact_producer_nextest_version(self):
        for key, _, _ in PLATFORMS:
            build = job(self.workflow, 'backend-test-build-'+key)
            tests = job(self.workflow, 'backend-tests-'+key)
            self.assertIn('nextest-version: ${{ steps.archive.outputs.nextest-version }}', build)
            self.assertIn('id: archive', build)
            self.assertIn('NEXTEST_VERSION: ${{ needs.backend-test-build-'+key+'.outputs.nextest-version }}', tests)
            self.assertIn('tool: cargo-nextest@${{ env.NEXTEST_VERSION }}', tests)

    def test_runtime_deps_and_node_preflight_preserved(self):
        for key, _, _ in PLATFORMS:
            tests = job(self.workflow, 'backend-tests-'+key)
            self.assertIn('node-version-file: .node-version', tests)
            self.assertIn('tests/qoder-runtime/fixtures/interrupted.mjs', tests)
            self.assertIn('QODER_HOST_NODE=${process.execPath}', tests)
            self.assertLess(tests.index('Verify backend Node runtime'), tests.index('nextest_archive.py run'))
            for prefix in ['backend-test-build-', 'backend-tests-']:
                self.assertIn('sudo python3 scripts/ci/linux_deps.py', job(self.workflow, prefix+key))

    def test_gate_explicitly_needs_each_producer_and_consumer(self):
        gate = job(self.workflow, 'required-checks')
        deps = [name.strip() for name in re.search(r'needs:\s*\[([^\]]+)\]', gate).group(1).split(',') if name.strip()]
        self.assertIn('if: always()', gate)
        expected = ['changes','frontend','backend','backend-windows-wsl2','workflow-lint']
        expected += [prefix+key for key, _, _ in PLATFORMS for prefix in ['backend-test-build-', 'backend-tests-']]
        self.assertCountEqual(deps, expected)
        script = gate.split('        run: |\n')[1]
        script = '\n'.join(line[10:] for line in script.splitlines() if line.startswith('          '))
        marker = "${{ join(needs.*.result, ' ') }}"
        for failed in deps:
            for status in ['failure','cancelled']:
                statuses = [status if d == failed else 'skipped' for d in deps]
                with self.subTest(job=failed, status=status):
                    self.assertEqual(subprocess.run(['bash','-c',script.replace(marker,' '.join(statuses))], capture_output=True).returncode, 1)
        for status in ['success','skipped']:
            self.assertEqual(subprocess.run(['bash','-c',script.replace(marker,' '.join([status]*len(deps)))], capture_output=True).returncode, 0)

    def test_all_python_contracts_remain_in_workflow_lint(self):
        self.assertIn("python3 -m unittest discover -s scripts/ci -p 'test_*.py' -v", job(self.workflow, 'workflow-lint'))
        self.assertNotIn('continue-on-error', job(self.workflow, 'workflow-lint'))


if __name__ == '__main__': unittest.main()
