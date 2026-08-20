"""Exercise the real test-all recipes without building or running a runtime."""

from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from contextlib import contextmanager
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class TestAllPhaseWorkflow(unittest.TestCase):
    def test_extension_target_directory_uses_cargo_configuration(self):
        real_just = shutil.which("just")
        self.assertIsNotNone(real_just)
        with tempfile.TemporaryDirectory(prefix="cargo-artifacts-", dir=ROOT / "work") as directory:
            artifacts = Path(directory) / "target with spaces"
            for selection in (str(artifacts), os.path.relpath(artifacts, ROOT)):
                with self.subTest(selection=selection):
                    result = subprocess.run(
                        [real_just, "--justfile", str(ROOT / "Justfile"), "_cargo-target-dir"],
                        cwd=ROOT,
                        env={**os.environ, "CARGO_TARGET_DIR": selection},
                        text=True,
                        capture_output=True,
                        timeout=30,
                        check=False,
                    )
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertEqual(Path(result.stdout.strip()), artifacts)

    def test_stack_capture_loads_source_tree_python_helpers(self):
        import runpy
        from unittest.mock import patch

        real_just = shutil.which("just")
        self.assertIsNotNone(real_just)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "cpython-source"
            helper = source / "Tools" / "gdb" / "libpython.py"
            helper.parent.mkdir(parents=True)
            helper.write_text("# source-tree helper, no in-tree build required\n")
            commands = root / "commands"
            commands.mkdir()
            calls = root / "gdb-calls.jsonl"
            gdb = commands / "gdb"
            gdb.write_text(
                f"#!{sys.executable}\n"
                "import json, os, sys\n"
                "with open(os.environ['TEST_STACK_CALLS'], 'a') as output:\n"
                "    output.write(json.dumps(sys.argv[1:]) + '\\n')\n"
            )
            gdb.chmod(0o700)
            environment = {
                **os.environ,
                "REPO_ROOT": str(ROOT),
                "VENV_DIR": str(ROOT / ".venv"),
                "CPYTHON_SOURCE_DIR": str(source),
                "PATH": str(commands) + os.pathsep + os.environ.get("PATH", ""),
                "UV_TOOL_BIN_DIR": str(commands),
                "TEST_STACK_CALLS": str(calls),
            }
            with patch.dict(os.environ, environment, clear=True):
                runner = runpy.run_path(str(ROOT / "scripts" / "run_pytest_parallel.py"))
                status, output = runner["run_gdb_stack_capture"](os.getpid())
            self.assertEqual(status, 0, output)
            # The public recipe executes its real path selection too; only
            # external gdb is substituted, so this is not ptrace evidence.
            result = subprocess.run(
                [real_just, "--justfile", str(ROOT / "Justfile"), "capture-test-stacks", str(os.getpid()), str(root / "stacks.log")],
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            invocations = [json.loads(line) for line in calls.read_text().splitlines()]
            self.assertGreaterEqual(len(invocations), 2)
            for invocation in invocations:
                with self.subTest(invocation=invocation):
                    self.assertIn(f"source {helper}", invocation)
                    self.assertIn("thread apply all py-bt", invocation)
                    self.assertFalse(any("auto-load safe-path" in arg for arg in invocation))

    def test_stack_capture_timeout_preserves_output_and_worker_cleanup(self):
        import runpy
        from unittest.mock import Mock, call, patch

        with patch.dict(os.environ, {
            "REPO_ROOT": str(ROOT), "VENV_DIR": str(ROOT / ".venv"),
        }):
            runner = runpy.run_path(str(ROOT / "scripts" / "run_pytest_parallel.py"))
        run_pytest = runner["run_pytest"]
        cases = (
            (b"stdout\n", b"stderr\n", "stdout\nstderr\n"),
            ("stdout\n", "stderr\n", "stdout\nstderr\n"),
            (None, None, ""),
            (b"stdout\n", "stderr\n", "stdout\nstderr\n"),
            ("stdout\n", b"stderr\n", "stdout\nstderr\n"),
            (b"stdout\n", None, "stdout\n"),
            (None, b"stderr\n", "stderr\n"),
            ("stdout\n", None, "stdout\n"),
            (None, "stderr\n", "stderr\n"),
            (b"partial UTF-8: \xe2", b"\xff\n", "partial UTF-8: \ufffd\ufffd\n"),
        )
        for stdout, stderr, expected in cases:
            with (
                self.subTest(stdout=stdout, stderr=stderr),
                tempfile.TemporaryDirectory() as directory,
            ):
                proc = Mock(pid=1234, returncode=-signal.SIGTERM)
                terminate = Mock()

                def communicate(*, timeout=None):
                    if timeout is not None:
                        raise subprocess.TimeoutExpired(["pytest"], timeout)
                    terminate.assert_called_once_with(proc)
                    return "worker stdout\n", "worker stderr\n"

                proc.communicate.side_effect = communicate
                monitor = runner["BatchMonitor"]()
                batch = runner["PytestBatch"]("timeout fixture", ["tests/test_wait.py"])
                # Keep the actual runner, diagnostic writer and monitor. Only
                # external processes and procfs observations are substituted;
                # existing process-group tests cover real descendant reaping.
                with (
                    patch.dict(run_pytest.__globals__, {
                        "LOGS_DIR": Path(directory),
                        "collect_descendant_pids": lambda _: [proc.pid],
                        "proc_cmdline": lambda _: "test worker",
                        "proc_cwd": lambda _: str(ROOT),
                        "proc_text": lambda _: None,
                        "process_exists": lambda _: True,
                        "shutil_which": lambda _: "gdb",
                        "terminate_process_group": terminate,
                    }),
                    patch.object(subprocess, "Popen", return_value=proc),
                    patch.object(subprocess, "run", side_effect=subprocess.TimeoutExpired(
                        ["gdb"], 20, output=stdout, stderr=stderr,
                    )) as gdb,
                ):
                    result = run_pytest(batch.selectors, batch, 1, 300.0, monitor)
                self.assertTrue(result.timed_out)
                self.assertEqual(result.returncode, 124)
                self.assertIn("worker stdout\nworker stderr\n", result.output)
                self.assertIn("batch timed out after 300.0s", result.output)
                self.assertIn("captured timeout stacks:", result.output)
                self.assertNotIn("failed to capture timeout stacks:", result.output)
                self.assertEqual(proc.communicate.call_args_list, [call(timeout=300.0), call()])
                terminate.assert_called_once_with(proc)
                self.assertEqual(monitor.snapshot(), [])
                gdb.assert_called_once()
                self.assertTrue(gdb.call_args.kwargs["text"])
                self.assertEqual(gdb.call_args.kwargs["timeout"], 20)
                logs = list(Path(directory).glob("pytest-timeout-stacks-*.log"))
                self.assertEqual(len(logs), 1)
                diagnostic = logs[0].read_text()
                self.assertIn(expected + "\ngdb stack capture timed out after 20s\n", diagnostic)
                self.assertIn("gdb capture exited with status 124 for pid 1234", diagnostic)

    def test_parallel_batches_stay_bounded_when_the_suite_grows(self):
        import runpy
        from collections import Counter
        from unittest.mock import patch

        with patch.dict(os.environ, {
            "REPO_ROOT": str(ROOT), "VENV_DIR": str(ROOT / ".venv"),
        }):
            runner = runpy.run_path(str(ROOT / "scripts" / "run_pytest_parallel.py"))
        selected = [f"tests/test_selected.py::test_case[{index}]" for index in range(179)]
        unrelated = [
            f"tests/test_other_{file}.py::test_case[{index}]"
            for file in range(12) for index in range(200)
        ]
        for jobs in (1, 8, 64):
            for nodes in (selected, selected + unrelated):
                with self.subTest(jobs=jobs, node_count=len(nodes)):
                    batches = runner["make_nodeid_batches"](nodes, jobs)
                    self.assertTrue(batches)
                    self.assertTrue(all(0 < len(batch.selectors) <= 4 for batch in batches))
                    self.assertEqual(
                        Counter(node for batch in batches for node in batch.selectors),
                        Counter(nodes),
                        "splitting must neither omit nor duplicate collected tests",
                    )
                    for batch in batches:
                        self.assertEqual(len({node.split("::", 1)[0] for node in batch.selectors}), 1)
                        self.assertEqual(
                            batch.selectors,
                            sorted(batch.selectors, key=nodes.index),
                            "each file-local slice retains collection order",
                        )

    def test_reviewed_multiphase_counter_tests_have_independent_batches(self):
        import runpy
        from collections import Counter
        from unittest.mock import patch

        with patch.dict(os.environ, {
            "REPO_ROOT": str(ROOT), "VENV_DIR": str(ROOT / ".venv"),
        }):
            runner = runpy.run_path(str(ROOT / "scripts" / "run_pytest_parallel.py"))
        file_path = "tests/test_counter_dump_file.py"
        reviewed = [
            f"{file_path}::test_profiled_full_nqueens_slice_preserves_results_mutations_and_ordinary_tracing",
            f"{file_path}::test_profiled_pyperformance_nqueens_preserves_rebinding_and_ordinary_tracing",
        ]
        neighbors = [
            [f"{file_path}::test_neighbor_{position}[{index}]" for index in range(2)]
            for position in ("before", "between", "after")
        ]
        nodes = [
            *neighbors[0], reviewed[0], *neighbors[1], reviewed[1], *neighbors[2],
        ]
        batches = runner["make_file_batches"](file_path, nodes, 4)
        self.assertEqual(
            [batch.selectors for batch in batches],
            [neighbors[0], [reviewed[0]], neighbors[1], [reviewed[1]], neighbors[2]],
            "a reviewed test must not share either neighbor's batch",
        )
        for jobs in (1, 2, 8, 64):
            with self.subTest(jobs=jobs):
                batches = runner["make_nodeid_batches"](nodes, jobs)
                self.assertEqual(
                    Counter(node for batch in batches for node in batch.selectors),
                    Counter(nodes),
                    "isolation must neither omit nor duplicate collected tests",
                )
                for batch in batches:
                    self.assertLessEqual(len(batch.selectors), 4)
                    for node in reviewed:
                        if node in batch.selectors:
                            self.assertEqual(batch.selectors, [node])

    def test_reviewed_closed_pipeline_backends_have_independent_batches(self):
        import runpy
        from collections import Counter
        from unittest.mock import patch

        with patch.dict(os.environ, {
            "REPO_ROOT": str(ROOT), "VENV_DIR": str(ROOT / ".venv"),
        }):
            runner = runpy.run_path(str(ROOT / "scripts" / "run_pytest_parallel.py"))
        file_path = "tests/test_closed_iterator_pipeline.py"
        group = f"{file_path}::test_reviewed_closed_pipelines_use_authenticated_entries"
        reviewed = [f"{group}[{backend}]" for backend in ("cpython", "compiled", "entry")]
        before = [f"{group}_ordinary_before[{index}]" for index in range(2)]
        after = [f"{group}_ordinary_after[{index}]" for index in range(2)]
        nodes = [*before, *reviewed, *after]
        batches = runner["make_file_batches"](file_path, nodes, 4)
        self.assertEqual(
            [batch.selectors for batch in batches],
            [before, *[[node] for node in reviewed], after],
            "each 12-module backend validation gets its own unchanged batch budget",
        )
        for jobs in (1, 2, 8, 64):
            with self.subTest(jobs=jobs):
                batches = runner["make_nodeid_batches"](nodes, jobs)
                self.assertEqual(
                    Counter(node for batch in batches for node in batch.selectors),
                    Counter(nodes),
                    "all backends and ordinary neighbors must remain enrolled exactly once",
                )
                for batch in batches:
                    self.assertLessEqual(len(batch.selectors), 4)
                    for node in reviewed:
                        if node in batch.selectors:
                            self.assertEqual(batch.selectors, [node])

    def test_singleton_policy_covers_parameters_but_not_similar_node_groups(self):
        import runpy
        from collections import Counter
        from unittest.mock import patch

        with patch.dict(os.environ, {
            "REPO_ROOT": str(ROOT), "VENV_DIR": str(ROOT / ".venv"),
        }):
            runner = runpy.run_path(str(ROOT / "scripts" / "run_pytest_parallel.py"))
        file_path = "tests/test_counter_dump_file.py"
        group = (
            f"{file_path}::"
            "test_profiled_full_nqueens_slice_preserves_results_mutations_and_ordinary_tracing"
        )
        # More than MAX_BATCH_NODEIDS also exercises ordinary group chunking
        # before the singleton policy is applied.
        parameters = [f"{group}[{index}]" for index in range(5)]
        before = [f"{group}_ordinary_before[{index}]" for index in range(2)]
        after = [f"{group}_ordinary_after[{index}]" for index in range(2)]
        nodes = [*before, *parameters, *after]
        batches = runner["make_file_batches"](file_path, nodes, 4)
        self.assertEqual(
            [batch.selectors for batch in batches],
            [before, *[[node] for node in parameters], after],
        )

        # Identical function spelling in another file is not an enrollment.
        other_group = group.replace(file_path, "tests/test_other.py", 1)
        other = [f"{other_group}[{index}]" for index in range(4)]
        batches = runner["make_nodeid_batches"]([*nodes, *other], 1)
        self.assertEqual(
            Counter(node for batch in batches for node in batch.selectors),
            Counter([*nodes, *other]),
        )
        self.assertEqual(
            [batch.selectors for batch in batches if other[0] in batch.selectors],
            [other],
        )

    def test_required_batch_runner_rejects_passthrough_before_execution(self):
        import io
        import runpy
        from contextlib import redirect_stderr
        from unittest.mock import Mock, patch

        with patch.dict(os.environ, {
            "REPO_ROOT": str(ROOT), "VENV_DIR": str(ROOT / ".venv"),
        }):
            runner = runpy.run_path(str(ROOT / "scripts" / "run_pytest_parallel.py"))
        main = runner["main"]
        for arguments, jobs in (
            (["--require-batch-runner", "tests", "-q"], "2"),
            (["--require-batch-runner", "tests", "-v"], "2"),
            (["--require-batch-runner"], "2"),
            (["--require-batch-runner", "tests"], "0"),
        ):
            with self.subTest(arguments=arguments, jobs=jobs):
                collect = Mock(return_value=(0, ["tests/test_case.py::test_one"], ""))
                diagnostic = io.StringIO()
                with (
                    patch.dict(os.environ, {"PYTEST_NUMPROCS": jobs}),
                    patch.dict(main.__globals__, {"collect_test_nodeids": collect}),
                    patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run,
                    redirect_stderr(diagnostic),
                ):
                    status = main(arguments)
                self.assertEqual(status, 2)
                collect.assert_not_called()
                run.assert_not_called()
                self.assertIn("requires the batch runner", diagnostic.getvalue())

    def test_unrequired_pytest_options_still_allow_explicit_passthrough(self):
        import runpy
        from unittest.mock import patch

        with patch.dict(os.environ, {
            "REPO_ROOT": str(ROOT), "VENV_DIR": str(ROOT / ".venv"),
        }):
            runner = runpy.run_path(str(ROOT / "scripts" / "run_pytest_parallel.py"))
        with patch.object(
            subprocess, "run", return_value=subprocess.CompletedProcess([], 7)
        ) as run:
            status = runner["main"](["tests", "-q"])
        self.assertEqual(status, 7)
        self.assertEqual(run.call_args.args[0][-2:], ["tests", "-q"])

    def test_required_batch_runner_does_not_replay_empty_collection(self):
        import io
        import runpy
        from contextlib import redirect_stderr, redirect_stdout
        from unittest.mock import Mock, patch

        with patch.dict(os.environ, {
            "REPO_ROOT": str(ROOT), "VENV_DIR": str(ROOT / ".venv"),
        }):
            runner = runpy.run_path(str(ROOT / "scripts" / "run_pytest_parallel.py"))
        main = runner["main"]
        collect = Mock(return_value=(0, [], "no runnable nodes\n"))
        with (
            patch.dict(os.environ, {"PYTEST_NUMPROCS": "2"}),
            patch.dict(main.__globals__, {"collect_test_nodeids": collect}),
            patch.object(subprocess, "run") as run,
            redirect_stdout(io.StringIO()),
            redirect_stderr(io.StringIO()),
        ):
            status = main(["--require-batch-runner", "tests"])
        self.assertEqual(status, 5)
        self.assertEqual(collect.call_args.args[0][-1:], ["tests"])
        run.assert_not_called()

    def test_every_phase_runs_and_first_failure_is_preserved(self):
        real_just = shutil.which("just")
        self.assertIsNotNone(real_just, "the repository workflow requires just")
        for statuses in ((0, 0, 0), (101, 0, 0), (0, 7, 0), (0, 0, 9), (3, 5, 7)):
            with (
                self.subTest(statuses=statuses),
                tempfile.TemporaryDirectory() as directory,
            ):
                root = Path(directory)
                commands = root / "commands"
                commands.mkdir()
                temporary = root / "phase-logs"
                temporary.mkdir()
                calls = root / "calls.jsonl"
                phase_statuses = dict(
                    zip(("cargo-test", "raw-runtime-test", "pytest"), statuses)
                )
                # The actual public recipe and its actual private phase recipe
                # execute. Only their external work commands are substituted;
                # an unexpected command fails instead of reaching a real build.
                program = textwrap.dedent("""\
                    #!/usr/bin/env python3
                    import json
                    import os
                    from pathlib import Path
                    import sys

                    if Path(sys.argv[0]).name == "cargo":
                        if sys.argv[1:] != ["test", "--no-fail-fast", "--jobs", "1", "--", "--test-threads=1"]:
                            raise SystemExit(190)
                        phase = "cargo-test"
                    else:
                        phase = {
                            "uninstall-extension": "cleanup",
                            "build-test-runtime": "build",
                            "_test-all-test-phase": "test-phase",
                            "test-jit-runtime": "raw-runtime-test",
                            "_pytest-run": "pytest",
                        }.get(sys.argv[1])
                        if phase is None:
                            raise SystemExit(191)
                    with open(os.environ["TEST_GATE_CALLS"], "a") as output:
                        output.write(json.dumps(phase) + "\\n")
                    if phase == "test-phase":
                        executable = os.environ["TEST_GATE_JUST"]
                        os.execv(executable, [executable, "--justfile", os.environ["TEST_GATE_JUSTFILE"], "_test-all-test-phase"])
                    print("executed " + phase, flush=True)
                    raise SystemExit(json.loads(os.environ["TEST_GATE_STATUSES"]).get(phase, 0))
                    """)
                for name in ("cargo", "just"):
                    executable = commands / name
                    executable.write_text(program)
                    executable.chmod(0o700)
                result = subprocess.run(
                    [real_just, "--justfile", str(ROOT / "Justfile"), "test-all"],
                    cwd=ROOT,
                    env={
                        **os.environ,
                        "PATH": str(commands) + os.pathsep + os.environ.get("PATH", ""),
                        "UV_TOOL_BIN_DIR": str(commands),
                        "TMPDIR": str(temporary),
                        "TEST_GATE_CALLS": str(calls),
                        "TEST_GATE_JUST": real_just,
                        "TEST_GATE_JUSTFILE": str(ROOT / "Justfile"),
                        "TEST_GATE_STATUSES": json.dumps(phase_statuses),
                    },
                    text=True,
                    capture_output=True,
                    timeout=30,
                    check=False,
                )
                evidence = result.stdout + result.stderr
                self.assertEqual(
                    result.returncode,
                    next((code for code in statuses if code), 0),
                    evidence,
                )
                self.assertEqual(
                    [json.loads(line) for line in calls.read_text().splitlines()],
                    [
                        "cleanup",
                        "build",
                        "test-phase",
                        "cargo-test",
                        "raw-runtime-test",
                        "pytest",
                        "cleanup",
                    ],
                    evidence,
                )
                recorded = {
                    path.stem: int(path.read_text())
                    for path in temporary.rglob("*.status")
                }
                self.assertEqual(recorded, phase_statuses, evidence)


@unittest.skipUnless(sys.platform == "linux", "worker process groups require Linux")
class ParallelPytestWorkflow(unittest.TestCase):
    def wait_for(self, predicate, *, timeout=5):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if predicate():
                return
            time.sleep(0.01)
        self.fail("timed out waiting for the subprocess checkpoint")

    def records(self, root):
        records = []
        for path in root.glob("started-*.json"):
            try:
                records.append(json.loads(path.read_text()))
            except (OSError, json.JSONDecodeError):
                pass
        return records

    @contextmanager
    def runner(self, mode="normal", *, jobs=2, arguments=("tests",)):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "tests").mkdir()
            for name in "abcd":
                (root / "tests" / f"test_{name}.py").write_text("")
            child_program = textwrap.dedent("""
                import os
                from pathlib import Path
                import signal
                import sys
                import time

                if os.environ["TEST_PARALLEL_MODE"] == "resistant":
                    signal.signal(signal.SIGTERM, signal.SIG_IGN)
                Path(sys.argv[1]).write_text(str(os.getpid()))
                print("descendant diagnostic", flush=True)
                while True:
                    time.sleep(1)
                """)
            executable = root / "venv" / "bin" / "python"
            executable.parent.mkdir(parents=True)
            executable.write_text(
                f"#!{sys.executable}\n"
                + textwrap.dedent("""
                    import json
                    import os
                    from pathlib import Path
                    import signal
                    import subprocess
                    import sys
                    import time

                    root = Path(os.environ["TEST_PARALLEL_ROOT"])
                    if "--collect-only" in sys.argv:
                        for name in "abcd":
                            print(f"tests/test_{name}.py::test_wait")
                        raise SystemExit(0)

                    name = sys.argv[-1].split("test_")[1].split(".")[0]
                    ready = root / f"child-ready-{name}"
                    child = subprocess.Popen([
                        sys.executable, "-u", "-c", CHILD_PROGRAM, str(ready),
                    ])
                    stopped = False
                    if os.environ["TEST_PARALLEL_MODE"] != "resistant":
                        def stop(signum, frame):
                            global stopped
                            stopped = True
                        signal.signal(signal.SIGTERM, stop)
                    while not ready.exists():
                        time.sleep(0.01)
                    print(f"worker diagnostic {name}", flush=True)
                    print(f"worker stderr {name}", file=sys.stderr, flush=True)
                    (root / f"started-{name}.json").write_text(json.dumps({
                        "name": name, "pid": os.getpid(), "child": child.pid,
                    }))
                    # Reap outside the handler: interrupting child.wait()
                    # and calling it again would deadlock its waitpid lock.
                    while child.poll() is None and not stopped:
                        time.sleep(0.01)
                    if stopped:
                        child.terminate()
                    child.wait()
                    """).replace("CHILD_PROGRAM", repr(child_program))
            )
            executable.chmod(0o700)
            driver = root / "driver.py"
            driver.write_text(textwrap.dedent("""
                import importlib.util
                import os
                from pathlib import Path
                import sys
                import time

                root = Path(os.environ["TEST_PARALLEL_ROOT"])
                spec = importlib.util.spec_from_file_location("parallel_runner", sys.argv[1])
                runner = importlib.util.module_from_spec(spec)
                sys.modules[spec.name] = runner
                spec.loader.exec_module(runner)
                mode = os.environ["TEST_PARALLEL_MODE"]
                if mode == "start-race":
                    popen = runner.subprocess.Popen
                    def paused_popen(*args, **kwargs):
                        process = popen(*args, **kwargs)
                        if kwargs.get("start_new_session"):
                            (root / "launch-paused").write_text(str(process.pid))
                            while not (root / "release-launch").exists():
                                time.sleep(0.01)
                        return process
                    runner.subprocess.Popen = paused_popen
                elif mode == "main-exception":
                    def fail_wait(*args, **kwargs):
                        deadline = time.monotonic() + 3
                        while len(list(root.glob("started-*.json"))) < 2:
                            if time.monotonic() >= deadline:
                                raise RuntimeError("worker startup failed")
                            time.sleep(0.01)
                        raise RuntimeError("injected main wait failure")
                    runner.wait = fail_wait
                raise SystemExit(runner.main(sys.argv[2:]))
                """))
            log = root / "runner.log"
            with log.open("w") as output:
                process = subprocess.Popen(
                    [sys.executable, str(driver), str(ROOT / "scripts" / "run_pytest_parallel.py"), *arguments],
                    cwd=ROOT,
                    env={
                        **os.environ,
                        "REPO_ROOT": str(root),
                        "VENV_DIR": str(root / "venv"),
                        "PYTEST_NUMPROCS": str(jobs),
                        "SOAC_PYTEST_PROGRESS_INTERVAL": "0.05",
                        "SOAC_PYTEST_BATCH_TIMEOUT": "0",
                        "TEST_PARALLEL_ROOT": str(root),
                        "TEST_PARALLEL_MODE": mode,
                        "PYTHONUNBUFFERED": "1",
                    },
                    stdout=output,
                    stderr=subprocess.STDOUT,
                    start_new_session=True,
                )
                try:
                    self.wait_for(lambda: len(self.records(root)) == jobs)
                    yield root, process
                finally:
                    # A failing BEFORE must not leave its own test workers.
                    if process.poll() is None:
                        os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=3)
                    for record in self.records(root):
                        try:
                            os.killpg(record["pid"], signal.SIGKILL)
                        except ProcessLookupError:
                            pass

    def assert_stopped(self, root, *, jobs):
        records = self.records(root)
        self.assertEqual(len(records), jobs, "queued batches must never launch after cancellation")
        for record in records:
            for pid in (record["pid"], record["child"]):
                self.wait_for(lambda pid=pid: not Path(f"/proc/{pid}").exists())
        output = (root / "runner.log").read_text()
        self.assertIn("descendant diagnostic", output)
        for record in records:
            self.assertIn(f"worker diagnostic {record['name']}", output)
            self.assertIn(f"worker stderr {record['name']}", output)
        return output

    def test_sigint_stops_workers_and_does_not_start_queued_batches(self):
        for jobs, arguments in (
            (2, ("tests",)),
            (2, ("--require-batch-runner", "tests")),
            (1, ("--require-batch-runner", "tests")),
        ):
            with self.subTest(jobs=jobs, arguments=arguments):
                with self.runner(jobs=jobs, arguments=arguments) as (root, process):
                    process.send_signal(signal.SIGINT)
                    self.assertEqual(process.wait(timeout=3), 128 + signal.SIGINT)
                    self.assert_stopped(root, jobs=jobs)

    def test_sigterm_kills_descendant_after_group_leader_exits(self):
        with self.runner("resistant") as (root, process):
            process.send_signal(signal.SIGTERM)
            self.assertEqual(process.wait(timeout=9), 128 + signal.SIGTERM)
            self.assert_stopped(root, jobs=2)

    def test_cancellation_waits_for_inflight_process_publication(self):
        with self.runner("start-race", jobs=1) as (root, process):
            self.wait_for(lambda: (root / "launch-paused").exists())
            process.send_signal(signal.SIGINT)
            self.wait_for(lambda: "stopping worker groups" in (root / "runner.log").read_text())
            (root / "release-launch").write_text("")
            self.assertEqual(process.wait(timeout=3), 128 + signal.SIGINT)
            self.assert_stopped(root, jobs=1)

    def test_main_exception_stops_workers_and_preserves_the_error(self):
        with self.runner("main-exception") as (root, process):
            self.assertEqual(process.wait(timeout=3), 1)
            output = self.assert_stopped(root, jobs=2)
            self.assertIn("injected main wait failure", output)


if __name__ == "__main__":
    unittest.main()
