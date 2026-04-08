# diet-python: disabled
DELETED = object()
NO_DEFAULT = object()
ELLIPSIS = Ellipsis
TRUE = True
FALSE = False
NONE = None
EMPTY_TUPLE = ()
_SOAC_RUNTIME_READY = False

from asyncio import coroutines as _coroutines
import collections.abc as _abc
import keyword as _keyword
import reprlib as _reprlib
import sys as _sys
import builtins as _builtins
import threading as _threading
import types as _types
import typing as _typing
import warnings as _warnings
import string.templatelib as _templatelib

from . import _soac_ext
from .sim import (
    _MISSING,
    _mro_getattr,
    add,
    aiter,
    and_,
    eq,
    floordiv,
    ge,
    globals,
    gt,
    iadd,
    iand,
    ifloordiv,
    ilshift,
    imatmul,
    imod,
    imul,
    invert,
    ior,
    ipow,
    irshift,
    isub,
    itruediv,
    ixor,
    le,
    lshift,
    lt,
    matmul,
    mod,
    mul,
    ne,
    neg,
    not_,
    or_,
    pos,
    pow,
    rshift,
    sub,
    truth,
    truediv,
    xor,
)

_jit_make_bb_function = _soac_ext.make_bb_function

next = _builtins.next
iter = _builtins.iter
anext = _builtins.anext
isinstance = _builtins.isinstance
getattr = _builtins.getattr
setattr = _builtins.setattr
delattr = _builtins.delattr
tuple = _builtins.tuple
list = _builtins.list
dict = _builtins.dict
set = _builtins.set
slice = _builtins.slice
classmethod = _builtins.classmethod
ascii = _builtins.ascii
repr = _builtins.repr
str = _builtins.str
format = _builtins.format
AssertionError = _builtins.AssertionError


def tuple_values(*values):
    # Strict variadic tuple construction for transformed code.
    return _builtins.tuple(values)


def tuple_from_iter(value):
    return _builtins.tuple(value)


def __deepcopy__(memo):
    # Modules are not pickleable; keep runtime as a singleton during deepcopy().
    return _sys.modules[__name__]


typing_Generic = _typing.Generic
typing_TypeVar = _typing.TypeVar
typing_TypeVarTuple = _typing.TypeVarTuple
typing_ParamSpec = _typing.ParamSpec
typing_TypeAliasType = _typing.TypeAliasType
typing_Unpack = _typing.Unpack
templatelib_Template = _templatelib.Template
templatelib_Interpolation = _templatelib.Interpolation


def load_deleted_name(name, value):
    if value is DELETED:
        raise UnboundLocalError(
            f"cannot access local variable {name!r} where it is not associated with a value"
        )
    return value


def bb_trace_enter(function_qualname, block_label, params=None):
    if params:
        pieces = []
        for name, value in params:
            try:
                rendered = _reprlib.repr(value)
            except Exception as err:
                rendered = f"<repr failed: {type(err).__name__}>"
            pieces.append(f"{name}={rendered}")
        message = f"[bb] {function_qualname}::{block_label} " + ", ".join(pieces)
    else:
        message = f"[bb] {function_qualname}::{block_label}"
    print(message, file=_sys.stderr, flush=True)


def _yieldfrom_cell_value(cell):
    try:
        value = cell.cell_contents
    except ValueError:
        return None
    return value


def _current_yieldfrom(owner):
    return _yieldfrom_cell_value(owner._yield_from_cell)


class AsyncGenComplete(Exception):
    pass


def _is_cancelled_error(exc):
    asyncio_mod = _sys.modules.get("asyncio")
    if asyncio_mod is None:
        return False
    cancelled_error = getattr(asyncio_mod, "CancelledError", None)
    return cancelled_error is not None and isinstance(exc, cancelled_error)


def _reraise_control_flow(exc):
    if isinstance(exc, GeneratorExit) or _is_cancelled_error(exc):
        raise exc.with_traceback(None)
    raise exc


def _mark_closed(owner):
    owner._is_closed = True
    owner._resume_fn = None


