from __future__ import annotations

import textwrap

import pytest

from tests._strict_integration import (
    _VALIDATION_PRELUDE,
    StrictValidationCase,
    create_strict_project,
)

# Each source and behavioral assertion below was individually reviewed. The
# mutated objects remain ordinary; only the caller opts into strict execution.
# The old assertion that an ordinary function acquired a JIT id is obsolete.
_MUTATION_CASES = {
    "test_warmed_direct_call_observes_replaced_function_code": (
        textwrap.dedent("""
        def target():
            return 41


        def replacement():
            return 42


        def run():
            return target()
        """),
        textwrap.dedent("""
        def run():
            return ordinary.target()
        """),
        textwrap.dedent("""
        for _ in range(16):
            assert module.run() == 41
        assert _soac_function_id(ordinary.target) == 0
        ordinary.target.__code__ = ordinary.replacement.__code__
        assert module.run() == 42
        assert _soac_function_id(ordinary.target) == 0
        assert ordinary.target() == 42
        for _ in range(16):
            assert module.run() == 42
        """),
        ("target",),
    ),
    "test_warmed_direct_call_observes_replaced_positional_defaults": (
        textwrap.dedent("""
        def target(increment=1):
            return 40 + increment


        def run():
            return target()
        """),
        textwrap.dedent("""
        def run():
            return ordinary.target()
        """),
        textwrap.dedent("""
        for _ in range(16):
            assert module.run() == 41
        ordinary.target.__defaults__ = (2,)
        assert module.run() == 42
        assert ordinary.target() == 42
        for _ in range(16):
            assert module.run() == 42
        """),
        ("target",),
    ),
    "test_warmed_direct_call_observes_replaced_keyword_defaults": (
        textwrap.dedent("""
        def target(*, increment=1):
            return 40 + increment


        def run():
            return target()
        """),
        textwrap.dedent("""
        def run():
            return ordinary.target()
        """),
        textwrap.dedent("""
        for _ in range(16):
            assert module.run() == 41
        ordinary.target.__kwdefaults__ = {'increment': 2}
        assert module.run() == 42
        assert ordinary.target() == 42
        for _ in range(16):
            assert module.run() == 42
        """),
        ("target",),
    ),
    "test_warmed_direct_call_observes_in_place_keyword_default_mutation": (
        textwrap.dedent("""
        def target(*, increment=1):
            return 40 + increment


        def run():
            return target()
        """),
        textwrap.dedent("""
        def run():
            return ordinary.target()
        """),
        textwrap.dedent("""
        for _ in range(16):
            assert module.run() == 41
        kwdefaults = ordinary.target.__kwdefaults__
        assert kwdefaults is not None
        kwdefaults['increment'] = 2
        assert ordinary.target.__kwdefaults__ is kwdefaults
        assert module.run() == 42
        assert ordinary.target() == 42
        del kwdefaults['increment']
        with pytest.raises(TypeError):
            module.run()
        with pytest.raises(TypeError):
            ordinary.target()
        """),
        ("target",),
    ),
    "test_warmed_method_call_observes_replaced_defaults_and_code": (
        textwrap.dedent("""
        class Example:
            def target(self, increment=1):
                return 40 + increment

            def replacement(self, increment=1):
                return 50 + increment


        def run(instance):
            return instance.target()
        """),
        textwrap.dedent("""
        def run(instance):
            return instance.target()
        """),
        textwrap.dedent("""
        instance = ordinary.Example()
        for _ in range(16):
            assert module.run(instance) == 41
        ordinary.Example.target.__defaults__ = (2,)
        assert instance.target() == 42
        for _ in range(16):
            assert module.run(instance) == 42
        ordinary.Example.target.__code__ = ordinary.Example.replacement.__code__
        assert instance.target() == 52
        for _ in range(16):
            assert module.run(instance) == 52
        """),
        ("Example.target", "Example.replacement"),
    ),
    "test_named_generator_observes_replaced_defaults_and_code": (
        textwrap.dedent("""
        def target(increment=1):
            yield 40 + increment


        def replacement(increment=1):
            yield 50 + increment


        def run():
            return next(target())
        """),
        textwrap.dedent("""
        def run():
            return next(ordinary.target())
        """),
        textwrap.dedent("""
        for _ in range(16):
            assert module.run() == 41
        ordinary.target.__defaults__ = (2,)
        assert module.run() == 42
        assert next(ordinary.target()) == 42
        ordinary.target.__code__ = ordinary.replacement.__code__
        assert module.run() == 52
        assert next(ordinary.target()) == 52
        """),
        ("target", "replacement"),
    ),
    "test_interpreted_entry_observes_replaced_code_and_defaults": (
        textwrap.dedent("""
        def target(increment=1):
            return 40 + increment


        def replacement(increment=1):
            return 50 + increment


        def run():
            return target()
        """),
        textwrap.dedent("""
        def run():
            return ordinary.target()
        """),
        textwrap.dedent("""
        assert module.run() == 41
        ordinary.target.__defaults__ = (2,)
        assert module.run() == 42
        ordinary.target.__code__ = ordinary.replacement.__code__
        assert module.run() == 52
        assert ordinary.target() == 52
        """),
        ("target",),
    ),
    "test_profiled_call_observes_function_mutation_during_argument_evaluation_code": (
        textwrap.dedent("""
        def target(value, increment=1):
            return "original", value + increment


        def replacement(value, increment=1):
            return "replacement", value + increment


        def decoy(value, increment=1):
            return "reloaded", value + increment


        def run(argument):
            return target(argument())
        """),
        textwrap.dedent("""
        def run(argument):
            return ordinary.target(argument())
        """),
        textwrap.dedent("""
        def argument():
            return 40
        for _ in range(64):
            assert module.run(argument) == ("original", 41)
        captured = ordinary.target
        calls = []
        def mutate_argument():
            calls.append("argument")
            if 'code' == "code":
                captured.__code__ = ordinary.replacement.__code__
            else:
                captured.__defaults__ = (2,)
            ordinary.target = ordinary.decoy
            return 40
        expected = ("replacement", 41) if 'code' == "code" else ("original", 42)
        actual = module.run(mutate_argument)
        assert calls == ["argument"], calls
        assert actual == expected, ('code', actual, expected)
        """),
        ("target", "replacement", "decoy"),
    ),
    "test_profiled_call_observes_function_mutation_during_argument_evaluation_defaults": (
        textwrap.dedent("""
        def target(value, increment=1):
            return "original", value + increment


        def replacement(value, increment=1):
            return "replacement", value + increment


        def decoy(value, increment=1):
            return "reloaded", value + increment


        def run(argument):
            return target(argument())
        """),
        textwrap.dedent("""
        def run(argument):
            return ordinary.target(argument())
        """),
        textwrap.dedent("""
        def argument():
            return 40
        for _ in range(64):
            assert module.run(argument) == ("original", 41)
        captured = ordinary.target
        calls = []
        def mutate_argument():
            calls.append("argument")
            if 'defaults' == "code":
                captured.__code__ = ordinary.replacement.__code__
            else:
                captured.__defaults__ = (2,)
            ordinary.target = ordinary.decoy
            return 40
        expected = ("replacement", 41) if 'defaults' == "code" else ("original", 42)
        actual = module.run(mutate_argument)
        assert calls == ["argument"], calls
        assert actual == expected, ('defaults', actual, expected)
        """),
        ("target", "replacement", "decoy"),
    ),
}

