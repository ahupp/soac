"""Raw selected-CPython managed generator ABI, without SOAC or a Python delegate."""

import gc
import hashlib
import importlib.util
import inspect
import json
import shlex
import subprocess
import sys
import sysconfig
import tempfile
import types
import unittest
import warnings
import weakref
from pathlib import Path

ERROR, RETURN, NEXT = -1, 0, 1
UNCHANGED, SUSPENDED, CLOSED = 0, 1, 2
SEND, THROW, CLOSE = 1, 2, 3
NO_SUSPEND, DIRECT, DELEGATING, ASYNC_YIELD = 0, 1, 2, 3


def source_body():
    raise AssertionError("the managed kernel must never execute source bytecode")
    yield


async def source_coroutine():
    raise AssertionError("the managed kernel must never execute coroutine bytecode")


async def source_async_generator():
    raise AssertionError(
        "the managed kernel must never execute async-generator bytecode"
    )
    yield


def owner_for(*steps, **settings):
    return {
        "bound": False,
        "cleared": False,
        "calls": [],
        "clears": 0,
        "position": 0,
        "steps": list(steps),
        **settings,
    }


def _native_probe_cppflags():
    # Use the actual configured preprocessor flags, not Py_DEBUG or -X dev.
    # Manual -I paths below already bind the current source/build headers.
    return shlex.split(sysconfig.get_config_var("CONFIGURE_CPPFLAGS") or "")


_ORDINARY_STACKREF_DEBUG_EXPORT_PROGRAM = r'''# Ordinary native StackRef-debug dynamic-linkage control. No SOAC is imported.
# Run with the actual interpreter: python -I -S -B repro.py module|symbols [path].
# An optional module path permits loading CPython's retained _failed extension.
import sys

case = sys.argv[1]
if case not in ("module", "symbols"):
    raise ValueError("expected module or symbols")
if len(sys.argv) not in (2, 3) or (case == "symbols" and len(sys.argv) != 2):
    raise ValueError("only the module case accepts an explicit extension path")
print("stackref-debug-exports: start " + case, flush=True)

if case == "module":
    if len(sys.argv) == 3:
        import importlib.util

        spec = importlib.util.spec_from_file_location("_testinternalcapi", sys.argv[2])
        if spec is None or spec.loader is None:
            raise AssertionError("cannot construct the actual native extension loader")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    else:
        import _testinternalcapi as module

    import json

    assert module.__name__ == "_testinternalcapi"
    assert module.__file__
    print(json.dumps({
        "case": case,
        "module": module.__name__,
        "extension": module.__file__,
        "executable": sys._base_executable,
    }), flush=True)
else:
    import ctypes
    import json
    import shlex
    import sysconfig

    support = (
        "_Py_stackref_get_object",
        "_Py_stackref_close",
        "_Py_stackref_create",
        "_Py_stackref_record_borrow",
        "_Py_stackref_get_borrowed_from",
        "_Py_stackref_set_borrowed_from",
    )
    helpers = (
        "PyStackRef_Is",
        "PyStackRef_UntagInt",
        "PyStackRef_TagInt",
        "PyStackRef_IncrementTaggedIntNoOverflow",
        "PyStackRef_IsNullOrInt",
    )
    # Resolve native symbols without calling a function with a guessed ABI.
    # Py_DEBUG alone is not evidence that StackRef debug handles are enabled.
    stackref_debug = getattr(ctypes.pythonapi, "_Py_stackref_create", None) is not None
    flags = shlex.split(sysconfig.get_config_var("CONFIGURE_CPPFLAGS") or "")
    explicitly_configured = (
        "-DPy_STACKREF_DEBUG=1" in flags
        and not sysconfig.get_config_var("Py_GIL_DISABLED")
    )
    required = support + helpers if stackref_debug or explicitly_configured else ()
    missing = [name for name in required if getattr(ctypes.pythonapi, name, None) is None]
    print(json.dumps({
        "case": case,
        "stackref_debug": stackref_debug,
        "configured_cppflags": flags,
        "py_debug": bool(sysconfig.get_config_var("Py_DEBUG")),
        "gil_disabled": bool(sysconfig.get_config_var("Py_GIL_DISABLED")),
        "required": required,
        "missing": missing,
        "executable": sys._base_executable,
    }), flush=True)
    assert not missing, "missing actual native StackRef-debug exports: " + repr(missing)
    assert not explicitly_configured or stackref_debug
'''


class OrdinaryStackRefDebugExportsNativeTests(unittest.TestCase):
    """The actual internal extension and debug-only helpers must remain loadable."""

    def check_stackref_debug_exports(self, case):
        result = subprocess.run(
            [
                sys._base_executable, "-I", "-S", "-B", "-c",
                _ORDINARY_STACKREF_DEBUG_EXPORT_PROGRAM, case,
            ],
            text=True, capture_output=True, check=False, timeout=30,
        )
        self.assertIn("stackref-debug-exports: start " + case, result.stdout)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        report = json.loads(result.stdout.splitlines()[-1])
        self.assertEqual(report["case"], case)
        return report

    def test_internal_capi_extension_is_loadable(self):
        report = self.check_stackref_debug_exports("module")
        self.assertEqual(report["module"], "_testinternalcapi")
        self.assertTrue(report["extension"])

    def test_debug_out_of_line_helpers_are_dynamically_exported(self):
        report = self.check_stackref_debug_exports("symbols")
        self.assertEqual(report["missing"], [])


_ORDINARY_FRAME_OVERWRITE_PROGRAM = r'''"""Ordinary real borrowed-argument/f_locals overwrite; no SOAC or C probe."""
import dis
import gc
import json
import sys
import unittest
import weakref

events = []
frames = []
reference = None


class Payload:
    def __init__(self):
        global reference
        self.marker = 47
        reference = weakref.ref(self)

    def __del__(self):
        events.append('retired')


def child(value, raise_after):
    frame = sys._getframe(1)
    frames.append(frame)
    frame.f_locals['payload'] = None
    assert value is reference()
    assert value.marker == 47
    assert events == []
    if raise_after:
        raise RuntimeError('after ancestor overwrite')
    return value.marker


def parent(raise_after):
    payload = Payload()
    return child(payload, raise_after)


class OrdinaryFrameOverwriteMinimal(unittest.TestCase):
    def check_overwrite(self, raise_after):
        instructions = list(dis.get_instructions(parent))
        opnames = [op.opname for op in instructions]
        self.assertTrue(any(
            (op.opname == 'LOAD_FAST_BORROW' and op.argval == 'payload')
            or (op.opname == 'LOAD_FAST_BORROW_LOAD_FAST_BORROW'
                and isinstance(op.argval, tuple) and 'payload' in op.argval)
            for op in instructions))
        print(json.dumps({'phase': 'before-overwrite', 'parent_opnames': opnames,
                          'raise_after': raise_after}), flush=True)
        if raise_after:
            try:
                parent(True)
            except RuntimeError as error:
                self.assertEqual(str(error), 'after ancestor overwrite')
            else:
                self.fail('RuntimeError was not raised after ancestor overwrite')
        else:
            self.assertEqual(parent(False), 47)
        self.assertEqual(len(frames), 1)
        self.assertIsNotNone(reference())
        self.assertEqual(events, [])
        gc.collect()
        self.assertIsNotNone(reference())
        frame = frames.pop()
        frame.clear()
        self.assertIsNone(reference())
        self.assertEqual(events, ['retired'])
        frame.clear()
        del frame
        gc.collect()
        self.assertEqual(events, ['retired'])
        self.assertFalse(any(name == 'soac' or name.startswith('soac.')
                             for name in sys.modules))

    def test_normal(self):
        self.check_overwrite(False)

    def test_error(self):
        self.check_overwrite(True)


if __name__ == '__main__':
    assert len(sys.argv) == 2 and sys.argv[1] in ('normal', 'error')
    unittest.main(argv=[sys.argv[0], 'OrdinaryFrameOverwriteMinimal.test_' + sys.argv[1]], verbosity=2)
'''


_ORDINARY_FRAME_OVERWRITE_LIFECYCLE_PROGRAM = r'''"""Ordinary native overwritten-local support lifecycle; no SOAC or C probe."""
import dis
import gc
import json
import sys
import weakref

case = sys.argv[1]
if case not in (
    "borrowed_escape", "growth_clear", "growth_dealloc", "cycle_gc",
    "external_tuple", "reentrant_clear",
):
    raise ValueError("unknown overwritten-local lifecycle case")

events = []
frames = []
tuples = []
references = {}


class Payload:
    def __init__(self, label):
        self.label = label
        self.frame = None
        self.replace_other = False
        references[label] = weakref.ref(self)

    def __del__(self):
        events.append(self.label)
        if self.replace_other:
            self.frame.f_locals["other"] = None


def require_borrowed_read(function, name):
    instructions = list(dis.get_instructions(function))
    borrowed = [instruction.offset for instruction in instructions if (
        instruction.opname == "LOAD_FAST_BORROW" and instruction.argval == name
    ) or (
        instruction.opname == "LOAD_FAST_BORROW_LOAD_FAST_BORROW"
        and isinstance(instruction.argval, tuple) and name in instruction.argval
    )]
    assert borrowed, (function.__name__, name, [instruction.opname for instruction in instructions])
    return {"function": function.__name__, "name": name, "borrowed_offsets": borrowed}


def frame_tuple(frame, labels):
    matches = [value for value in gc.get_referents(frame)
               if type(value) is tuple and len(value) == len(labels)
               and all(value[index] is references[label]()
                       for index, label in enumerate(labels))]
    assert len(matches) == 1
    return matches[0]


def borrowed_leaf(value):
    frame = sys._getframe(1)
    frames.append(frame)
    before = sys.getrefcount(value)
    frame.f_locals["alias"] = None
    assert value is references["borrowed"]()
    # A borrowed formal owns no object edge to release. The real tuple adds one.
    assert sys.getrefcount(value) == before + 1
    return value.label


def borrowed_middle(alias):
    return borrowed_leaf(alias)


def borrowed_outer():
    payload = Payload("borrowed")
    return borrowed_middle(payload)


def growth_leaf(first, second):
    frame = sys._getframe(1)
    frames.append(frame)
    before_first = sys.getrefcount(first)
    before_second = sys.getrefcount(second)
    frame.f_locals["first_value"] = None
    if case == "external_tuple":
        tuples.append(frame_tuple(frame, ["first"]))
    frame.f_locals["second_value"] = None
    if case == "external_tuple":
        assert sys.getrefcount(first) == before_first + 1
    else:
        assert sys.getrefcount(first) == before_first
    assert sys.getrefcount(second) == before_second
    if case != "external_tuple":
        frame.f_locals["first_value"] = first
        frame.f_locals["first_value"] = None
        assert sys.getrefcount(first) == before_first + 1
        assert sys.getrefcount(second) == before_second
    try:
        frame.clear()
    except RuntimeError:
        pass
    else:
        raise AssertionError("an executing ancestor frame must not be clearable")
    assert events == []
    assert first is references["first"]() and second is references["second"]()


def growth_parent():
    first_value = Payload("first")
    second_value = Payload("second")
    growth_leaf(first_value, second_value)


def cycle_leaf(value):
    frame = sys._getframe(1)
    value.frame = frame
    frames.append(frame)
    frame.f_locals["payload"] = None
    assert value is references["cycle"]() and events == []


def cycle_parent():
    payload = Payload("cycle")
    cycle_leaf(payload)


def reentrant_leaf(value):
    frame = sys._getframe(1)
    value.frame = frame
    value.replace_other = True
    frames.append(frame)
    frame.f_locals["trigger"] = None
    assert value is references["trigger"]() and events == []


def reentrant_parent():
    trigger = Payload("trigger")
    other = Payload("other")
    reentrant_leaf(trigger)


proof = []
if case == "borrowed_escape":
    proof.extend([
        require_borrowed_read(borrowed_outer, "payload"),
        require_borrowed_read(borrowed_middle, "alias"),
    ])
elif case in ("growth_clear", "growth_dealloc", "external_tuple"):
    proof.extend([
        require_borrowed_read(growth_parent, "first_value"),
        require_borrowed_read(growth_parent, "second_value"),
    ])
elif case == "cycle_gc":
    proof.append(require_borrowed_read(cycle_parent, "payload"))
else:
    proof.append(require_borrowed_read(reentrant_parent, "trigger"))
print(json.dumps({"phase": "before-overwrite", "case": case, "native_reads": proof}), flush=True)

if case == "borrowed_escape":
    assert borrowed_outer() == "borrowed"
    assert len(frames) == 1 and events == []
    assert references["borrowed"]() is not None
    frame = frames.pop()
    frame.clear()
    # The middle frame still owns f_back; its outer caller owns payload.
    assert references["borrowed"]() is not None and events == []
    frame.clear()
    assert references["borrowed"]() is not None and events == []
    del frame
    # Deleting the last middle-frame owner releases that real ancestry chain.
    assert references["borrowed"]() is None and events == ["borrowed"]
elif case in ("growth_clear", "growth_dealloc", "external_tuple"):
    growth_parent()
    assert len(frames) == 1 and events == []
    assert references["first"]() is not None and references["second"]() is not None
    frame = frames.pop()
    if case == "growth_dealloc":
        del frame
    elif case == "growth_clear":
        frame.clear()
        frame.clear()
        del frame
    else:
        retained = tuples.pop()
        original_identity = id(retained)
        frame.clear()
        assert events == ["second"]
        assert references["second"]() is None and references["first"]() is not None
        assert id(retained) == original_identity
        assert len(retained) == 1 and retained[0] is references["first"]()
        frame.clear()
        del frame
        assert events == ["second"]
        del retained
    assert events == ["second", "first"]
    assert references["first"]() is None and references["second"]() is None
elif case == "cycle_gc":
    cycle_parent()
    assert len(frames) == 1 and events == []
    frame = frames.pop()
    del frame
    assert references["cycle"]() is not None and events == []
    gc.collect()
    assert references["cycle"]() is None and events == ["cycle"]
else:
    reentrant_parent()
    assert len(frames) == 1 and events == []
    frame = frames.pop()
    frame.clear()
    assert events == ["trigger"]
    assert references["trigger"]() is None and references["other"]() is not None
    # The first tuple's finalizer created this new support; it must not be lost.
    frame.clear()
    assert events == ["trigger", "other"]
    assert references["other"]() is None
    del frame

before_gc = events.copy()
gc.collect()
assert events == before_gc
assert not frames and not tuples
assert not any(name == "soac" or name.startswith("soac.") for name in sys.modules)
print(json.dumps({"phase": "complete", "case": case, "events": events,
                  "all_payloads_retired": all(reference() is None for reference in references.values())}), flush=True)
'''


