# This module is evaluated through normal Python, not the SOAC import
# transform. Keep it limited to constants and helpers needed while
# `soac.runtime` itself is still initializing.

import builtins as _builtins
import keyword as _keyword

NO_DEFAULT = object()
ELLIPSIS = Ellipsis
TRUE = True
FALSE = False
NONE = None
EMPTY_TUPLE = ()
ANNOTATION_FORWARDREF_MISSING = object()

# This module executes on native CPython. Keep the literal here so runtime
# initialization does not recursively lower a t-string through its own helpers
# or compile source against an unrelated native frame.
_template_probe = t"{0}"
TEMPLATE_TYPE = type(_template_probe)
INTERPOLATION_TYPE = type(_template_probe.interpolations[0])
del _template_probe

_DP_CODE_WITH_FREEVARS_CACHE = {}
_CLIF_ENTRY_RUNTIME_ERROR = "CLIF entry executed without vectorcall interception"


def code_with_freevars(names, is_async, is_generator):
    names = tuple(names)
    is_async = bool(is_async)
    is_generator = bool(is_generator)
    cache_key = (names, is_async, is_generator)
    cached = _DP_CODE_WITH_FREEVARS_CACHE.get(cache_key)
    if cached is not None:
        return cached
    for name in names:
        if not isinstance(name, str):
            raise TypeError(f"freevar names must be str, got {type(name)!r}")
        # Native generic class helpers capture this compiler-owned cell. It is
        # code metadata, not a Python source identifier or an execution permit.
        if name != ".type_params" and (
            not name.isidentifier() or _keyword.iskeyword(name)
        ):
            raise ValueError(f"invalid freevar name: {name!r}")
    if len(set(names)) != len(names):
        raise ValueError("freevar names must be unique")

    # Never interpolate captured metadata into source. Compile an inert helper
    # with valid placeholders, then project the exact requested closure layout
    # onto its code object below. Native source-code admission is independent.
    source_names = tuple(f"__dp_freevar_{index}" for index in range(len(names)))
    outer_lines = ["def __dp_make_code():"]
    for name in source_names:
        outer_lines.append(f"    {name} = None")
    if is_async:
        outer_lines.append("    async def wrapped(*args, **kwargs):")
    else:
        outer_lines.append("    def wrapped(*args, **kwargs):")
    if names:
        outer_lines.append("        if False:")
        for name in source_names:
            outer_lines.append(f"            {name}")
    if is_async and is_generator:
        outer_lines.append("        if False:")
        outer_lines.append("            yield None")
        outer_lines.append(
            f"        raise RuntimeError({_CLIF_ENTRY_RUNTIME_ERROR!r})"
        )
    elif is_async:
        outer_lines.append(
            f"        raise RuntimeError({_CLIF_ENTRY_RUNTIME_ERROR!r})"
        )
    elif is_generator:
        outer_lines.append("        if False:")
        outer_lines.append("            yield None")
        outer_lines.append(
            f"        raise RuntimeError({_CLIF_ENTRY_RUNTIME_ERROR!r})"
        )
    else:
        outer_lines.append(
            f"        raise RuntimeError({_CLIF_ENTRY_RUNTIME_ERROR!r})"
        )
    outer_lines.append("    return wrapped.__code__")

    ns = {}
    _builtins.exec("\n".join(outer_lines), {}, ns)
    code = ns["__dp_make_code"]()
    if code.co_freevars != names:
        code = code.replace(co_freevars=names)
    _DP_CODE_WITH_FREEVARS_CACHE[cache_key] = code
    return code


def _entry_template(*args, **kwargs):
    raise RuntimeError(_CLIF_ENTRY_RUNTIME_ERROR)