_CASE_MODULES = {
    name: f"mutation_{index}" for index, name in enumerate(_MUTATION_CASES)
}
_INTEROP_CASES = tuple(name for name in _MUTATION_CASES if "profiled_call" not in name)
_PROFILE_CASES = tuple(name for name in _MUTATION_CASES if "profiled_call" in name)

_SEALED_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

def target(increment=1):
    return 40 + increment

def replacement(increment=1):
    return 50 + increment

def keyword(*, increment=1):
    return 40 + increment

def run():
    return target()

def run_keyword():
    return keyword()

class Example:
    def target(self, increment=1):
        return 40 + increment
    def replacement(self, increment=1):
        return 50 + increment

def generator(increment=1):
    yield 40 + increment

def replacement_generator(increment=1):
    yield 50 + increment

def run_generator():
    return next(generator())
"""

_PRESEAL_PROBE = """
events = []
retained = []

def new_body(increment=1):
    return 50 + increment

def exercise(target, run, keyword, run_keyword, outcome):
    retained.append(target)
    code, defaults, kwdefaults = target.__code__, target.__defaults__, keyword.__kwdefaults__
    for _ in range(16):
        assert run() == run_keyword() == 41
    target.__defaults__ = (2,)
    assert run() == target() == 42
    events.append(("defaults", run()))
    target.__defaults__ = defaults
    assert run() == 41
    keyword.__kwdefaults__ = {"increment": 2}
    assert run_keyword() == 42
    keyword.__kwdefaults__["increment"] = 3
    assert run_keyword() == 43
    del keyword.__kwdefaults__["increment"]
    try:
        run_keyword()
    except TypeError:
        pass
    else:
        raise AssertionError("deleted default did not affect the live binder")
    keyword.__kwdefaults__ = kwdefaults
    assert run_keyword() == 41 and keyword.__kwdefaults__ is kwdefaults
    target.__code__ = new_body.__code__
    assert run() == target() == 51
    target.__defaults__ = (2,)
    assert run() == 52
    events.append(("replacement", run()))
    if outcome != "keep_code":
        target.__code__ = code
        if outcome == "restore":
            target.__defaults__ = defaults
        expected = 41 if outcome == "restore" else 42
        assert run() == target() == expected
        assert target.__code__ is code
        if outcome == "restore":
            assert target.__defaults__ is defaults
    else:
        expected = 52
    events.append((outcome, run()))
    return expected