class OrdinaryFrameOverwriteNativeTests(unittest.TestCase):
    """Real native local/argument borrows and the frame's existing tuple support."""

    def check_overwrite_program(self, program, case):
        result = subprocess.run(
            [sys._base_executable, "-I", "-S", "-B", "-c", program, case],
            text=True, capture_output=True, check=False, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_owned_ancestor_overwrite_and_explicit_clear(self):
        self.check_overwrite_program(_ORDINARY_FRAME_OVERWRITE_PROGRAM, "normal")

    def test_owned_ancestor_overwrite_error_and_explicit_clear(self):
        self.check_overwrite_program(_ORDINARY_FRAME_OVERWRITE_PROGRAM, "error")

    def test_borrowed_intermediate_frame_can_outlive_its_original_parent(self):
        self.check_overwrite_program(_ORDINARY_FRAME_OVERWRITE_LIFECYCLE_PROGRAM, "borrowed_escape")

    def test_repeated_overwrite_tuple_growth_and_clear_order(self):
        self.check_overwrite_program(_ORDINARY_FRAME_OVERWRITE_LIFECYCLE_PROGRAM, "growth_clear")

    def test_repeated_overwrite_tuple_growth_and_deallocation_order(self):
        self.check_overwrite_program(_ORDINARY_FRAME_OVERWRITE_LIFECYCLE_PROGRAM, "growth_dealloc")

    def test_overwritten_local_frame_cycle_has_no_hidden_gc_owner(self):
        self.check_overwrite_program(_ORDINARY_FRAME_OVERWRITE_LIFECYCLE_PROGRAM, "cycle_gc")

    def test_external_original_tuple_keeps_its_identity_and_lifetime(self):
        self.check_overwrite_program(_ORDINARY_FRAME_OVERWRITE_LIFECYCLE_PROGRAM, "external_tuple")

    def test_tuple_finalizer_can_publish_fresh_overwrite_during_clear(self):
        self.check_overwrite_program(_ORDINARY_FRAME_OVERWRITE_LIFECYCLE_PROGRAM, "reentrant_clear")


class _NativeProbeTestCase(unittest.TestCase):
    probe_extra_cflags = ()

    @classmethod
    def setUpClass(cls):
        source = Path(sysconfig.get_config_var("abs_srcdir")).resolve(strict=True)
        build = Path(sysconfig.get_config_var("abs_builddir")).resolve(strict=True)
        if (build / "python").resolve() != Path(sys._base_executable).resolve():
            raise AssertionError(
                "native test requires the running build's real sysconfig"
            )
        cls.temporary = tempfile.TemporaryDirectory(prefix="soac-managed-generator-")
        cls.addClassCleanup(cls.temporary.cleanup)
        output = Path(cls.temporary.name)
        extension = output / (
            "_strict_managed_generator" + sysconfig.get_config_var("EXT_SUFFIX")
        )
        command = [
            *shlex.split(sysconfig.get_config_var("LDSHARED")),
            *shlex.split(sysconfig.get_config_var("CCSHARED")),
            *_native_probe_cppflags(),
            *cls.probe_extra_cflags,
            "-O0",
            "-g",
            "-Wall",
            "-Wextra",
            "-Werror",
            f"-I{source / 'Include'}",
            f"-I{build}",
            str(Path(__file__).parent / "native" / "managed_generator.c"),
            "-o",
            str(extension),
        ]
        result = subprocess.run(
            command, capture_output=True, text=True, timeout=120, check=False
        )
        (output / "build.log").write_text(
            shlex.join(command) + "\n" + result.stdout + result.stderr
        )
        if result.returncode:
            raise AssertionError(f"native managed probe build failed:\n{result.stderr}")
        spec = importlib.util.spec_from_file_location(
            "_strict_managed_generator", extension
        )
        cls.probe = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.probe)
        libraries = sorted(
            {
                line.split()[-1]
                for line in Path("/proc/self/maps").read_text().splitlines()
                if "libpython" in line
            }
        )
        if not libraries or any(Path(path).parent != build for path in libraries):
            raise AssertionError(f"native probe loaded a different build: {libraries}")
        print(
            json.dumps(
                {
                    "managed_probe_inputs": {
                        "source": str(source),
                        "build": str(build),
                        "executable": sys.executable,
                        "libraries": {
                            path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
                            for path in libraries
                        },
                        "probe": str(extension),
                        "probe_sha256": hashlib.sha256(
                            extension.read_bytes()
                        ).hexdigest(),
                        "command": command,
                    }
                },
                sort_keys=True,
            ),
            flush=True,
        )