def _normalize_throw_exc(typ, val=None, tb=None, *, where, throw_context=None):
    if val is not None or tb is not None:
        raise TypeError(f"{where} does not support value/traceback in this mode")
    exc = raise_from(typ, None)
    if exc.__context__ is None and isinstance(throw_context, BaseException):
        exc.__context__ = throw_context
    return exc


def _current_throw_context(owner):
    return _yieldfrom_cell_value(owner._throw_context_cell)

class ClosureGenerator:
    __slots__ = (
        "_resume_fn",
        "_is_closed",
        "_yield_from_cell",
        "_throw_context_cell",
        "__name__",
        "__qualname__",
        "gi_code",
    )

    def __init__(
        self,
        *,
        resume,
        name,
        qualname,
        code,
        yieldfrom_cell,
        throw_context_cell,
    ):
        self._resume_fn = resume
        self._is_closed = False
        self._yield_from_cell = yieldfrom_cell
        self._throw_context_cell = throw_context_cell
        self.__name__ = name
        self.__qualname__ = qualname
        self.gi_code = code

    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        if self._is_closed:
            raise StopIteration
        try:
            return self._resume_fn(self, value, NO_DEFAULT)
        except BaseException as exc:
            _mark_closed(self)
            _reraise_control_flow(exc)

    def throw(self, typ=None, val=None, tb=None):
        exc = _normalize_throw_exc(
            typ,
            val,
            tb,
            where="ClosureGenerator.throw()",
            throw_context=_current_throw_context(self),
        )
        if self._is_closed:
            _reraise_control_flow(exc)
        try:
            return self._resume_fn(self, NO_DEFAULT, exc)
        except BaseException as exc:
            _mark_closed(self)
            _reraise_control_flow(exc)

    def close(self):
        if self._is_closed:
            return None
        try:
            self.throw(GeneratorExit)
        except (GeneratorExit, StopIteration):
            return None
        raise RuntimeError("generator ignored GeneratorExit")

    @property
    def gi_yieldfrom(self):
        return _current_yieldfrom(self)


class Coroutine(_abc.Coroutine):
    __slots__ = ("_gen",)

    def __init__(self, gen):
        self._gen = gen

    def __await__(self):
        return self

    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        return self._gen.send(value)

    def throw(self, typ, val=None, tb=None):
        return self._gen.throw(
            _normalize_throw_exc(typ, val, tb, where="Coroutine.throw()")
        )

    def close(self):
        return self._gen.close()

    @property
    def cr_frame(self):
        return getattr(self._gen, "gi_frame", None)

    @property
    def cr_running(self):
        return False

    @property
    def cr_code(self):
        return self._gen.gi_code

    @property
    def cr_await(self):
        return self._gen.gi_yieldfrom


class ClosureAsyncGenerator:
    __slots__ = (
        "_resume_fn",
        "_is_closed",
        "_yield_from_cell",
        "_throw_context_cell",
        "__name__",
        "__qualname__",
        "ag_code",
    )

    def __init__(
        self,
        *,
        resume,
        name,
        qualname,
        code,
        yieldfrom_cell,
        throw_context_cell,
    ):
        self._resume_fn = resume
        self._is_closed = False
        self._yield_from_cell = yieldfrom_cell
        self._throw_context_cell = throw_context_cell
        self.__name__ = name
        self.__qualname__ = qualname
        self.ag_code = code

    def __aiter__(self):
        return self

    def __anext__(self):
        return self.asend(None)

    def __getattr__(self, name):
        if name == "ag_running":
            return False
        if name == "ag_frame":
            return None
        if name == "ag_await":
            return self.gi_yieldfrom
        raise AttributeError(name)

    @property
    def gi_yieldfrom(self):
        return _current_yieldfrom(self)

    def asend(self, value):
        return AsyncGenSend(self, value, NO_DEFAULT)

    def athrow(self, typ=None, val=None, tb=None):
        exc = _normalize_throw_exc(
            typ,
            val,
            tb,
            where="ClosureAsyncGenerator.athrow()",
            throw_context=_current_throw_context(self),
        )
        return AsyncGenSend(self, NO_DEFAULT, exc)

    async def aclose(self):
        try:
            await self.athrow(GeneratorExit)
        except (GeneratorExit, StopAsyncIteration):
            return None
        raise RuntimeError("async generator ignored GeneratorExit")