"""


@pytest.fixture(scope="module")
def mutation_project(tmp_path_factory):
    sources = {
        "sealed_mutations.py": _SEALED_SOURCE,
        "preseal_probe.py": _PRESEAL_PROBE,
    }
    modules = {"sealed_mutations": "sealed_mutations.py"}
    for name, (source, caller, _validator, _witnesses) in _MUTATION_CASES.items():
        module_name = _CASE_MODULES[name]
        sources[f"ordinary_{module_name}.py"] = source
        sources[f"stock_{module_name}.py"] = source
        sources[f"stock_caller_{module_name}.py"] = (
            f"import stock_{module_name} as ordinary\n" + caller
        )
        sources[f"{module_name}.py"] = (
            f"# soac: module(strict_assign=true, checked_attr=true)\nimport ordinary_{module_name} as ordinary\n"
            + caller
        )
        modules[module_name] = f"{module_name}.py"
    preseal_source = _SEALED_SOURCE.split("class Example:", 1)[0]
    for outcome in ("restore", "keep_defaults", "keep_code"):
        module_name = f"preseal_{outcome}"
        sources[f"{module_name}.py"] = preseal_source + (
            "\nfrom preseal_probe import exercise\n"
            f"observed = exercise(target, run, keyword, run_keyword, {outcome!r})\n"
        )
        modules[module_name] = f"{module_name}.py"
    return create_strict_project(
        tmp_path_factory.mktemp("strict-function-mutation-interop"),
        sources,
        modules=modules,
    )


def _mutation_validator(name: str, *, profile_train_only: bool = False) -> str:
    _source, _caller, assertions, witnesses = _MUTATION_CASES[name]
    if profile_train_only:
        assertions = """
def argument():
    return 40
for _ in range(64):
    assert module.run(argument) == ("original", 41)
"""
    setup = f"""
ordinary = module.ordinary
assert _soac_ext.strict_module_diagnostics(ordinary) is None
get_id = ctypes.pythonapi.PyFunction_GetSoacFunctionId
get_id.argtypes = [ctypes.py_object]
get_id.restype = ctypes.c_uint64
get_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
get_owner.argtypes = [ctypes.py_object]
get_owner.restype = ctypes.c_void_p
def _soac_function_id(function):
    return int(get_id(function))
def ordinary_witnesses():
    for path in {witnesses!r}:
        value = ordinary
        for component in path.split('.'):
            value = vars(value)[component]
        assert not get_owner(value), path
        assert _soac_function_id(value) == 0, path