class ManagedGeneratorNativeTests(_NativeProbeTestCase):
    def new(self, owner=None, function=source_body):
        if owner is None:
            owner = owner_for((NEXT, SUSPENDED, 11), (RETURN, CLOSED, 23))
        return self.probe.new(function, owner), owner

    def assert_no_native_exception_item(self, generator):
        self.assertEqual(self.probe.state(generator)[1:], (1, 1))

    def test_exact_native_type_metadata_and_created_close(self):
        generator, owner = self.new()
        self.assertIs(type(generator), types.GeneratorType)
        self.assertIs(iter(generator), generator)
        self.assertIs(generator.gi_code, source_body.__code__)
        self.assertEqual(generator.__name__, source_body.__name__)
        self.assertEqual(generator.gi_state, "GEN_CREATED")
        self.assertFalse(generator.gi_running)
        self.assertFalse(generator.gi_suspended)
        self.assertIsNone(generator.gi_yieldfrom)
        self.assertEqual(self.probe.matches(generator, owner), 1)
        self.assertEqual(self.probe.matches(generator, {}), 0)
        self.assert_no_native_exception_item(generator)
        self.assertIsNone(generator.close())
        self.assertEqual(owner["calls"], [])
        self.assertEqual(owner["clears"], 1)
        self.assertIs(generator.gi_code, source_body.__code__)
        with self.assertRaisesRegex(RuntimeError, "terminal"):
            self.probe.matches(generator, owner)
        self.assertIsNone(generator.close())
        self.assertEqual(owner["clears"], 1)

    def test_send_next_return_and_native_c_api(self):
        token = object()
        generator, owner = self.new(
            owner_for((NEXT, SUSPENDED, token), (RETURN, CLOSED, 9))
        )
        with self.assertRaisesRegex(TypeError, "just-started generator"):
            generator.send(42)
        self.assertEqual(owner["calls"], [])
        self.assertEqual(generator.gi_state, "GEN_CREATED")
        self.assertEqual(self.probe.send(generator, None), (NEXT, token))
        self.assertEqual(generator.gi_state, "GEN_SUSPENDED")
        self.assertTrue(generator.gi_suspended)
        self.assert_no_native_exception_item(generator)
        self.assertEqual(self.probe.send(generator, token), (RETURN, 9))
        self.assertIs(owner["calls"][1][2], token)
        self.assertEqual(owner["clears"], 1)
        self.assertEqual(self.probe.send(generator, None), (RETURN, None))
        with self.assertRaises(StopIteration) as completed:
            generator.send(None)
        self.assertIsNone(completed.exception.value)

    def test_names_and_source_code_are_explicit_non_authority_snapshots(self):
        function = types.FunctionType(source_body.__code__, globals(), "renamed")
        function.__qualname__ = "outer.renamed"
        generator, _ = self.new(function=function)
        self.assertEqual(generator.__name__, "renamed")
        self.assertEqual(generator.__qualname__, "outer.renamed")
        function.__name__ = "later"
        self.assertEqual(generator.__name__, "renamed")
        generator.close()
        expression = (value for value in ())
        code = expression.gi_code
        expression.close()
        owner = owner_for()
        generator = self.probe.new(
            function,
            owner,
            code,
            self.probe.layout()["abi"],
            0,
            code.co_name,
            code.co_qualname,
        )
        self.assertIs(generator.gi_code, code)
        self.assertIs(owner["function"], function)
        self.assertIs(owner["code"], code)
        self.assertEqual(generator.__name__, "<genexpr>")
        self.assertEqual(generator.__qualname__, code.co_qualname)
        generator.close()

    def test_invalid_constructor_inputs_do_not_bind_an_owner(self):
        owner = owner_for()
        current_abi = self.probe.layout()["abi"]
        for code, abi, reserved in (
            (source_body.__code__, 9, 0),
            (source_body.__code__, current_abi, 7),
            ((lambda: None).__code__, current_abi, 0),
        ):
            with self.subTest(abi=abi, reserved=reserved, code=code):
                with self.assertRaises(TypeError):
                    self.probe.new(source_body, owner, code, abi, reserved)
                self.assertFalse(owner["bound"])
                self.assertEqual(owner["clears"], 0)

    def test_tp_iternext_and_send_preserve_return_values(self):
        for entry in (next, lambda generator: generator.send(None)):
            with self.subTest(entry=entry):
                result = (17, "result")
                generator, owner = self.new(owner_for((RETURN, CLOSED, result)))
                with self.assertRaises(StopIteration) as completed:
                    entry(generator)
                self.assertIs(completed.exception.value, result)
                self.assertEqual(owner["clears"], 1)
                self.assertIsNone(next(generator, None))

    def test_throw_raw_operands_are_not_normalized_or_validated_before_callback(self):
        try:
            raise LookupError("traceback")
        except LookupError as error:
            traceback = error.__traceback__
        marker = object()
        arguments = (
            (marker,),
            (marker, None),
            (marker, None, None),
            (marker, "value", traceback),
        )
        for args in arguments:
            with self.subTest(arity=len(args)):
                generator, owner = self.new(owner_for((NEXT, SUSPENDED, "delegated")))
                with warnings.catch_warnings():
                    warnings.simplefilter("ignore", DeprecationWarning)
                    self.assertEqual(generator.throw(*args), "delegated")
                call = owner["calls"][0]
                self.assertEqual(call[:2], (THROW, 1))
                self.assertIs(call[2], marker)
                self.assertEqual(call[5:8], (False, len(args) < 2, len(args) < 3))
                if len(args) > 1:
                    self.assertIs(call[3], args[1])
                if len(args) > 2:
                    self.assertIs(call[4], args[2])
                generator.close()

    def test_created_throw_validity_normalization_and_no_body(self):
        generator, owner = self.new(owner_for(normalize_throw=True))
        with self.assertRaisesRegex(TypeError, "exceptions must be"):
            generator.throw(17)
        self.assertEqual(generator.gi_state, "GEN_CREATED")
        self.assertEqual(owner["clears"], 0)
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", DeprecationWarning)
            with self.assertRaisesRegex(TypeError, "third argument"):
                generator.throw(ValueError, None, object())
        self.assertEqual(generator.gi_state, "GEN_CREATED")
        exact = ValueError("created throw")
        with self.assertRaises(ValueError) as raised:
            generator.throw(exact)
        self.assertIs(raised.exception, exact)
        self.assertEqual(generator.gi_state, "GEN_CLOSED")
        self.assertEqual(owner["clears"], 1)
        self.assertEqual(owner["position"], 0)
        self.assert_no_native_exception_item(generator)

    def test_normalizer_preserves_native_constructor_and_traceback_rules(self):
        events = []
        outer = KeyError("caller")

        class Observed(ValueError):
            def __init__(self, *args):
                events.append((args, sys.exception()))
                super().__init__(*args)

        try:
            raise outer
        except KeyError:
            normalized = self.probe.normalize(Observed, (1, 2))
            self.assertIs(sys.exception(), outer)
        self.assertEqual(normalized.args, (1, 2))
        self.assertEqual(events, [((1, 2), outer)])
        self.assertIs(self.probe.normalize(normalized), normalized)
        with self.assertRaisesRegex(TypeError, "separate value"):
            self.probe.normalize(normalized, 12)

    def test_callback_runs_under_caller_item_and_errors_survive_retirement(self):
        original = LookupError("original")
        clobber = RuntimeError("clear callback")
        generator, owner = self.new(
            owner_for((ERROR, CLOSED, original), clear_error=clobber)
        )
        caller = KeyError("caller")
        try:
            raise caller
        except KeyError:
            with self.assertRaises(LookupError) as raised:
                next(generator)
            self.assertIs(sys.exception(), caller)
        self.assertIs(raised.exception, original)
        self.assertIs(owner["calls"][0][-1], caller)
        self.assertEqual(owner["clears"], 1)
        self.assert_no_native_exception_item(generator)

    def test_prebody_error_does_not_close_a_suspended_generator(self):
        error = AttributeError("delegate throw lookup")
        generator, owner = self.new(
            owner_for(
                (NEXT, SUSPENDED, 1), (ERROR, UNCHANGED, error), (NEXT, SUSPENDED, 2)
            )
        )
        self.assertEqual(next(generator), 1)
        with self.assertRaises(AttributeError) as raised:
            generator.throw(RuntimeError)
        self.assertIs(raised.exception, error)
        self.assertEqual(generator.gi_state, "GEN_SUSPENDED")
        self.assertEqual(owner["clears"], 0)
        self.assertEqual(next(generator), 2)
        generator.close()

    def test_close_return_error_and_ignored_exit(self):
        for outcome in ("return", "exit", "ignored"):
            with self.subTest(outcome=outcome):
                action = {
                    "return": (RETURN, CLOSED, 37),
                    "exit": (ERROR, CLOSED, GeneratorExit()),
                    "ignored": (NEXT, SUSPENDED, 51),
                }[outcome]
                generator, owner = self.new(owner_for((NEXT, SUSPENDED, 1), action))
                self.assertEqual(next(generator), 1)
                if outcome == "ignored":
                    with self.assertRaisesRegex(RuntimeError, "ignored GeneratorExit"):
                        generator.close()
                    self.assertEqual(generator.gi_state, "GEN_SUSPENDED")
                    self.assertEqual(owner["clears"], 0)
                    generator.close()
                else:
                    self.assertEqual(
                        generator.close(), 37 if outcome == "return" else None
                    )
                self.assertEqual(owner["calls"][1][:2], (CLOSE, 1))
                self.assertEqual(owner["calls"][1][5:8], (True, True, True))
                self.assertEqual(owner["clears"], 1)

    def test_native_reentrancy_rejects_before_callback(self):
        for method in ("send", "throw", "close"):
            with self.subTest(method=method):
                generator, owner = self.new(
                    owner_for((NEXT, SUSPENDED, 1), reenter=method)
                )
                self.assertEqual(next(generator), 1)
                self.assertEqual(len(owner["calls"]), 1)
                self.assertEqual(
                    str(owner["reentrant_error"]), "generator already executing"
                )
                owner["reenter"] = None
                generator.close()

    def test_failed_bind_leaves_escaped_generator_terminal(self):
        error = LookupError("bind failed")
        owner = owner_for(
            bind_error=error,
            escaped=[],
            reenter_bind=True,
            clear_error=RuntimeError("cleanup must not replace bind error"),
        )
        with self.assertRaises(LookupError) as raised:
            self.probe.new(source_body, owner)
        self.assertIs(raised.exception, error)
        (generator,) = owner["escaped"]
        self.assertEqual(generator.gi_state, "GEN_CLOSED")
        self.assertEqual(owner["calls"], [])
        self.assertEqual(owner["clears"], 1)
        self.assertIsNone(next(generator, None))
        with self.assertRaisesRegex(RuntimeError, "terminal"):
            self.probe.matches(generator, owner)

    def test_owner_reuse_cannot_retire_the_original_generator(self):
        generator, owner = self.new()
        with self.assertRaisesRegex(ValueError, "already bound"):
            self.probe.new(source_body, owner)
        self.assertEqual(owner["clears"], 0)
        self.assertEqual(self.probe.matches(generator, owner), 1)
        self.assertEqual(next(generator), 11)
        generator.close()
        self.assertEqual(owner["clears"], 1)

    def test_invalid_result_protocol_is_terminal_before_cleanup(self):
        cases = [
            (NEXT, CLOSED, 1),
            (RETURN, SUSPENDED, 2),
            (ERROR, UNCHANGED, None),
            (42, SUSPENDED, 3),
        ]
        for script in cases:
            with self.subTest(script=script):
                generator, owner = self.new(owner_for(script))
                with self.assertRaisesRegex(
                    SystemError, "managed generator callback result"
                ):
                    next(generator)
                self.assertEqual(generator.gi_state, "GEN_CLOSED")
                self.assertEqual(owner["clears"], 1)
                self.assertIsNone(next(generator, None))
        generator, owner = self.new(
            owner_for((NEXT, SUSPENDED, 1), step_error=ValueError("result with error"))
        )
        with self.assertRaisesRegex(SystemError, "managed generator callback result"):
            next(generator)
        self.assertEqual(owner["clears"], 1)

    def test_terminal_notification_precedes_callback_local_cleanup(self):
        def payload_for(generator, owner, observations, injected, caller, probe):
            def observe(call):
                try:
                    value = call()
                except BaseException as error:  # noqa: BLE001 - record every reentrant outcome.
                    return (
                        "error",
                        type(error).__name__,
                        error.args,
                        error is injected,
                    )
                return ("value", value)

            class Payload:
                def __del__(self):
                    observations.append(
                        (
                            generator.gi_running,
                            generator.gi_state,
                            generator.gi_suspended,
                            generator.gi_yieldfrom,
                            observe(lambda: generator.send(None)),
                            observe(lambda: generator.throw(injected)),
                            observe(generator.close),
                            owner["clears"],
                            observe(lambda: probe.matches(generator, owner)),
                            observe(lambda: probe.mark_terminal(generator, owner)),
                            sys.exception() is caller,
                        )
                    )

            return Payload()

        for operation in ("return", "error", "close"):
            with self.subTest(operation=operation):
                original = LookupError("original terminal error")
                returned = ("returned", 41)
                outcome = (
                    (ERROR, CLOSED, original)
                    if operation == "error"
                    else (RETURN, CLOSED, returned)
                )
                generator, owner = self.new(
                    owner_for((NEXT, SUSPENDED, "ready"), outcome)
                )
                self.assertEqual(next(generator), "ready")
                observations = []
                injected = RuntimeError("reentrant throw")
                caller = KeyError("caller")
                owner["release_callback_payload"] = True
                owner["callback_payload"] = payload_for(
                    generator, owner, observations, injected, caller, self.probe
                )
                try:
                    raise caller
                except KeyError:
                    if operation == "error":
                        with self.assertRaises(LookupError) as raised:
                            next(generator)
                        self.assertIs(raised.exception, original)
                    elif operation == "close":
                        self.assertIs(generator.close(), returned)
                    else:
                        with self.assertRaises(StopIteration) as completed:
                            next(generator)
                        self.assertIs(completed.exception.value, returned)
                    self.assertIs(sys.exception(), caller)
                self.assertEqual(
                    observations,
                    [
                        (
                            False,
                            "GEN_CLOSED",
                            False,
                            None,
                            ("error", "StopIteration", (), False),
                            ("error", "RuntimeError", injected.args, True),
                            ("value", None),
                            0,
                            (
                                "error",
                                "RuntimeError",
                                ("managed generator association is terminal",),
                                False,
                            ),
                            ("value", None),
                            True,
                        )
                    ],
                )
                self.assertEqual(owner["clears"], 1)
                self.assertEqual(len(owner["calls"]), 2)
                self.assert_no_native_exception_item(generator)

    def test_terminal_notification_requires_exact_active_owner(self):
        ordinary = source_body()
        self.addCleanup(ordinary.close)
        for value in (object(), ordinary):
            with (
                self.subTest(value_type=type(value)),
                self.assertRaisesRegex(RuntimeError, "managed generator"),
            ):
                self.probe.mark_terminal(value, {})
        generator, owner = self.new()
        for state in ("GEN_CREATED", "GEN_SUSPENDED"):
            with self.subTest(state=state):
                with self.assertRaisesRegex(RuntimeError, "active native step"):
                    self.probe.mark_terminal(generator, owner)
                with self.assertRaisesRegex(RuntimeError, "does not own"):
                    self.probe.mark_terminal(generator, {})
                owner["attempt_terminal_owner"] = {}
                with self.assertRaisesRegex(RuntimeError, "does not own"):
                    next(generator)
                del owner["attempt_terminal_owner"]
                self.assertEqual(generator.gi_state, state)
                self.assertEqual(self.probe.matches(generator, owner), 1)
                self.assertEqual(owner["clears"], 0)
                if state == "GEN_CREATED":
                    self.assertEqual(next(generator), 11)
        generator.close()
        with self.assertRaises(RuntimeError):
            self.probe.mark_terminal(generator, owner)
        self.assertEqual(owner["clears"], 1)

    def test_terminal_notification_cannot_yield_or_restore_prior_state(self):
        for script in ((NEXT, SUSPENDED, 1), (ERROR, UNCHANGED, LookupError("late"))):
            with self.subTest(script=script):
                generator, owner = self.new(
                    owner_for(
                        script,
                        release_callback_payload=True,
                        callback_payload=object(),
                    )
                )
                with self.assertRaisesRegex(
                    SystemError, "managed generator callback result"
                ):
                    next(generator)
                self.assertEqual(generator.gi_state, "GEN_CLOSED")
                self.assertEqual(owner["clears"], 1)

    def test_owner_cycle_and_finalization_are_gc_visible(self):
        class Payload:
            pass

        owner = owner_for((NEXT, SUSPENDED, 1))
        generator, _ = self.new(owner)
        payload = Payload()
        payload.generator = generator
        owner["payload"] = payload
        gen_ref, payload_ref = weakref.ref(generator), weakref.ref(payload)
        self.assertEqual(next(generator), 1)
        del generator, owner, payload, _
        gc.collect()
        self.assertIsNone(gen_ref())
        self.assertIsNone(payload_ref())

    def test_visible_delegate_and_warmed_native_iteration_paths(self):
        def ordinary():
            yield 1
            yield 2

        def loop(generator):
            total = 0
            for value in generator:
                total += value
            return total

        def delegator(generator):
            return (yield from generator)

        for _ in range(100):
            self.assertEqual(loop(ordinary()), 3)
            self.assertEqual(list(delegator(ordinary())), [1, 2])
        for _ in range(40):
            owner = owner_for((NEXT, SUSPENDED, 7), (NEXT, SUSPENDED, 8))
            generator, _owner = self.new(owner)
            self.assertEqual(loop(generator), 15)
            owner = owner_for((NEXT, SUSPENDED, DELEGATING, 9), (RETURN, CLOSED, 12))
            generator, _owner = self.new(owner)
            parent = delegator(generator)
            self.assertEqual(next(parent), 9)
            self.assertIs(parent.gi_yieldfrom, generator)
            delegate = iter([1])
            owner["delegate"] = delegate
            self.assertIs(generator.gi_yieldfrom, delegate)
            with self.assertRaises(StopIteration) as completed:
                next(parent)
            self.assertEqual(completed.exception.value, 12)
            self.assertIsNone(generator.gi_yieldfrom)
            self.assert_no_native_exception_item(generator)

    def test_header_layout_has_distinct_common_managed_metadata(self):
        layout = self.probe.layout()
        self.assertEqual(layout["gen_metadata"], layout["coro_metadata"])
        self.assertEqual(layout["gen_metadata"], layout["asyncgen_metadata"])
        self.assertNotEqual(layout["coro_metadata"], layout["coro_origin"])
        self.assertNotEqual(layout["asyncgen_metadata"], layout["asyncgen_finalizer"])
        self.assertEqual(layout["gen_frame"], layout["coro_frame"])
        self.assertEqual(layout["gen_frame"], layout["asyncgen_frame"])
        self.assertEqual(layout["abi"], 2)
        self.assertEqual(
            (layout["spec"], layout["input"], layout["result"]), (40, 32, 24)
        )

    def test_delegate_throw_distinguishes_missing_lookup_error_and_called_error(self):
        self.assertEqual(
            self.probe.throw_delegate(object(), 1, ValueError), (0, None, None)
        )
        lookup_error = LookupError("throw attribute")
        called_error = RuntimeError("throw call")

        class LookupFailure:
            @property
            def throw(self):
                raise lookup_error

        class CallFailure:
            def throw(self, *args):
                raise called_error

        status, value, error = self.probe.throw_delegate(LookupFailure(), 1, ValueError)
        self.assertEqual((status, value), (-1, None))
        self.assertIs(error, lookup_error)
        status, value, error = self.probe.throw_delegate(CallFailure(), 1, ValueError)
        self.assertEqual((status, value), (1, None))
        self.assertIs(error, called_error)

    def test_delegate_throw_keeps_raw_arguments_and_caller_exception(self):
        calls = []
        marker = object()
        caller = KeyError("caller")

        class Delegate:
            @property
            def throw(self):
                calls.append(("lookup", sys.exception()))

                def invoke(*args):
                    calls.append(("call", args, sys.exception()))
                    return marker

                return invoke

        try:
            raise caller
        except KeyError:
            self.assertEqual(
                self.probe.throw_delegate(Delegate(), 1, marker, None, None),
                (1, marker, None),
            )
            self.assertIs(sys.exception(), caller)
        self.assertEqual(
            calls, [("lookup", caller), ("call", (marker, None, None), caller)]
        )

    def test_exact_generator_delegation_does_not_emit_an_extra_throw_warning(self):
        def inner():
            try:
                yield "ready"
            except ValueError as error:
                yield error.args

        generator = inner()
        self.assertEqual(next(generator), "ready")
        with warnings.catch_warnings(record=True) as reported:
            warnings.simplefilter("always", DeprecationWarning)
            self.assertEqual(
                self.probe.throw_delegate(generator, 1, ValueError, "payload", None),
                (1, ("payload",), None),
            )
        self.assertEqual(reported, [])
        generator.close()
        ordinary = inner()
        next(ordinary)
        with warnings.catch_warnings(record=True) as reported:
            warnings.simplefilter("always", DeprecationWarning)
            self.assertEqual(ordinary.throw(ValueError, "payload", None), ("payload",))
        self.assertEqual(len(reported), 1)
        ordinary.close()

    def test_delegate_close_uses_native_lookup_unraisable_and_call_errors(self):
        lookup_error = LookupError("close lookup")
        call_error = RuntimeError("close call")
        captured = []

        class LookupFailure:
            @property
            def close(self):
                raise lookup_error

        class CallFailure:
            def close(self):
                raise call_error

        previous = sys.unraisablehook
        sys.unraisablehook = captured.append
        try:
            self.assertIsNone(self.probe.close_delegate(LookupFailure()))
        finally:
            sys.unraisablehook = previous
        self.assertEqual(len(captured), 1)
        self.assertIs(captured[0].exc_value, lookup_error)
        self.assertIsNone(self.probe.close_delegate(object()))
        with self.assertRaises(RuntimeError) as raised:
            self.probe.close_delegate(CallFailure())
        self.assertIs(raised.exception, call_error)

    def test_delegate_close_exact_generator_and_coroutine_keep_native_semantics(self):
        events = []

        def inner():
            try:
                yield "ready"
            finally:
                events.append(sys.exception())

        async def coroutine():
            return 1

        generator = inner()
        next(generator)
        self.assertIsNone(self.probe.close_delegate(generator))
        self.assertEqual(generator.gi_state, "GEN_CLOSED")
        self.assertIsInstance(events[0], GeneratorExit)
        suspended = coroutine()
        self.assertIsNone(self.probe.close_delegate(suspended))
        self.assertIsNone(suspended.cr_frame)

    def test_managed_coroutine_and_async_generator_keep_exact_native_types(self):
        for function, native_type, prefix in (
            (source_coroutine, types.CoroutineType, "cr"),
            (source_async_generator, types.AsyncGeneratorType, "ag"),
        ):
            with self.subTest(kind=prefix):
                ordinary = function()
                self.assertIs(type(ordinary), native_type)
                self.assertIsNotNone(getattr(ordinary, prefix + "_frame"))
                if prefix == "cr":
                    ordinary.close()
                else:
                    with self.assertRaises(StopIteration):
                        ordinary.aclose().send(None)
                self.assertIsNone(getattr(ordinary, prefix + "_frame"))

                managed, owner = self.new(owner_for(), function)
                self.assertIs(type(managed), native_type)
                self.assertIs(getattr(managed, prefix + "_code"), function.__code__)
                self.assertEqual(self.probe.matches(managed, owner), 1)
                self.assert_no_native_exception_item(managed)
                if prefix == "cr":
                    managed.close()
                else:
                    with self.assertRaises(StopIteration):
                        managed.aclose().send(None)
                self.assertEqual(owner["clears"], 1)

    def test_managed_coroutine_preserves_running_state_and_concurrent_await_guard(self):
        token = object()
        observations = []

        class Pause:
            def __await__(self):
                return (yield token)

        async def forward(coroutine):
            return await coroutine

        def observe(coroutine):
            observations.append(
                (coroutine.cr_state, coroutine.cr_running, coroutine.cr_suspended)
            )

        for managed in (False, True):
            with self.subTest(managed=managed):
                observations.clear()
                holder = []

                async def ordinary(active):
                    observe(active[0])
                    await Pause()
                    observe(active[0])
                    return 29

                owner = owner_for(
                    (NEXT, SUSPENDED, DELEGATING, token),
                    (RETURN, CLOSED, 29),
                    observe=observe,
                    delegate=iter((token,)),
                )
                coroutine = (
                    self.new(owner, source_coroutine)[0]
                    if managed
                    else ordinary(holder)
                )
                holder.append(coroutine)
                self.assertEqual(inspect.getcoroutinestate(coroutine), "CORO_CREATED")
                with self.assertRaises(TypeError):
                    iter(coroutine)
                self.assertIsNot(coroutine.__await__(), coroutine)
                with self.assertRaisesRegex(TypeError, "just-started coroutine"):
                    coroutine.send(1)
                self.assertEqual(self.probe.send(coroutine, None), (NEXT, token))
                self.assertEqual(observations, [("CORO_RUNNING", True, False)])
                self.assertEqual(inspect.getcoroutinestate(coroutine), "CORO_SUSPENDED")
                self.assertTrue(coroutine.cr_suspended)
                self.assertFalse(coroutine.cr_running)
                self.assertIsNotNone(coroutine.cr_await)
                second_await = forward(coroutine)
                with self.assertRaisesRegex(RuntimeError, "being awaited already"):
                    second_await.send(None)
                self.assertEqual(len(observations), 1)
                self.assertEqual(self.probe.send(coroutine, 23), (RETURN, 29))
                self.assertEqual(observations, [("CORO_RUNNING", True, False)] * 2)
                self.assertEqual(inspect.getcoroutinestate(coroutine), "CORO_CLOSED")
                if not managed:
                    self.assertIsNone(coroutine.cr_frame)
                self.assertIsNone(coroutine.cr_await)
                with self.assertRaisesRegex(
                    RuntimeError, "cannot reuse already awaited"
                ):
                    self.probe.send(coroutine, None)
                if managed:
                    self.assertEqual(owner["clears"], 1)
                    self.assert_no_native_exception_item(coroutine)

    def test_managed_coroutine_warmed_native_await_does_not_push_a_frame(self):
        async def forward(coroutine):
            return await coroutine

        async def ordinary():
            return 7

        for _ in range(100):
            with self.assertRaises(StopIteration) as completed:
                forward(ordinary()).send(None)
            self.assertEqual(completed.exception.value, 7)
        for _ in range(40):
            coroutine, owner = self.new(
                owner_for((RETURN, CLOSED, 19)), source_coroutine
            )
            with self.assertRaises(StopIteration) as completed:
                forward(coroutine).send(None)
            self.assertEqual(completed.exception.value, 19)
            self.assertEqual(owner["clears"], 1)
            self.assert_no_native_exception_item(coroutine)

    def test_ordinary_coroutine_origin_and_unobserved_managed_retirement(self):
        previous = sys.get_coroutine_origin_tracking_depth()
        try:
            sys.set_coroutine_origin_tracking_depth(2)
            ordinary = source_coroutine()
            self.assertIsInstance(ordinary.cr_origin, tuple)
            self.assertTrue(ordinary.cr_origin)
            ordinary.close()
            # Managed source ancestry and observer refusal are not part of the
            # SOAC contract. Keep its actual owner/retirement control unobserved.
            sys.set_coroutine_origin_tracking_depth(0)
            coroutine, owner = self.new(owner_for(), source_coroutine)
            self.assertTrue(owner["bound"])
            coroutine.close()
            self.assertEqual(owner["clears"], 1)
        finally:
            sys.set_coroutine_origin_tracking_depth(previous)

    def test_managed_coroutine_keeps_native_unawaited_warning_and_retirement(self):
        previous = sys.get_coroutine_origin_tracking_depth()
        try:
            sys.set_coroutine_origin_tracking_depth(0)
            for managed in (False, True):
                with (
                    self.subTest(managed=managed),
                    warnings.catch_warnings(record=True) as seen,
                ):
                    warnings.simplefilter("always")
                    owner = owner_for()
                    coroutine = (
                        self.new(owner, source_coroutine)[0]
                        if managed
                        else source_coroutine()
                    )
                    witness = weakref.ref(coroutine)
                    del coroutine
                    gc.collect()
                    self.assertIsNone(witness())
                    self.assertEqual(len(seen), 1)
                    self.assertIs(seen[0].category, RuntimeWarning)
                    self.assertIn("was never awaited", str(seen[0].message))
                    # Captured WarningMessage.source resurrects the coroutine
                    # after native weakrefs have already been cleared.
                    self.assertIs(type(seen[0].source), types.CoroutineType)
                    if managed:
                        self.assertEqual(owner["clears"], 0)
                        self.assertEqual(self.probe.matches(seen[0].source, owner), 1)
                    seen.clear()
                    gc.collect()
                    if managed:
                        self.assertEqual(owner["clears"], 1)
        finally:
            sys.set_coroutine_origin_tracking_depth(previous)

    def test_managed_async_generator_keeps_native_operation_ownership(self):
        token = object()
        observations = []

        class Pause:
            def __await__(self):
                return (yield token)

        def observe(generator):
            observations.append(
                (generator.ag_state, generator.ag_running, generator.ag_suspended)
            )

        native_operation_type = None
        for managed in (False, True):
            with self.subTest(managed=managed):
                observations.clear()
                holder = []

                async def ordinary(active):
                    observe(active[0])
                    await Pause()
                    observe(active[0])
                    yield 29
                    observe(active[0])

                owner = owner_for(
                    (NEXT, SUSPENDED, DELEGATING, token),
                    (NEXT, SUSPENDED, ASYNC_YIELD, 29),
                    (RETURN, CLOSED, None),
                    observe=observe,
                    wrap_async_yield=True,
                    delegate=iter((token,)),
                )
                generator = (
                    self.new(owner, source_async_generator)[0]
                    if managed
                    else ordinary(holder)
                )
                holder.append(generator)
                self.assertEqual(inspect.getasyncgenstate(generator), "AGEN_CREATED")
                operation = generator.__anext__()
                if managed:
                    self.assertIs(type(operation), native_operation_type)
                else:
                    native_operation_type = type(operation)
                self.assertIs(operation.__await__(), operation)
                self.assertFalse(generator.ag_running)
                self.assertIs(operation.send(None), token)
                self.assertEqual(observations, [("AGEN_RUNNING", True, False)])
                self.assertEqual(inspect.getasyncgenstate(generator), "AGEN_SUSPENDED")
                self.assertTrue(generator.ag_running)
                self.assertTrue(generator.ag_suspended)
                self.assertIsNotNone(generator.ag_await)
                for second in (
                    generator.__anext__(),
                    generator.asend(None),
                    generator.athrow(ValueError),
                    generator.aclose(),
                ):
                    with self.assertRaisesRegex(RuntimeError, "already running"):
                        second.send(None)
                    with self.assertRaisesRegex(RuntimeError, "cannot reuse"):
                        second.send(None)
                with self.assertRaises(StopIteration) as yielded:
                    operation.send(23)
                self.assertEqual(yielded.exception.value, 29)
                self.assertFalse(generator.ag_running)
                self.assertEqual(generator.ag_state, "AGEN_SUSPENDED")
                self.assertIsNone(generator.ag_await)
                with self.assertRaisesRegex(RuntimeError, "cannot reuse"):
                    operation.send(None)
                with self.assertRaises(StopAsyncIteration):
                    generator.__anext__().send(None)
                self.assertEqual(observations, [("AGEN_RUNNING", True, False)] * 3)
                self.assertEqual(inspect.getasyncgenstate(generator), "AGEN_CLOSED")
                self.assertFalse(generator.ag_running)
                if not managed:
                    self.assertIsNone(generator.ag_frame)
                if managed:
                    self.assertEqual(owner["clears"], 1)
                    self.assert_no_native_exception_item(generator)

    def test_managed_result_disposition_is_validated_for_the_exact_native_family(self):
        for function, step in (
            (source_body, (NEXT, SUSPENDED, NO_SUSPEND, 1)),
            (source_body, (NEXT, SUSPENDED, ASYNC_YIELD, self.probe.wrap_async(1))),
            (source_coroutine, (NEXT, SUSPENDED, DIRECT, 1)),
            (source_async_generator, (NEXT, SUSPENDED, DIRECT, 1)),
            (source_async_generator, (NEXT, SUSPENDED, ASYNC_YIELD, 1)),
            (source_async_generator, (RETURN, CLOSED, NO_SUSPEND, 1)),
            (source_coroutine, (RETURN, CLOSED, DELEGATING, None)),
            (source_body, (ERROR, CLOSED, DIRECT, ValueError("bad disposition"))),
        ):
            with self.subTest(function=function, step=step):
                generator, owner = self.new(owner_for(step), function)
                with self.assertRaisesRegex(
                    SystemError, "managed generator callback result"
                ):
                    self.probe.send(generator, None)
                self.assertEqual(owner["clears"], 1)
                self.assert_no_native_exception_item(generator)
                with self.assertRaisesRegex(RuntimeError, "terminal"):
                    self.probe.matches(generator, owner)

    def test_managed_constructor_family_mismatch_does_not_bind_or_publish(self):
        for constructor, function in (
            (self.probe.new_coroutine, source_body),
            (self.probe.new_coroutine, source_async_generator),
            (self.probe.new_asyncgen, source_body),
            (self.probe.new_asyncgen, source_coroutine),
        ):
            with self.subTest(constructor=constructor, function=function):
                owner = owner_for()
                with self.assertRaisesRegex(TypeError, "native family"):
                    constructor(function, owner)
                self.assertFalse(owner["bound"])
                self.assertEqual(owner["clears"], 0)

    def test_async_yield_token_owns_its_value_and_allocation_error_stays_in_callback(
        self,
    ):
        class Payload:
            pass

        value = Payload()
        witness = weakref.ref(value)
        with self.assertRaises(MemoryError):
            self.probe.wrap_async_oom(value)
        token = self.probe.wrap_async(value)
        del value
        self.assertIsNotNone(witness())
        del token
        self.assertIsNone(witness())

        owner = owner_for(
            (NEXT, SUSPENDED, ASYNC_YIELD, 11),
            (RETURN, CLOSED, None),
            wrap_async_yield=True,
            fail_wrap=True,
            recover_wrap=True,
            wrap_recovery_value=29,
        )
        generator, owner = self.new(owner, source_async_generator)
        with self.assertRaises(StopIteration) as yielded:
            generator.__anext__().send(None)
        self.assertEqual(yielded.exception.value, 29)
        self.assertIsInstance(owner["caught_wrap_error"], MemoryError)
        self.assertEqual(owner["clears"], 0)
        self.assertEqual(generator.ag_state, "AGEN_SUSPENDED")
        self.assertEqual(self.probe.matches(generator, owner), 1)
        with self.assertRaises(StopAsyncIteration):
            generator.__anext__().send(None)
        self.assertEqual(owner["clears"], 1)

    def test_managed_async_throw_keeps_raw_operands_and_native_normalization_timing(
        self,
    ):
        constructions = []

        class Injected(Exception):
            def __init__(self, value):
                constructions.append((value, sys.exception()))
                super().__init__(value)

        caller = KeyError("caller")
        for managed in (False, True):
            with (
                self.subTest(managed=managed),
                warnings.catch_warnings(record=True) as seen,
            ):
                warnings.simplefilter("always", DeprecationWarning)
                constructions.clear()
                owner = owner_for(normalize_throw=True)
                generator = (
                    self.new(owner, source_async_generator)[0]
                    if managed
                    else source_async_generator()
                )
                operation = generator.athrow(Injected, "raw", None)
                self.assertEqual(constructions, [])
                self.assertEqual(len(seen), 1)
                self.assertIs(seen[0].category, DeprecationWarning)
                try:
                    raise caller
                except KeyError:
                    with self.assertRaises(Injected) as raised:
                        operation.send(None)
                    self.assertIs(sys.exception(), caller)
                self.assertEqual(raised.exception.args, ("raw",))
                self.assertEqual(constructions, [("raw", caller)])
                self.assertEqual(generator.ag_state, "AGEN_CLOSED")
                self.assertFalse(generator.ag_running)
                if managed:
                    self.assertEqual(
                        owner["calls"][0],
                        (THROW, 0, Injected, "raw", None, False, False, False, caller),
                    )
                    self.assertEqual(owner["clears"], 1)

    def test_managed_async_invalid_throw_preserves_created_state_and_aclose_raw_mode(
        self,
    ):
        for managed in (False, True):
            with self.subTest(managed=managed):
                owner = owner_for(normalize_throw=True)
                generator = (
                    self.new(owner, source_async_generator)[0]
                    if managed
                    else source_async_generator()
                )
                operation = generator.athrow(42)
                with self.assertRaisesRegex(TypeError, "exceptions must be classes"):
                    operation.send(None)
                self.assertEqual(generator.ag_state, "AGEN_CREATED")
                self.assertFalse(generator.ag_running)
                with self.assertRaisesRegex(RuntimeError, "cannot reuse"):
                    operation.send(None)
                with self.assertRaises(StopIteration):
                    generator.aclose().send(None)
                self.assertEqual(generator.ag_state, "AGEN_CLOSED")
                if managed:
                    self.assertEqual(owner["calls"][0][0:3], (THROW, 0, 42))
                    self.assertEqual(owner["calls"][1][0:3], (THROW, 0, GeneratorExit))
                    self.assertEqual(owner["clears"], 1)

    def test_managed_async_firstiter_failure_is_once_and_finalizer_capture_is_native(
        self,
    ):
        previous = sys.get_asyncgen_hooks()
        try:
            for managed in (False, True):
                with self.subTest(managed=managed):
                    events = []
                    failure = LookupError("firstiter failed")

                    def firstiter(generator, events=events, failure=failure):
                        events.append(("first", type(generator), generator.ag_state))
                        raise failure

                    sys.set_asyncgen_hooks(firstiter=firstiter, finalizer=None)
                    owner = owner_for()
                    generator = (
                        self.new(owner, source_async_generator)[0]
                        if managed
                        else source_async_generator()
                    )
                    with self.assertRaises(LookupError) as raised:
                        generator.__anext__()
                    self.assertIs(raised.exception, failure)
                    self.assertEqual(
                        events, [("first", types.AsyncGeneratorType, "AGEN_CREATED")]
                    )
                    with self.assertRaises(StopIteration):
                        generator.aclose().send(None)
                    self.assertEqual(len(events), 1)

            for managed in (False, True):
                with self.subTest(finalizer_managed=managed):
                    events = []
                    resurrected = []

                    async def ordinary():
                        yield 11

                    def firstiter(generator, events=events):
                        events.append(("first", type(generator)))

                    def finalizer(generator, events=events, resurrected=resurrected):
                        events.append(("final", type(generator), generator.ag_state))
                        resurrected.append(generator)

                    sys.set_asyncgen_hooks(firstiter=firstiter, finalizer=finalizer)
                    owner = owner_for(
                        (NEXT, SUSPENDED, ASYNC_YIELD, 11),
                        (RETURN, CLOSED, None),
                        wrap_async_yield=True,
                    )
                    generator = (
                        self.new(owner, source_async_generator)[0]
                        if managed
                        else ordinary()
                    )
                    operation = generator.__anext__()
                    sys.set_asyncgen_hooks(firstiter=None, finalizer=None)
                    with self.assertRaises(StopIteration) as yielded:
                        operation.send(None)
                    self.assertEqual(yielded.exception.value, 11)
                    del operation, generator
                    gc.collect()
                    self.assertEqual(
                        events,
                        [
                            ("first", types.AsyncGeneratorType),
                            ("final", types.AsyncGeneratorType, "AGEN_SUSPENDED"),
                        ],
                    )
                    self.assertEqual(len(resurrected), 1)
                    generator = resurrected.pop()
                    if managed:
                        self.assertEqual(self.probe.matches(generator, owner), 1)
                        self.assertEqual(owner["clears"], 0)
                    with self.assertRaises(StopIteration):
                        generator.aclose().send(None)
                    if managed:
                        self.assertEqual(owner["clears"], 1)
                    del generator
                    gc.collect()
                    self.assertEqual(len(events), 2)
        finally:
            sys.set_asyncgen_hooks(
                firstiter=previous.firstiter, finalizer=previous.finalizer
            )

    def test_managed_native_families_publish_terminal_state_before_local_finalizers(
        self,
    ):
        def outcome(call):
            try:
                value = call()
            except BaseException as error:  # noqa: BLE001 - Record exact native terminal protocol outcomes.
                return type(error), str(error)
            else:
                return "return", value

        class Pause:
            def __await__(self):
                yield "ready"

        class Payload:
            def __init__(
                self, generator, prefix, observations, caller, owner_view=None
            ):
                self.generator = weakref.ref(generator)
                self.prefix = prefix
                self.observations = observations
                self.caller = caller
                self.owner_view = (
                    weakref.ref(owner_view) if owner_view is not None else None
                )

            def __del__(self):
                generator = self.generator()
                if generator is None:
                    self.observations.append(
                        {"error": "generator disappeared before local"}
                    )
                    return
                prefix = self.prefix
                injected = LookupError("terminal reentrant throw")
                if prefix == "cr":
                    operations = (
                        lambda: generator.send(None),
                        lambda: generator.throw(injected),
                        generator.close,
                    )
                else:
                    operations = (
                        lambda: generator.__anext__().send(None),
                        lambda: generator.athrow(injected).send(None),
                        lambda: generator.aclose().send(None),
                    )
                row = {
                    "state": getattr(generator, prefix + "_state"),
                    "running": getattr(generator, prefix + "_running"),
                    "suspended": getattr(generator, prefix + "_suspended"),
                    "delegate_none": getattr(generator, prefix + "_await") is None,
                    "caller_handled": sys.exception() is self.caller,
                    "operations": [outcome(call) for call in operations],
                }
                if self.owner_view is None:
                    # Keep the original ordinary CPython frame control only.
                    row["frame_none"] = getattr(generator, prefix + "_frame") is None
                view = self.owner_view() if self.owner_view is not None else None
                if view is not None:
                    row["owner_matches"] = outcome(
                        lambda: view.probe.matches(generator, view.owner)
                    )[0]
                self.observations.append(row)

        class OwnerView:
            def __init__(self, owner, probe):
                self.owner = owner
                self.probe = probe

        async def ordinary_coroutine(active, details, mode):
            _payload = Payload(active[0], **details)
            await Pause()
            if mode == "raise":
                raise ValueError("body failed")

        async def ordinary_async_generator(active, details, mode):
            _payload = Payload(active[0], **details)
            yield "ready"
            if mode == "raise":
                raise ValueError("body failed")

        for prefix in ("cr", "ag"):
            for mode in ("return", "raise", "close"):
                ordinary_terminal_protocol = None
                caller = KeyError("terminal caller")
                for managed in (False, True):
                    with self.subTest(prefix=prefix, mode=mode, managed=managed):
                        observations = []
                        details = {
                            "prefix": prefix,
                            "observations": observations,
                            "caller": caller,
                        }
                        active = []
                        if managed:
                            terminal = (
                                (RETURN, CLOSED, None)
                                if mode == "return"
                                else (
                                    ERROR,
                                    CLOSED,
                                    ValueError("body failed")
                                    if mode == "raise"
                                    else GeneratorExit(),
                                )
                            )
                            owner = owner_for(
                                (
                                    NEXT,
                                    SUSPENDED,
                                    DELEGATING if prefix == "cr" else ASYNC_YIELD,
                                    "ready",
                                ),
                                terminal,
                                wrap_async_yield=prefix == "ag",
                                delegate=iter(("ready",)),
                                consume_step=True,
                            )
                            del terminal
                            function = (
                                ordinary_coroutine
                                if prefix == "cr"
                                else ordinary_async_generator
                            )
                            generator, owner = self.new(owner, function)
                        else:
                            function = (
                                ordinary_coroutine
                                if prefix == "cr"
                                else ordinary_async_generator
                            )
                            generator = function(active, details, mode)
                        active.append(generator)
                        if prefix == "cr":
                            self.assertEqual(generator.send(None), "ready")
                        else:
                            with self.assertRaises(StopIteration) as yielded:
                                generator.__anext__().send(None)
                            self.assertEqual(yielded.exception.value, "ready")
                        if managed:
                            owner_view = OwnerView(owner, self.probe)
                            owner["callback_payload"] = Payload(
                                generator, **details, owner_view=owner_view
                            )
                            owner["release_callback_payload"] = True
                            terminal_call_count = len(owner["calls"]) + 1
                        try:
                            raise caller
                        except KeyError:
                            if prefix == "cr":
                                terminal_call = (
                                    generator.close
                                    if mode == "close"
                                    else lambda generator=generator: generator.send(
                                        None
                                    )
                                )
                            else:
                                operation = (
                                    generator.aclose()
                                    if mode == "close"
                                    else generator.__anext__()
                                )
                                terminal_call = lambda operation=operation: (
                                    operation.send(None)
                                )
                            terminal_result = outcome(terminal_call)
                            self.assertIs(sys.exception(), caller)
                            # Allow deferred implicit release to reach a
                            # quiescent point without changing the caller's
                            # handled exception or requiring a release phase.
                            gc.collect()
                            self.assertIs(sys.exception(), caller)
                        if mode == "raise":
                            self.assertEqual(
                                terminal_result, (ValueError, "body failed")
                            )
                        elif mode == "close" and prefix == "cr":
                            self.assertEqual(terminal_result, ("return", None))
                        else:
                            self.assertIs(
                                terminal_result[0],
                                StopAsyncIteration
                                if mode == "return" and prefix == "ag"
                                else StopIteration,
                            )
                        self.assertEqual(len(observations), 1)
                        row = observations[0]
                        self.assertEqual(
                            row["state"],
                            "CORO_CLOSED" if prefix == "cr" else "AGEN_CLOSED",
                        )
                        if not managed:
                            self.assertEqual(
                                row["running"], prefix == "ag" and mode == "return"
                            )
                            self.assertTrue(row["frame_none"])
                        self.assertFalse(row["suspended"])
                        self.assertTrue(row["delegate_none"])
                        self.assertTrue(row["caller_handled"])
                        if managed:
                            self.assertIs(row["owner_matches"], RuntimeError)
                            self.assertEqual(owner["clears"], 1)
                            # Reentry during implicit release may still see an
                            # active outer operation. It must finish safely,
                            # yield no value and never enter another body step.
                            self.assertEqual(len(row["operations"]), 3)
                            for disposition, value in row["operations"]:
                                if disposition == "return":
                                    self.assertIsNone(value)
                            self.assertEqual(len(owner["calls"]), terminal_call_count)

                        # Compare terminal protocol behavior only after the
                        # original operation and eventual cleanup have finished.
                        self.assertEqual(
                            getattr(generator, prefix + "_state"),
                            "CORO_CLOSED" if prefix == "cr" else "AGEN_CLOSED",
                        )
                        self.assertFalse(getattr(generator, prefix + "_running"))
                        self.assertFalse(getattr(generator, prefix + "_suspended"))
                        self.assertIsNone(getattr(generator, prefix + "_await"))
                        injected = LookupError("terminal quiescent throw")
                        if prefix == "cr":
                            terminal_protocol = [
                                outcome(lambda: generator.send(None)),
                                outcome(lambda: generator.throw(injected)),
                                outcome(generator.close),
                            ]
                        else:
                            terminal_protocol = [
                                outcome(lambda: generator.__anext__().send(None)),
                                outcome(lambda: generator.athrow(injected).send(None)),
                                outcome(lambda: generator.aclose().send(None)),
                            ]
                        if managed:
                            self.assertEqual(terminal_protocol, ordinary_terminal_protocol)
                            self.assertEqual(len(owner["calls"]), terminal_call_count)
                            self.assertEqual(owner["clears"], 1)
                            self.assert_no_native_exception_item(generator)
                            with self.assertRaisesRegex(RuntimeError, "terminal"):
                                self.probe.matches(generator, owner)
                        else:
                            ordinary_terminal_protocol = terminal_protocol

    def test_created_throw_retains_bound_arguments_without_entering_any_native_body(
        self,
    ):
        def generator_source(argument):
            raise AssertionError("created throw entered the generator body")
            yield

        async def coroutine_source(argument):
            raise AssertionError("created throw entered the coroutine body")

        async def async_generator_source(argument):
            raise AssertionError("created throw entered the async-generator body")
            yield

        class Payload:
            def __init__(self, events, prefix):
                self.events = events
                self.prefix = prefix
                self.generator = None

            def __del__(self):
                instance = self.generator()
                self.events.append(
                    (
                        getattr(instance, self.prefix + "_state"),
                        getattr(instance, self.prefix + "_running"),
                    )
                )

        for prefix, source in (
            ("gi", generator_source),
            ("cr", coroutine_source),
            ("ag", async_generator_source),
        ):
            for retain in (False, True):
                for managed in (False, True):
                    with self.subTest(prefix=prefix, retain=retain, managed=managed):
                        events = []
                        payload = Payload(events, prefix)
                        reference = weakref.ref(payload)
                        if managed:
                            owner = owner_for(normalize_throw=True, payload=payload)
                            instance, _owner = self.new(owner, source)
                        else:
                            instance = source(payload)
                        payload.generator = weakref.ref(instance)
                        del payload

                        def throw(value, instance=instance, prefix=prefix):
                            if prefix == "ag":
                                return instance.athrow(value).send(None)
                            return instance.throw(value)

                        with self.assertRaises(TypeError):
                            throw("invalid exception")
                        self.assertTrue(
                            getattr(instance, prefix + "_state").endswith("CREATED")
                        )
                        self.assertIsNotNone(reference())
                        retained = None
                        try:
                            throw(ValueError)
                        except ValueError as error:
                            if not managed:
                                self.assertIsNotNone(reference())
                            if retain:
                                retained = error
                        else:
                            self.fail("created throw did not raise ValueError")
                        self.assertTrue(
                            getattr(instance, prefix + "_state").endswith("CLOSED")
                        )
                        self.assertFalse(getattr(instance, prefix + "_running"))
                        if retain:
                            if not managed:
                                self.assertIsNotNone(reference())
                            retained.__traceback__ = None
                        self.assertIsNone(reference())
                        self.assertEqual(len(events), 1)
                        self.assertTrue(events[0][0].endswith("CLOSED"))
                        if not managed:
                            # Ordinary tracebacks delay the implicit release.
                            # Managed cleanup may run inside the active athrow;
                            # CLOSED and post-call not-running still hold above.
                            self.assertFalse(events[0][1])