class AsyncGenSend:
    __slots__ = ("_generator", "_send_value", "_resume_exception", "_is_done")

    def __init__(self, gen, value, resume_exc):
        self._generator = gen
        self._send_value = value
        self._resume_exception = resume_exc
        self._is_done = False

    def __iter__(self):
        return self

    def __await__(self):
        return self

    def __next__(self):
        return self.send(None)

    def _step(self, transport_sent):
        if self._generator._is_closed:
            self._is_done = True
            resume_exc = self._resume_exception
            self._resume_exception = NO_DEFAULT
            if resume_exc is NO_DEFAULT:
                raise StopAsyncIteration
            raise StopIteration(None)
        step_send_value = (
            transport_sent
            if _current_yieldfrom(self._generator) is not None
            else self._send_value
        )
        try:
            result = self._generator._resume_fn(
                self._generator,
                step_send_value,
                self._resume_exception,
                transport_sent,
            )
        except AsyncGenComplete:
            self._is_done = True
            self._resume_exception = NO_DEFAULT
            _mark_closed(self._generator)
            raise StopAsyncIteration
        except BaseException as exc:
            self._is_done = True
            self._resume_exception = NO_DEFAULT
            _mark_closed(self._generator)
            if _is_cancelled_error(exc) or isinstance(exc, GeneratorExit):
                _reraise_control_flow(exc)
            if isinstance(exc, StopIteration):
                raise RuntimeError("async generator raised StopIteration") from exc
            if isinstance(exc, StopAsyncIteration):
                raise RuntimeError("async generator raised StopAsyncIteration") from exc
            raise exc
        self._resume_exception = NO_DEFAULT
        if _current_yieldfrom(self._generator) is None:
            self._is_done = True
            raise StopIteration(result)
        return result

    def send(self, value):
        if self._is_done:
            raise StopIteration
        if (
            value is not None
            and self._send_value is None
            and self._resume_exception is NO_DEFAULT
            and _current_yieldfrom(self._generator) is None
        ):
            raise TypeError(
                "can't send non-None value to a just-started async generator"
            )
        return self._step(value)

    def throw(self, typ, val=None, tb=None):
        if self._is_done:
            raise _normalize_throw_exc(typ, val, tb, where="AsyncGenSend.throw()")
        self._resume_exception = _normalize_throw_exc(
            typ, val, tb, where="AsyncGenSend.throw()"
        )
        return self._step(None)

    def close(self):
        return None


# TODO: very questionable
def float_from_literal(literal):
    # Preserve CPython's literal parsing for values that Rust rounds differently.
    return float(literal.replace("_", ""))


def class_lookup_cell(class_ns, name, cell):
    try:
        return class_ns[name]
    except KeyError:
        pass
    try:
        value = cell.cell_contents
    except ValueError as exc:
        raise NameError(
            f"cannot access free variable {name!r} where it is not associated with a value in enclosing scope"
        ) from exc
    if value is DELETED:
        raise NameError(
            f"cannot access free variable {name!r} where it is not associated with a value in enclosing scope"
        )
    return value


def class_lookup_global(class_ns, name, globals_dict):
    try:
        return class_ns[name]
    except KeyError:
        try:
            return globals_dict[name]
        except KeyError:
            try:
                return _builtins.__dict__[name]
            except KeyError as exc:
                raise NameError(f"name {name!r} is not defined") from exc


def _validate_exception_type(exc_type):
    if isinstance(exc_type, tuple):
        for entry in exc_type:
            _validate_exception_type(entry)
        return
    if isinstance(exc_type, type) and issubclass(exc_type, BaseException):
        return
    raise TypeError(
        "catching classes that do not inherit from BaseException is not allowed"
    )


def exception_matches(exc, exc_type):
    if isinstance(exc, RecursionError):
        return isinstance(exc, exc_type)
    _validate_exception_type(exc_type)
    return isinstance(exc, exc_type)


