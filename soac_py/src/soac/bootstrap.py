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
        if not name.isidentifier() or _keyword.iskeyword(name):
            raise ValueError(f"invalid freevar name: {name!r}")
    if len(set(names)) != len(names):
        raise ValueError("freevar names must be unique")

    outer_lines = ["def __dp_make_code():"]
    for name in names:
        outer_lines.append(f"    {name} = None")
    if is_async:
        outer_lines.append("    async def wrapped(*args, **kwargs):")
    else:
        outer_lines.append("    def wrapped(*args, **kwargs):")
    if names:
        outer_lines.append("        if False:")
        for name in names:
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