class ContextAndMetadataNativeTests(_NativeProbeTestCase):
    @staticmethod
    def source(payload):
        raise AssertionError("context kernel must never execute source bytecode")

    def test_contextual_calls_preserve_binding_error_identity_and_explicit_globals(self):
        for object_call in (False, True):
            callbacks = []
            marker = LookupError("contextual body")
            value = object()

            def call(function, args=(), keywords=None, pending=None):
                return self.probe.context_call(
                    self.source, function, args, keywords, object_call, pending,
                )

            def callee(argument, *, label):
                callbacks.append((argument, label))
                return argument

            args = (value,) if object_call else (value, "keyword")
            keywords = {"label": "keyword"} if object_call else ("label",)
            self.assertIs(call(callee, args, keywords), value)
            self.assertEqual(callbacks, [(value, "keyword")])
            callbacks.clear()

            def fail():
                callbacks.append("body")
                raise marker

            with self.assertRaises(LookupError) as raised:
                call(fail)
            self.assertIs(raised.exception, marker)
            marker.__traceback__ = None
            self.assertEqual(callbacks, ["body"])
            callbacks.clear()
            with self.assertRaises(LookupError) as raised:
                call(fail, pending=marker)
            self.assertIs(raised.exception, marker)
            marker.__traceback__ = None
            self.assertEqual(callbacks, [])
            self.assertIs(call(globals), self.source.__globals__)
            with self.assertRaises(NotImplementedError):
                call(locals)

    def test_metadata_query_rejects_foreign_c_payload_and_preserves_pending_error(self):
        def function():
            raise AssertionError("metadata query must not execute a body")

        marker = LookupError("pending opaque metadata query")
        self.assertEqual(self.probe.metadata_query_checks(function, marker), (1, 1, 1))

    def test_public_managed_and_metadata_headers_compile_with_configured_cxx(self):
        source = Path(sysconfig.get_config_var("abs_srcdir")).resolve(strict=True)
        build = Path(sysconfig.get_config_var("abs_builddir")).resolve(strict=True)
        cpp = Path(self.temporary.name) / "managed-metadata-api.cpp"
        cpp.write_text(
            '#include <Python.h>\n'
            '#include <frameobject.h>\n'
            '#include <cstddef>\n'
            '#include <type_traits>\n'
            'using ManagedNew = PyObject *(*)(PyObject *, PyCodeObject *, PyObject *, PyObject *, PyObject *, const PySoacGeneratorSpec *);\n'
            'static_assert(std::is_same<decltype(&PyGen_NewSoacManaged), ManagedNew>::value);\n'
            'static_assert(std::is_same<decltype(&PyCoro_NewSoacManaged), ManagedNew>::value);\n'
            'static_assert(std::is_same<decltype(&PyAsyncGen_NewSoacManaged), ManagedNew>::value);\n'
            'static_assert(sizeof(PySoacGeneratorResult) == 24);\n'
            'static_assert(sizeof(PySoacGeneratorSpec) == 40);\n'
            'using StrictOwner = int (*)(PyObject *, PyObject *);\n'
            'using OwnedMetadataQuery = void *(*)(PyObject *, void (*)(void *));\n'
            'static_assert(std::is_same<decltype(&PyFunction_SetSoacStrictOwner), StrictOwner>::value);\n'
            'static_assert(std::is_same<decltype(&PyFunction_GetSoacMetadataForDestructorV1), OwnedMetadataQuery>::value);\n'
            'using ContextVectorcall = PyObject *(*)(PyObject *, PyObject *const *, size_t, PyObject *, PyObject *, PyObject *, PyObject *);\n'
            'using ContextObjectCall = PyObject *(*)(PyObject *, PyObject *, PyObject *, PyObject *, PyObject *, PyObject *);\n'
            'static_assert(std::is_same<decltype(&PySoac_VectorcallWithContext), ContextVectorcall>::value);\n'
            'static_assert(std::is_same<decltype(&PySoac_ObjectCallWithContext), ContextObjectCall>::value);\n'
        )
        command = [
            *shlex.split(sysconfig.get_config_var("CXX")),
            *_native_probe_cppflags(),
            "-fsyntax-only",
            f"-I{source / 'Include'}",
            f"-I{build}",
            str(cpp),
        ]
        result = subprocess.run(
            command, text=True, capture_output=True, check=False, timeout=120
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        print(
            json.dumps(
                {
                    "managed_metadata_public_cxx": command,
                    "source_sha256": hashlib.sha256(cpp.read_bytes()).hexdigest(),
                }
            )
        )


class OrdinaryCallbackFrameNativeTests(_NativeProbeTestCase):
    def test_ordinary_trace_and_profile_deliver_real_function_events(self):
        def ordinary_other():
            return 19

        for install in (sys.settrace, sys.setprofile):
            observed = []

            def trace(frame, event, arg):
                if frame.f_code is ordinary_other.__code__:
                    observed.append(event)
                return trace

            install(trace)
            try:
                self.assertEqual(ordinary_other(), 19)
            finally:
                install(None)
            self.assertEqual(observed[0], "call")
            self.assertEqual(observed[-1], "return")

    def test_ordinary_local_monitoring_delivers_real_function_events(self):
        def ordinary_other():
            return 19

        observed = []
        tool = next(index for index in range(6) if sys.monitoring.get_tool(index) is None)
        line = sys.monitoring.events.LINE
        sys.monitoring.use_tool_id(tool, "ordinary-native-observer-control")
        try:
            sys.monitoring.register_callback(tool, line, lambda code, offset: observed.append(code))
            sys.monitoring.set_local_events(tool, ordinary_other.__code__, line)
            sys.monitoring.restart_events()
            self.assertEqual(ordinary_other(), 19)
            self.assertIn(ordinary_other.__code__, observed)
        finally:
            sys.monitoring.set_local_events(tool, ordinary_other.__code__, 0)
            sys.monitoring.register_callback(tool, line, None)
            sys.monitoring.free_tool_id(tool)

    @staticmethod
    def retaining_callback(error):
        def callback():
            try:
                raise error
            except StopIteration:
                pass

        return callback

    def ordinary_parent_retirement(self, labels):
        """Real native frames establish the exact last-traceback cleanup order."""
        events = []
        references = []
        error = StopIteration("ordinary transitive parent control")
        callback = self.retaining_callback(error)

        class Payload:
            def __init__(self, label):
                self.label = label

            def __del__(self):
                events.append(self.label)

        def ordinary_frame(depth):
            payload = Payload(labels[depth])
            references.append(weakref.ref(payload))
            if depth + 1 < len(labels):
                ordinary_frame(depth + 1)
            else:
                callback()

        ordinary_frame(0)
        self.assertTrue(all(reference() is not None for reference in references))
        self.assertEqual(events, [])
        error.__traceback__ = None
        self.assertTrue(all(reference() is None for reference in references))
        return events

    def test_ordinary_callback_traceback_retains_its_native_parent_control(self):
        events = []
        error = StopIteration("ordinary parent")
        callback = self.retaining_callback(error)

        class Payload:
            def __del__(self):
                events.append("native")

        def ordinary_parent():
            payload = Payload()
            callback()
            return weakref.ref(payload)

        reference = ordinary_parent()
        self.assertIsNotNone(reference())
        self.assertEqual(events, [])
        error.__traceback__ = None
        self.assertIsNone(reference())
        self.assertEqual(events, ["native"])

    def test_ordinary_native_frame_members_keep_their_descriptor_kind(self):
        self.assertIs(type(types.TracebackType.tb_lasti), types.MemberDescriptorType)
        self.assertIs(type(types.FrameType.f_trace_lines), types.MemberDescriptorType)

    def test_ordinary_c_unraisable_retains_the_native_source_parent_control(self):
        events = []
        observed = []
        error = ValueError("ordinary unraisable")

        class Payload:
            def __del__(self):
                events.append("ordinary-unraisable")

        def ordinary_parent():
            payload = Payload()
            self.probe.ordinary_unraisable(error)
            return weakref.ref(payload)

        previous = sys.unraisablehook
        sys.unraisablehook = lambda data: observed.append(data.exc_traceback)
        try:
            reference = ordinary_parent()
        finally:
            sys.unraisablehook = previous
        self.assertIsNotNone(reference())
        self.assertIs(error.__traceback__.tb_frame.f_code, ordinary_parent.__code__)
        error.__traceback__ = None
        self.assertIsNotNone(reference())
        observed.clear()
        self.assertIsNone(reference())
        self.assertEqual(events, ["ordinary-unraisable"])


    def test_ordinary_transitive_callback_parents_release_once(self):
        labels = ["outer", "inner"]
        self.assertEqual(sorted(self.ordinary_parent_retirement(labels)), sorted(labels))

    def test_ordinary_tracemalloc_keeps_real_records_and_pending_error(self):
        import tracemalloc

        tracemalloc.start(8)
        try:
            marker = LookupError("allocator error identity")
            self.assertIs(self.probe.ordinary_allocate_pending(marker), marker)
            allocation = bytes(4096)
            trace = tracemalloc.get_object_traceback(allocation)
            self.assertIsNotNone(trace)
            self.assertTrue(all(frame.lineno > 0 for frame in trace))
            tracemalloc.take_snapshot()
        finally:
            tracemalloc.stop()

    def test_ordinary_fatal_allocator_dump_keeps_real_records(self):
        import os

        child = (
            "import resource, tracemalloc, _testcapi\n"
            "resource.setrlimit(resource.RLIMIT_CORE, (0, 0))\n"
            "tracemalloc.start(8)\n"
            "_testcapi.pymem_buffer_overflow()\n"
        )
        result = subprocess.run(
            [sys.executable, "-c", child],
            env={**os.environ, "PYTHONMALLOC": "debug"},
            text=True, capture_output=True, check=False, timeout=30,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Fatal Python error", result.stderr)
        self.assertIn("Memory block allocated at", result.stderr)
        self.assertNotIn("line 4294967295", result.stderr)
        self.assertNotIn("<invalid frame>", result.stderr)










_ORDINARY_PEP523_BINDING_PROGRAM = r'''# Ordinary CPython only: no SOAC import, native-token adapter, or C probe.
import _testinternalcapi
import dis
import json
import resource
import sys
import sysconfig

resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
settings = json.loads(sys.argv[1])
kind = settings["kind"]
initially_hooked = settings["initially_hooked"]
hook_after = settings["hook_after"]
warm = settings["warm"]
failure = settings["failure"]
records = []
events = []
comparisons = 0
armed = False
marker = ValueError("binding comparison marker")
argument = object()

def body_child():
    events.append("body-child")

def target(value, *, trigger=2, later=1):
    assert value is argument
    events.append(("target", trigger, later))
    body_child()
    return trigger, later

class Keyword(str):
    __hash__ = str.__hash__

    def __eq__(self, other):
        global comparisons
        if armed:
            comparisons += 1
            # Binding must see this later default after the current comparison.
            target.__kwdefaults__["later"] = 7
            if hook_after:
                _testinternalcapi.set_eval_frame_record(records)
            else:
                _testinternalcapi.set_eval_frame_default()
            if failure == "comparison":
                raise marker
        return str.__eq__(self, other)

kwargs = {Keyword("trigger"): 2}
if kind == "call":
    # CALL has no keyword-name tuple; a real kw-default lookup can still reenter.
    target.__kwdefaults__ = {Keyword("trigger"): 2, "later": 1}
    def invoke():
        return target(argument)
    expected_form = "CALL"
elif kind == "kw":
    target.__kwdefaults__ = {Keyword("trigger"): 2, "later": 1}
    def invoke():
        return target(value=argument)
    expected_form = "CALL_KW"
elif kind == "ex":
    def invoke():
        return target(*(argument,), **kwargs)
    expected_form = "CALL_FUNCTION_EX"
else:
    raise AssertionError(kind)

calls = [
    item for item in dis.get_instructions(invoke, adaptive=False)
    if item.opname in ("CALL", "CALL_KW", "CALL_FUNCTION_EX")
]
assert len(calls) == 1 and calls[0].opname == expected_form, calls
call_offset = calls[0].offset
for _ in range(warm):
    assert invoke() == (2, 1)
warm_instruction = next(
    item for item in dis.get_instructions(invoke, adaptive=True)
    if item.offset == call_offset
)
# Record the actual specialized opcode rather than assume warmup selected it.
warm_opcode = warm_instruction.opname

if failure == "unknown":
    assert kind == "ex"
    kwargs = {Keyword("absent"): 2}
elif failure == "duplicate":
    assert kind == "ex"
    kwargs = {Keyword("value"): argument}
elif failure not in ("none", "comparison"):
    raise AssertionError(failure)

events.clear()
records.clear()
armed = True
try:
    if initially_hooked:
        _testinternalcapi.set_eval_frame_record(records)
    caught = None
    try:
        first = invoke()
    except BaseException as error:
        caught = error
    assert comparisons > 0, "the actual native argument/default binder must reenter"
    assert target.__kwdefaults__["later"] == 7
    current_count = records.count("target")
    current_child_count = records.count("body_child")
    if failure == "none":
        assert caught is None, repr(caught)
        assert first == (2, 7)
        # Default VM dispatch commits before binding. An initially hooked call
        # follows vectorcall, whose actual frame evaluator is chosen after bind.
        assert current_count == int(initially_hooked and hook_after), records
        assert current_child_count == int(hook_after), records
        assert events == [("target", 2, 7), "body-child"], events
    else:
        if failure == "comparison":
            assert caught is marker, repr(caught)
        else:
            assert isinstance(caught, TypeError), repr(caught)
        assert current_count == current_child_count == 0, records
        assert events == [], events
    # Supply every keyword explicitly: this next call must not reenter Keyword.
    before_comparisons = comparisons
    assert target(argument, trigger=2, later=9) == (2, 9)
    next_count = records.count("target") - current_count
    next_child_count = records.count("body_child") - current_child_count
    assert next_count == next_child_count == int(hook_after), records
    assert comparisons == before_comparisons
finally:
    _testinternalcapi.set_eval_frame_default()

assert "soac" not in sys.modules
print(json.dumps({
    **settings,
    "source_form": expected_form,
    "warm_opcode": warm_opcode,
    "comparisons": comparisons,
    "target_current_count": current_count,
    "target_next_delta": next_count,
    "body_current_count": current_child_count,
    "body_next_delta": next_child_count,
    "executable": sys._base_executable,
    "debug": sysconfig.get_config_var("Py_DEBUG"),
    "internalcapi": _testinternalcapi.__file__,
}), flush=True)
'''


class NativeOrdinaryEvalFrameDispatchTests(unittest.TestCase):
    """Default-evaluator commitment across ordinary native argument binding."""

    def run_case(self, kind, initially_hooked, hook_after, warm, failure="none"):
        settings = {
            "kind": kind,
            "initially_hooked": initially_hooked,
            "hook_after": hook_after,
            "warm": warm,
            "failure": failure,
        }
        result = subprocess.run(
            [
                sys._base_executable, "-I", "-S", "-B", "-c",
                _ORDINARY_PEP523_BINDING_PROGRAM, json.dumps(settings),
            ],
            text=True, capture_output=True, check=False, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        receipt = json.loads(result.stdout)
        self.assertEqual({key: receipt[key] for key in settings}, settings)
        print(json.dumps({"ordinary_pep523_binding": receipt}), flush=True)

    def test_default_choice_survives_binding_hook_installation(self):
        for kind in ("call", "kw", "ex"):
            for warm in (0, 64):
                with self.subTest(kind=kind, warm=warm):
                    self.run_case(kind, False, True, warm)

    def test_initial_hook_uses_postbinding_hook_for_current_and_next_call(self):
        for kind in ("call", "kw", "ex"):
            for hook_after in (False, True):
                for warm in (0, 64):
                    with self.subTest(kind=kind, hook_after=hook_after, warm=warm):
                        self.run_case(kind, True, hook_after, warm)

    def test_binding_comparison_error_preserves_new_hook_for_next_call(self):
        for kind in ("call", "kw", "ex"):
            for initially_hooked in (False, True):
                for warm in (0, 64):
                    with self.subTest(kind=kind, initially_hooked=initially_hooked, warm=warm):
                        self.run_case(kind, initially_hooked, not initially_hooked, warm, "comparison")

    def test_expanded_type_errors_preserve_new_hook_for_next_call(self):
        for failure in ("unknown", "duplicate"):
            for initially_hooked in (False, True):
                for warm in (0, 64):
                    with self.subTest(failure=failure, initially_hooked=initially_hooked, warm=warm):
                        self.run_case("ex", initially_hooked, not initially_hooked, warm, failure)





# Ignored native22 controls. Reuse the existing ABI-matched C probe family;
# no original strict body, counterfeit capability, or Python delegate executes.




_ORDINARY_FRAME_DIAGNOSTIC_PROGRAM = r'''# Ordinary native diagnostic control; no SOAC or probe module is imported.
# Ready interpreters accept -I -S -B -c; bootstrap requires this script filename.
import faulthandler
import json
import os
import sys
import tempfile
import time
import types
import weakref

try:
    import resource
except ImportError:
    pass
else:
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))

case = sys.argv[1]
if case not in ("watchdog", "c-thread", "released-gil"):
    raise ValueError(case)

def diagnostic_subject(mode):
    if mode == "watchdog":
        with tempfile.TemporaryFile() as stream:
            faulthandler.dump_traceback_later(0.05, file=stream)
            try:
                deadline = time.monotonic() + 5
                while os.fstat(stream.fileno()).st_size == 0:
                    if time.monotonic() >= deadline:
                        raise AssertionError("native watchdog produced no dump")
                    time.sleep(0.01)  # real ordinary frame; target releases GIL
            finally:
                faulthandler.cancel_dump_traceback_later()
            stream.seek(0)
            return stream.read().decode("ascii", "backslashreplace")
    if mode == "c-thread":
        faulthandler.enable()
        faulthandler._fatal_error_c_thread()
    import _testcapi
    _testcapi.fatal_error(b"native debug frame diagnostic", True)
    raise AssertionError("native fatal helper returned")

filename = "<ordinary-debug-frame-" + case + ">"
copied = types.FunctionType(
    diagnostic_subject.__code__.replace(co_filename=filename), globals()
)
code_reference = weakref.ref(copied.__code__)
assert not any(name == "soac" or name.startswith("soac.") or name.startswith("soac_native")
               for name in sys.modules)
print("native-diagnostic-enter:" + case, flush=True)
trace = copied(case)
assert case == "watchdog"
del copied
assert code_reference() is None, "diagnostic view retained a Python code owner"
assert "Timeout " in trace
assert 'File "' + filename + '", line ' in trace
assert " in diagnostic_subject\n" in trace
assert " in <module>\n" in trace
assert "PyInterpreterState_Get" not in trace
assert "<invalid frame>" not in trace
print(json.dumps({
    "case": case,
    "code_released": True,
    "complete_native_traceback": True,
    "trace": trace,
}, sort_keys=True))
'''


_ORDINARY_FRAME_COPY_PROGRAM = r'''
import json, sys, weakref

case = sys.argv[1]
events, references, frames = [], [], []

class Payload:
    def __init__(self):
        references.append(weakref.ref(self))

    def __del__(self):
        events.append("released")

bodies = {
    "return": "payload = Payload()\nframes.append(sys._getframe())\nreturn None\n",
    "traceback": "payload = Payload()\nraise ValueError('ordinary frame-copy control')\n",
    "generator": "payload = Payload()\nyield None\n",
}
namespace = {"Payload": Payload, "frames": frames, "sys": sys}
source = "def subject():\n" + "".join("    " + line for line in bodies[case].splitlines(True))
exec(compile(source, "ordinary-frame-copy-" + case + ".py", "exec", dont_inherit=True), namespace)
function = namespace["subject"]
code = function.__code__
assert not sys._is_immortal(code), "the control requires freshly compiled mortal code"
code_ref = weakref.ref(code)
print(json.dumps({"case": case, "phase": "before-native-frame-retirement"}), flush=True)
if case == "return":
    function()
    frame = frames.pop()
elif case == "traceback":
    try:
        function()
    except ValueError as error:
        assert error.__traceback__.tb_next is not None
        frame = error.__traceback__.tb_next.tb_frame
        error.__traceback__ = None
    else:
        raise AssertionError("ordinary exception control did not raise")
else:
    generator = function()
    assert next(generator) is None
    frame = generator.gi_frame
    generator.close()
    del generator
assert frame.f_code is code
assert len(references) == 1 and references[0]() is not None and events == []
del namespace["subject"], function, code
assert code_ref() is not None
frame.clear()
assert references[0]() is None and events == ["released"]
assert code_ref() is not None, "a cleared frame still owns its original code"
del frame
assert code_ref() is None, "no code owner may leak after the last frame is released"
print(json.dumps({"case": case, "released_once": events, "code_released": True}), flush=True)
'''


_ORDINARY_UNRAISABLE_CYCLE_PROGRAM = r'''# Ordinary native unraisable-hook GC ownership, with no SOAC or C probe.
import gc
import json
import sys
import weakref

events = []

class Payload:
    def __del__(self):
        events.append("released")

class Iterator:
    def __iter__(self):
        return self
    def __next__(self):
        return 1

def delegated(delegate):
    yield from delegate

def capture():
    payload = Payload()
    reference = weakref.ref(payload)
    error = LookupError("close lookup")
    captured = []

    class LookupFailure(Iterator):
        @property
        def close(self):
            raise error

    generator = delegated(LookupFailure())
    next(generator)
    previous = sys.unraisablehook
    sys.unraisablehook = captured.append
    try:
        assert generator.close() is None
    finally:
        sys.unraisablehook = previous
    assert len(captured) == 1
    assert captured[0].exc_value is error
    return captured, reference

captured, reference = capture()
hook_args_tracked = gc.is_tracked(captured[0])
gc.collect()
retained_alive = reference() is not None
assert retained_alive and events == []

# Drop the sole external capture owner. Do not clear args, traceback, frame,
# or the captured list: ordinary cyclic GC must release the real cycle.
del captured
gc.collect()
report = {
    "hook_args_tracked": hook_args_tracked,
    "retained_alive": retained_alive,
    "released_after_external_owner": reference() is None,
    "events": events,
    "soac_or_probe_loaded": any(
        name == "soac" or name.startswith("soac.") or name == "_strict_managed_generator"
        for name in sys.modules
    ),
}
print(json.dumps(report, sort_keys=True), flush=True)
assert report["released_after_external_owner"], report
assert events == ["released"], report
assert hook_args_tracked, report
'''


class OrdinaryFaultHandlerNativeTests(unittest.TestCase):
    """Actual ordinary no-GIL diagnostics, including StackRef-debug builds."""

    def run_diagnostic(self, case):
        result = subprocess.run(
            [sys._base_executable, "-I", "-S", "-B", "-c",
             _ORDINARY_FRAME_DIAGNOSTIC_PROGRAM, case],
            text=True, capture_output=True, check=False, timeout=15,
        )
        self.assertIn("native-diagnostic-enter:" + case, result.stdout)
        return result

    def test_watchdog_without_attached_tstate_keeps_real_code_and_caller_records(self):
        result = self.run_diagnostic("watchdog")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        actual = json.loads(result.stdout.splitlines()[-1])
        self.assertEqual(
            {key: actual[key] for key in ("case", "code_released", "complete_native_traceback")},
            {"case": "watchdog", "code_released": True, "complete_native_traceback": True},
        )
        self.assertIn('File "<ordinary-debug-frame-watchdog>", line ', actual["trace"])
        self.assertIn(" in diagnostic_subject\n", actual["trace"])
        self.assertIn(" in <module>\n", actual["trace"])

    def check_fatal_diagnostic(self, case, headline):
        result = self.run_diagnostic(case)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(headline, result.stderr)
        self.assertIn('File "<ordinary-debug-frame-' + case + '>", line ', result.stderr)
        self.assertIn(" in diagnostic_subject\n", result.stderr)
        self.assertIn(" in <module>\n", result.stderr)
        self.assertNotIn("PyInterpreterState_Get", result.stderr)
        self.assertNotIn("<invalid frame>", result.stderr)

    def test_fatal_error_from_unattached_c_thread_keeps_real_python_caller(self):
        self.check_fatal_diagnostic(
            "c-thread", "Fatal Python error: faulthandler_fatal_error_thread: in new thread"
        )

    def test_fatal_error_after_gil_release_keeps_real_python_caller(self):
        self.check_fatal_diagnostic(
            "released-gil",
            "Fatal Python error: _testcapi_fatal_error_impl: native debug frame diagnostic",
        )


class OrdinaryFrameCopyNativeTests(unittest.TestCase):
    """Ordinary frame-copy controls also run under real StackRef-debug builds."""

    def check_frame_copy(self, case):
        # No SOAC import, synthetic lifetime frame, C probe, or gc.collect().
        # The subprocess isolates a native handle abort from the test runner.
        result = subprocess.run(
            [sys._base_executable, "-I", "-S", "-B", "-c", _ORDINARY_FRAME_COPY_PROGRAM, case],
            text=True, capture_output=True, check=False, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(json.loads(result.stdout.splitlines()[-1]), {
            "case": case, "released_once": ["released"], "code_released": True,
        })

    def test_returned_frame_keeps_mortal_code_and_locals_until_clear(self):
        self.check_frame_copy("return")

    def test_traceback_frame_keeps_mortal_code_and_locals_until_clear(self):
        self.check_frame_copy("traceback")

    def test_closed_generator_frame_keeps_mortal_code_and_locals_until_clear(self):
        self.check_frame_copy("generator")

    def test_captured_unraisable_traceback_cycle_releases_after_external_owner(self):
        # The child keeps and then releases its sole external capture owner.
        # It does not clear any internal traceback/list/frame edge for the pass.
        result = subprocess.run(
            [sys._base_executable, "-I", "-S", "-B", "-c", _ORDINARY_UNRAISABLE_CYCLE_PROGRAM],
            text=True, capture_output=True, check=False, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(json.loads(result.stdout.splitlines()[-1]), {
            "hook_args_tracked": True,
            "retained_alive": True,
            "released_after_external_owner": True,
            "events": ["released"],
            "soac_or_probe_loaded": False,
        })







_ORDINARY_UNICODE_INPLACE_PROGRAM = r'''# Ordinary native Unicode in-place-add borrow-order regression.
# The retained _bootstrap_python runs this source as a script with -I -S, not -c.
# No SOAC import, C probe, unittest, site, or gc.collect is used.
import sys

case = sys.argv[1]
if case not in ("distinct", "self"):
    raise ValueError("expected distinct or self")

def append_distinct(path, suffix):
    path += suffix
    return path

def append_self(path):
    path += path
    return path

seed = "".join(("ordinary-native", "-mortal-seed"))
suffix = "".join(("/", "mortal-suffix"))
assert not sys._is_immortal(seed)
assert not sys._is_immortal(suffix)
alias = seed
original = seed[:]
expected = seed + (suffix if case == "distinct" else seed)
subject = append_distinct if case == "distinct" else append_self

print("unicode-inplace: warm-start", case, flush=True)
for iteration in range(512):
    if case == "distinct":
        result = subject(seed, suffix)
    else:
        result = subject(seed)
    assert result == expected
    assert seed == original and alias is seed
print("unicode-inplace: warm-complete", case, flush=True)

# Inspect the actual quickened native instruction objects only after the
# minimal reproducer. An old debug interpreter must reach warm-start before
# aborting, rather than crashing during unittest/site or dis imports.
import dis
import json

instructions = tuple(dis.get_instructions(subject, adaptive=True))
specialized = [
    instruction.offset for instruction in instructions
    if instruction.opname == "BINARY_OP_INPLACE_ADD_UNICODE"
]
borrowed_reads = [
    instruction.offset for instruction in instructions
    if instruction.opname in (
        "LOAD_FAST_BORROW",
        "LOAD_FAST_BORROW_LOAD_FAST_BORROW",
    )
]
assert len(specialized) == 1, instructions
assert borrowed_reads, instructions
print(json.dumps({
    "case": case,
    "rounds": 512,
    "result": result,
    "mortal_input": True,
    "input_alias_unchanged": True,
    "specialized_offsets": specialized,
    "borrowed_read_offsets": borrowed_reads,
    "opcodes": [instruction.opname for instruction in instructions],
}), flush=True)
'''


class OrdinaryUnicodeInplaceNativeTests(unittest.TestCase):
    """Ordinary specialized string-add controls, including StackRef-debug."""

    def check_unicode_inplace_borrow_order(self, case):
        result = subprocess.run(
            [
                sys._base_executable, "-I", "-S", "-B", "-c",
                _ORDINARY_UNICODE_INPLACE_PROGRAM, case,
            ],
            text=True, capture_output=True, check=False, timeout=30,
        )
        self.assertIn("unicode-inplace: warm-start " + case, result.stdout)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        report = json.loads(result.stdout.splitlines()[-1])
        self.assertEqual(report["case"], case)
        self.assertEqual(report["rounds"], 512)
        self.assertTrue(report["mortal_input"])
        self.assertTrue(report["input_alias_unchanged"])
        seed = "ordinary-native-mortal-seed"
        self.assertEqual(
            report["result"],
            seed + ("/mortal-suffix" if case == "distinct" else seed),
        )
        self.assertEqual(len(report["specialized_offsets"]), 1)
        self.assertTrue(report["borrowed_read_offsets"])
        self.assertIn("BINARY_OP_INPLACE_ADD_UNICODE", report["opcodes"])

    def test_specialized_unicode_append_retires_left_borrow_before_local_owner(self):
        self.check_unicode_inplace_borrow_order("distinct")

    def test_specialized_unicode_self_append_retires_both_operand_borrows(self):
        self.check_unicode_inplace_borrow_order("self")


if __name__ == "__main__":
    unittest.main()