def exceptiongroup_split(exc, exc_type):
    _validate_exception_type(exc_type)
    if isinstance(exc, BaseExceptionGroup):
        match, rest = exc.split(exc_type)
        return match, rest
    if isinstance(exc, exc_type):
        return exc, None
    return None, exc


def unpack(iterable, spec):
    try:
        iterator = iter(iterable)
    except TypeError as exc:
        raise TypeError(
            f"cannot unpack non-iterable {type(iterable).__name__} object"
        ) from exc

    result = []
    star_index = None

    for idx, flag in enumerate(spec):
        if flag:
            try:
                result.append(next(iterator))
            except StopIteration as exc:
                raise ValueError from exc
        else:
            if star_index is not None:
                raise ValueError("only one starred target is supported")
            star_index = idx
            break

    if star_index is None:
        try:
            next(iterator)
        except StopIteration:
            return tuple(result)
        raise ValueError

    suffix_flags = list(spec[star_index + 1 :])
    if not all(suffix_flags):
        raise ValueError("only one starred target is supported")

    remainder = list(iterator)
    suffix_count = len(suffix_flags)

    if len(remainder) < suffix_count:
        raise ValueError

    if suffix_count:
        tail = remainder[-suffix_count:]
        remainder = remainder[:-suffix_count]
    else:
        tail = []

    result.append(remainder)
    result.extend(tail)
    return _builtins.tuple(result)
def call_super(super_fn, cls, instance_or_cls):
    if super_fn is _builtins.super:
        if isinstance(cls, _types.CellType):
            try:
                cls_value = cls.cell_contents
            except ValueError:
                raise RuntimeError("super(): empty __class__ cell")
            return _builtins.super(cls_value, instance_or_cls)
        return _builtins.super(cls, instance_or_cls)
    return super_fn()


def call_super_noargs(super_fn):
    if super_fn is _builtins.super:
        raise RuntimeError("super(): no arguments")
    return super_fn()


def _match_class_validate_arity(cls, match_args, total):
    allowed = 1 if match_args is None else len(match_args)
    if total > allowed:
        plural_allowed = "" if allowed == 1 else "s"
        raise TypeError(
            f"{cls.__name__}() accepts {allowed} positional sub-pattern"
            f"{plural_allowed} ({total} given)"
        )
    return allowed


def match_class_attr_exists(cls, subject, idx, total):
    match_args = getattr(cls, "__match_args__", None)
    _match_class_validate_arity(cls, match_args, total)

    if match_args is None:
        return True

    name = match_args[idx]
    return hasattr(subject, name)


def match_class_attr_value(cls, subject, idx, total):
    match_args = getattr(cls, "__match_args__", None)
    _match_class_validate_arity(cls, match_args, total)

    if match_args is None:
        return subject

    name = match_args[idx]
    return getattr(subject, name)


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
    exec("\n".join(outer_lines), {}, ns)
    code = ns["__dp_make_code"]()
    if code.co_freevars != names:
        code = code.replace(co_freevars=names)
    _DP_CODE_WITH_FREEVARS_CACHE[cache_key] = code
    return code


def _entry_template(*args, **kwargs):
    raise RuntimeError(_CLIF_ENTRY_RUNTIME_ERROR)


def code_template_gen(_it):
    while True:
        yield next(_it)


async def code_template_async_gen():
    if False:
        yield None


def make_function(
    function_id,
    kind,
    captures,
    param_defaults,
    annotate_fn=None,
):
    func = _jit_make_bb_function(
        function_id,
        captures,
        param_defaults,
        annotate_fn,
    )
    if kind == "coroutine":
        func._is_coroutine = _coroutines._is_coroutine

    return func