ordinary_witnesses()
"""
    exercise = setup + textwrap.dedent(assertions) + "\nordinary_witnesses()\n"
    return (
        "def validate(module):\n"
        "    import ctypes\n    import pytest\n    from soac import _soac_ext\n"
        f"    import stock_caller_{_CASE_MODULES[name]} as stock\n"
        "    assert _soac_ext.strict_function_entry_kind(stock.run) is None\n"
        "    def exercise(module):\n"
        + textwrap.indent(exercise, "        ")
        + "\n    exercise(stock)\n    exercise(module)\n"
    )


@pytest.fixture(
    scope="module", params=[False, True], ids=["checked_native", "entry_interpreter"]
)
def mutation_results(mutation_project, request):
    return mutation_project.run_cases(
        {
            _CASE_MODULES[name]: StrictValidationCase(
                _mutation_validator(name),
                mutation_project.project / f"{_CASE_MODULES[name]}.py",
                ("run",),
            )
            for name in _INTEROP_CASES
        },
        entry_interpreter=request.param,
    )


@pytest.mark.parametrize("case_name", _INTEROP_CASES)
def test_original_mutation_behaviors_remain_ordinary_interop(
    mutation_results, case_name
):
    # The two-mode batch reports every original case independently and guards
    # shared process state; these source modules have no cross-case globals.
    assert mutation_results[_CASE_MODULES[case_name]] is None, mutation_results[
        _CASE_MODULES[case_name]
    ]


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("case_name", _PROFILE_CASES)
def test_profiled_call_observes_function_mutation_during_argument_evaluation(
    mutation_project, case_name, entry_interpreter
):
    project = mutation_project
    module_name = _CASE_MODULES[case_name]
    work_dir = project.root / f"profile-{module_name}-{int(entry_interpreter)}"
    for mode in ("profile", "apply"):
        # Reuse the exact admission/entry assertions from run_case while
        # selecting the original profile/apply protocol and shared profile.
        program = _VALIDATION_PRELUDE + project._validation_program(
            module_name,
            StrictValidationCase(
                _mutation_validator(case_name, profile_train_only=mode == "profile"),
                project.project / f"{module_name}.py",
                ("run",),
            ),
            entry_interpreter=entry_interpreter,
        )
        project.run(
            program,
            entry_interpreter=entry_interpreter,
            opt_mode=mode,
            extra_env={"SOAC_WORK_DIR": str(work_dir)},
        )
        if mode == "profile":
            assert (work_dir / "profile.bin").is_file()


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_sealed_source_functions_reject_code_default_and_keyword_dictionary_mutations(
    mutation_project, entry_interpreter
):
    mutation_project.run_case(
        "sealed_mutations",
        textwrap.dedent("""
        def validate(module):
            import ctypes
            from soac.strict import StrictMutationError
            strict_id = ctypes.pythonapi.PyFunction_GetSoacStrictId
            strict_id.argtypes = [ctypes.py_object]
            strict_id.restype = ctypes.c_uint64
            instance = module.Example()
            cases = [
                (module.target, lambda: setattr(module.target, '__code__', module.replacement.__code__), module.run),
                (module.target, lambda: setattr(module.target, '__defaults__', (2,)), module.run),
                (module.keyword, lambda: setattr(module.keyword, '__kwdefaults__', {'increment': 2}), module.run_keyword),
                (module.keyword, lambda: module.keyword.__kwdefaults__.__setitem__('increment', 2), module.run_keyword),
                (module.keyword, lambda: module.keyword.__kwdefaults__.__delitem__('increment'), module.run_keyword),
                (module.Example.target, lambda: setattr(module.Example.target, '__defaults__', (2,)), instance.target),
                (module.Example.target, lambda: setattr(module.Example.target, '__code__', module.Example.replacement.__code__), instance.target),
                (module.generator, lambda: setattr(module.generator, '__defaults__', (2,)), module.run_generator),
                (module.generator, lambda: setattr(module.generator, '__code__', module.replacement_generator.__code__), module.run_generator),
            ]
            for function, mutate, call in cases:
                before = strict_id(function)
                assert before
                for _ in range(16):
                    assert call() == 41
                try:
                    mutate()
                except StrictMutationError:
                    pass
                else:
                    raise AssertionError('sealed function accepted a semantic mutation')
                assert strict_id(function) == before
                for _ in range(16):
                    assert call() == 41
        """),
        mutation_project.project / "sealed_mutations.py",
        entry_interpreter=entry_interpreter,
        required_functions=(
            "target",
            "keyword",
            "run",
            "run_keyword",
            "Example.target",
            "run_generator",
        ),
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("outcome, expected", [("restore", 41), ("keep_defaults", 42)])
def test_preseal_mutation_executes_changed_code_and_defaults_before_freezing(
    mutation_project, entry_interpreter, outcome, expected
):
    module_name = f"preseal_{outcome}"
    mutation_project.run_case(
        module_name,
        textwrap.dedent(f"""
        def validate(module):
            import preseal_probe
            from soac.strict import StrictMutationError
            assert preseal_probe.events == [('defaults', 42), ('replacement', 52), ({outcome!r}, {expected})]
            assert module.observed == module.run() == module.target() == {expected}
            assert module.run_keyword() == 41
            try:
                module.target.__defaults__ = (99,)
            except StrictMutationError:
                pass
            else:
                raise AssertionError('adopted initialization default remained mutable')
            assert module.run() == {expected}
        """),
        mutation_project.project / f"{module_name}.py",
        entry_interpreter=entry_interpreter,
        required_functions=("target", "keyword", "run", "run_keyword"),
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_kept_preseal_code_replacement_executes_then_fails_source_sealing(
    mutation_project, entry_interpreter
):
    mutation_project.run(
        """
import preseal_probe
from soac.strict import StrictRuntimeUnavailableError
try:
    import preseal_keep_code
except StrictRuntimeUnavailableError as error:
    assert 'strict function native metadata changed' in str(error)
else:
    raise AssertionError('replacement code acquired the original source contract')
assert preseal_probe.events == [('defaults', 42), ('replacement', 52), ('keep_code', 52)]
assert 'preseal_keep_code' not in sys.modules
assert len(preseal_probe.retained) == 1
try:
    preseal_probe.retained[0]()
except StrictRuntimeUnavailableError:
    pass
else:
    raise AssertionError('failed module execution still authorized a retained callable')
""",
        entry_interpreter=entry_interpreter,
    )