def create_class(
    name,
    namespace_fn,
    bases,
    kwds,
    requires_class_cell,
    firstlineno=None,
    static_attributes=(),
):
    resolved_bases = _types.resolve_bases(bases)
    meta, ns, meta_kwds = _types.prepare_class(name, resolved_bases, kwds)

    class_cell = ns.get("__classcell__", None)
    if requires_class_cell and class_cell is None:
        class_cell = _types.CellType()
        ns["__classcell__"] = class_cell

    namespace_fn(ns, class_cell)
    if "__firstlineno__" not in ns and firstlineno is not None:
        ns["__firstlineno__"] = firstlineno
    if "__static_attributes__" not in ns:
        ns["__static_attributes__"] = static_attributes

    if resolved_bases is not bases and "__orig_bases__" not in ns:
        ns["__orig_bases__"] = bases
    cls = meta(name, resolved_bases, ns, **meta_kwds)

    if cls is not None:
        ns.pop("__classcell__", None)

        if class_cell is not None:
            if isinstance(class_cell, _types.CellType):
                class_cell.cell_contents = cls
            else:
                raise TypeError("__classcell__ must be a cell")
        _soac_ext.profile_watch_type_key_layout(cls)

    return cls


def exc_info():
    exc = current_exception()
    return exc_info_from_exception(exc)


def exc_info_from_exception(exc):
    if exc is None:
        return None
    return (type(exc), exc, exc.__traceback__)

def current_exception():
    return _sys.exception()


class _AwaitIterWrapper:
    __slots__ = ("_it",)

    def __init__(self, iterator):
        self._it = iterator

    def __await__(self):
        return self._it


def _get_awaitable_iter(awaitable):
    try:
        iterator = awaitable.__await__()
    except AttributeError:
        awaitable_type = type(awaitable).__name__
        awaitable = None
        raise TypeError(
            f"'async for' received an invalid object from __anext__: {awaitable_type}"
        ) from None
    except Exception as exc:
        awaitable_type = type(awaitable).__name__
        awaitable = None
        raise TypeError(
            f"'async for' received an invalid object from __anext__: {awaitable_type}"
        ) from exc
    if not hasattr(iterator, "__next__"):
        awaitable_type = type(awaitable).__name__
        awaitable = None
        raise TypeError(
            f"'async for' received an invalid object from __anext__: {awaitable_type}"
        ) from None
    return iterator


def await_iter(awaitable):
    try:
        iterator = awaitable.__await__()
    except AttributeError:
        awaitable_type = type(awaitable).__name__
        awaitable = None
        raise TypeError(
            f"object {awaitable_type!r} can't be used in 'await' expression"
        ) from None
    except Exception as exc:
        awaitable_type = type(awaitable).__name__
        awaitable = None
        raise TypeError(
            f"object {awaitable_type!r} can't be used in 'await' expression"
        ) from exc
    if not hasattr(iterator, "__next__"):
        awaitable_type = type(awaitable).__name__
        awaitable = None
        raise TypeError(
            f"object {awaitable_type!r} can't be used in 'await' expression"
        ) from None
    return iterator


ITER_COMPLETE = object()


async def anext_or_sentinel(iterator):
    try:
        awaitable = iterator.__anext__()
    except AttributeError:
        iter_type = type(iterator).__name__
        iterator = None
        raise TypeError(
            "'async for' received an object from __aiter__ that does not implement __anext__"
            f": {iter_type}"
        ) from None
    try:
        await_iter = _get_awaitable_iter(awaitable)
    except Exception:
        iterator = None
        awaitable = None
        raise
    try:
        return await _AwaitIterWrapper(await_iter)
    except StopAsyncIteration:
        return ITER_COMPLETE


def next_or_sentinel(iterator):
    try:
        return iterator.__next__()
    except AttributeError:
        iter_type = type(iterator).__name__
        iterator = None
        raise TypeError(
            "'for' received an object from __iter__ that does not implement __next__"
            f": {iter_type}"
        ) from None
    except StopIteration:
        return ITER_COMPLETE


def raise_from(exc, cause):
    CancelledError = None
    asyncio_mod = _sys.modules.get("asyncio")
    if asyncio_mod is not None:
        CancelledError = getattr(asyncio_mod, "CancelledError", None)
    if exc is None:
        raise TypeError("exceptions must derive from BaseException")
    if isinstance(exc, type):
        if issubclass(exc, BaseException):
            exc = _call_exception_class(exc)
        else:
            raise TypeError("exceptions must derive from BaseException")
    elif not isinstance(exc, BaseException):
        raise TypeError("exceptions must derive from BaseException")
    if cause is None:
        exc.__cause__ = None
        exc.__suppress_context__ = True
    else:
        if isinstance(cause, type):
            if issubclass(cause, BaseException):
                cause = _call_exception_class(cause)
            else:
                raise TypeError("exception causes must derive from BaseException")
        elif not isinstance(cause, BaseException):
            raise TypeError("exception causes must derive from BaseException")
        if CancelledError is not None and type(cause) is CancelledError:
            cause = cause.with_traceback(None)
        exc.__cause__ = cause
        exc.__suppress_context__ = True
    return exc


def _call_exception_class(exc_type):
    inst = exc_type()
    if not isinstance(inst, BaseException):
        raise TypeError(
            f"calling {exc_type!r} should have returned an instance of BaseException, "
            f"not {type(inst)!r}"
        )
    return inst


def import_(name, spec, fromlist=None, level=0):
    if fromlist is None:
        fromlist = []
    globals_dict = {"__spec__": spec}
    if spec is not None:
        package = spec.parent
        if not package and getattr(spec, "submodule_search_locations", None):
            package = spec.name
        globals_dict["__package__"] = package
        globals_dict["__name__"] = spec.name
    try:
        return _builtins.__import__(name, globals_dict, {}, fromlist, level)
    except Exception as exc:
        raise exc from None


def import_attr(module, attr):
    try:
        return getattr(module, attr)
    except AttributeError as exc:
        module_name = getattr(module, "__name__", None)
        if module_name:
            submodule = _sys.modules.get(f"{module_name}.{attr}")
            if submodule is not None:
                try:
                    setattr(module, attr, submodule)
                except Exception:
                    _warnings.warn(
                        f"cannot set attribute {attr!r} on {module_name!r}",
                        ImportWarning,
                        stacklevel=2,
                    )
                return submodule
        module_spec = getattr(module, "__spec__", None)
        if (
            module_name
            and module_spec is not None
            and getattr(module_spec, "_initializing", False)
        ):
            message = (
                f"cannot import name {attr!r} from partially initialized module "
                f"{module_name!r} (most likely due to a circular import)"
            )
            import_error = ImportError(message, name=module_name)
            raise import_error.with_traceback(exc.__traceback__) from None
        module_name = module_name or "<unknown module name>"
        module_file = getattr(module, "__file__", None)
        message = f"cannot import name {attr!r} from {module_name!r}"
        if module_file is not None:
            message = f"{message} ({module_file})"
        else:
            message = f"{message} (unknown location)"
        import_error = ImportError(message, name=module_name, path=module_file)

        raise import_error.with_traceback(exc.__traceback__) from None


def import_star(name, spec, globals_dict, level=0):
    module = import_(name, spec, ["*"], level)
    try:
        names = getattr(module, "__all__", None)
    except Exception:
        names = None
    if names is None:
        names = [name for name in dir(module) if not name.startswith("_")]
    for name in names:
        globals_dict[name] = getattr(module, name)
    return module


def _lookup_special_method(obj, name: str):
    cls = type(obj)
    descr = _mro_getattr(cls, name)
    if descr is _MISSING:
        return _MISSING
    if hasattr(descr, "__get__"):
        return descr.__get__(obj, cls)
    return descr


def _has_special_method(obj, name: str) -> bool:
    return _mro_getattr(type(obj), name) is not _MISSING


def _missing_context_protocol_message(
    obj,
    protocol: str,
    missing_method: str,
    alt_method_names: tuple[str, str],
    hint: str,
):
    cls = type(obj)
    module = getattr(cls, "__module__", None)
    qualname = getattr(cls, "__qualname__", cls.__name__)
    if module and module != "builtins":
        type_name = f"{module}.{qualname}"
    else:
        type_name = qualname
    message = (
        f"{type_name!r} object does not support the {protocol} protocol "
        f"(missed {missing_method} method)"
    )
    if _has_special_method(obj, alt_method_names[0]) or _has_special_method(
        obj, alt_method_names[1]
    ):
        message += hint
    return message


def contextmanager_enter(ctx):
    enter = _lookup_special_method(ctx, "__enter__")
    if enter is _MISSING:
        message = _missing_context_protocol_message(
            ctx,
            "context manager",
            "__enter__",
            ("__aenter__", "__aexit__"),
            " but it supports the asynchronous context manager protocol. Did you mean to use 'async with'?",
        )
        raise TypeError(message)
    return enter()


def contextmanager_get_exit(cm):
    exit_fn = _lookup_special_method(cm, "__exit__")
    if exit_fn is _MISSING:
        message = _missing_context_protocol_message(
            cm,
            "context manager",
            "__exit__",
            ("__aenter__", "__aexit__"),
            " but it supports the asynchronous context manager protocol. Did you mean to use 'async with'?",
        )
        raise TypeError(message)
    return exit_fn


def contextmanager_exit(exit_fn, exc):
    if exc is not None:
        exc_info = (type(exc), exc, exc.__traceback__)
        try:
            suppress = exit_fn(*exc_info)
            if suppress:
                exc.__traceback__ = None
                return
            raise exc
        finally:
            # Clear the reference for GC in long-lived frames.
            exc_info = None
            exc = None
    else:
        exit_fn(None, None, None)


def _ensure_awaitable(awaitable, method_name: str, *, suppress_context: bool = True):
    try:
        iterator = awaitable.__await__()
    except AttributeError as exc:
        if suppress_context:
            awaitable_type = type(awaitable).__name__
            awaitable = None
            raise TypeError(
                f"'async with' received an object from {method_name} that does not implement __await__: {awaitable_type}"
            ) from None
        iterator = None
    except Exception as exc:
        if suppress_context:
            awaitable_type = type(awaitable).__name__
            awaitable = None
            raise TypeError(
                f"'async with' received an object from {method_name} that does not implement __await__: {awaitable_type}"
            ) from exc
        iterator = None
    if iterator is None:
        awaitable_type = type(awaitable).__name__
        awaitable = None
        raise TypeError(
            f"'async with' received an object from {method_name} that does not implement __await__: {awaitable_type}"
        )
    if not hasattr(iterator, "__next__"):
        awaitable_type = type(awaitable).__name__
        awaitable = None
        raise TypeError(
            f"'async with' received an object from {method_name} that does not implement __await__: {awaitable_type}"
        ) from None
    return iterator


async def asynccontextmanager_aenter(ctx):
    aenter = _lookup_special_method(ctx, "__aenter__")
    if aenter is _MISSING:
        message = _missing_context_protocol_message(
            ctx,
            "asynchronous context manager",
            "__aenter__",
            ("__enter__", "__exit__"),
            " but it supports the context manager protocol. Did you mean to use 'with'?",
        )
        raise TypeError(message)
    await_iter = _ensure_awaitable(aenter(), "__aenter__")
    return await _AwaitIterWrapper(await_iter)


def asynccontextmanager_get_aexit(acm):
    aexit = _lookup_special_method(acm, "__aexit__")
    if aexit is _MISSING:
        message = _missing_context_protocol_message(
            acm,
            "asynchronous context manager",
            "__aexit__",
            ("__enter__", "__exit__"),
            " but it supports the context manager protocol. Did you mean to use 'with'?",
        )
        raise TypeError(message)
    return aexit


async def asynccontextmanager_exit(exit_fn, exc):
    if exc is not None:
        exc_info = (type(exc), exc, exc.__traceback__)
        try:
            await_iter = _ensure_awaitable(
                exit_fn(*exc_info), "__aexit__", suppress_context=False
            )
            suppress = await _AwaitIterWrapper(await_iter)
            if suppress:
                exc.__traceback__ = None
                return None
            return exc
        finally:
            exc_info = None
            exc = None
    else:
        await_iter = _ensure_awaitable(exit_fn(None, None, None), "__aexit__")
        await _AwaitIterWrapper(await_iter)
        return None


_SOAC_RUNTIME_READY = True
