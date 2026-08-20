/* A fixed C consumer of the managed-generator ABI.  The explicit owner is a
 * GC-visible dictionary of scripted outcomes, not a Python generator delegate.
 * No strict source capability is created or inferred by this test fixture. */
#define Py_BUILD_CORE
#define Py_BUILD_CORE_MODULE
#include <Python.h>
#include "internal/pycore_genobject.h"
#include "internal/pycore_interp.h"
#include <stddef.h>

static PyObject *wrap_with_failure(PyObject *value);

static PyObject *
item(PyObject *owner, const char *name)
{
    return PyDict_GetItemString(owner, name);
}

static int
put(PyObject *owner, const char *name, PyObject *value)
{
    return PyDict_SetItemString(owner, name, value);
}

static int
enabled(PyObject *owner, const char *name)
{
    return item(owner, name) == Py_True;
}

static void
retire(PyObject *owner, PyObject *generator)
{
    PyObject *address = item(owner, "address");
    if (enabled(owner, "cleared") || address == NULL ||
        PyLong_AsVoidPtr(address) != generator) {
        return;
    }
    (void)put(owner, "cleared", Py_True);
    /* The fixture deliberately permits a clobbered error so that the native
     * terminal boundary, including owner decrefs, proves error preservation. */
    PyObject *count = item(owner, "clears");
    long n = count == NULL ? 0 : PyLong_AsLong(count);
    PyObject *next = PyLong_FromLong(n + 1);
    if (next != NULL) {
        (void)put(owner, "clears", next);
        Py_DECREF(next);
    }
    if (item(owner, "payload") != NULL) {
        (void)PyDict_DelItemString(owner, "payload");
    }
    PyErr_Clear();
    PyObject *error = item(owner, "clear_error");
    if (error != NULL && error != Py_None) {
        PyErr_SetRaisedException(Py_NewRef(error));
    }
}

static int
bind_owner(PyObject *owner, PyObject *gen, PyObject *function,
           PyCodeObject *code)
{
    if (!PyDict_CheckExact(owner)) {
        PyErr_SetString(PyExc_TypeError, "test owner must be an exact dict");
        return -1;
    }
    if (enabled(owner, "bound")) {
        PyErr_SetString(PyExc_ValueError, "test owner is already bound");
        return -1;
    }
    if (PyGen_MatchesSoacOwner(gen, owner) != 1) {
        PyErr_SetString(PyExc_AssertionError, "native association missing at bind");
        return -1;
    }
    if (PyObject_GC_IsTracked(gen)) {
        PyErr_SetString(PyExc_AssertionError, "generator published before bind");
        return -1;
    }
    PyObject *witness = PyWeakref_NewRef(gen, NULL);
    if (witness == NULL) {
        return -1;
    }
    int result = put(owner, "witness", witness);
    Py_DECREF(witness);
    if (result < 0 || put(owner, "bound", Py_True) < 0 ||
        put(owner, "function", function) < 0 ||
        put(owner, "code", (PyObject *)code) < 0) {
        return -1;
    }
    PyObject *address = PyLong_FromVoidPtr(gen);
    if (address == NULL) {
        return -1;
    }
    result = put(owner, "address", address);
    Py_DECREF(address);
    if (result < 0) {
        return -1;
    }
    if (enabled(owner, "reenter_bind")) {
        PyObject *value = PyIter_Next(gen);
        Py_XDECREF(value);
        if (value != NULL || !PyErr_ExceptionMatches(PyExc_RuntimeError)) {
            PyErr_SetString(PyExc_AssertionError, "unbound generator was callable");
            return -1;
        }
        PyErr_Clear();
    }
    PyObject *escaped = item(owner, "escaped");
    if (escaped != NULL && PyList_Append(escaped, gen) < 0) {
        return -1;
    }
    PyObject *error = item(owner, "bind_error");
    if (error != NULL && error != Py_None) {
        PyErr_SetRaisedException(Py_NewRef(error));
        return -1;
    }
    return 0;
}

static int
record_input(PyObject *owner, PyObject *gen, const PySoacGeneratorInput *input)
{
    PyGenObject *native = (PyGenObject *)gen;
    const char *attribute = PyGen_CheckExact(gen) ? "gi_state" :
                            PyCoro_CheckExact(gen) ? "cr_state" : "ag_state";
    const char *expected = PyGen_CheckExact(gen) ? "GEN_RUNNING" :
                           PyCoro_CheckExact(gen) ? "CORO_RUNNING" : "AGEN_RUNNING";
    PyObject *state = PyObject_GetAttrString(gen, attribute);
    int executing = state != NULL &&
                    PyUnicode_CompareWithASCIIString(state, expected) == 0;
    Py_XDECREF(state);
    if (native->gi_exc_state.exc_value != NULL ||
        native->gi_exc_state.previous_item != NULL ||
        !executing ||
        PyGen_MatchesSoacOwner(gen, owner) != 1) {
        PyErr_SetString(PyExc_AssertionError, "managed callback has a native frame activation");
        return -1;
    }
    PyObject *handled = PyErr_GetHandledException();
    PyObject *row = Py_BuildValue("(iiOOOOOOO)",
        (int)input->operation, input->close_on_genexit,
        input->arg ? input->arg : Py_None,
        input->value ? input->value : Py_None,
        input->traceback ? input->traceback : Py_None,
        input->arg ? Py_False : Py_True,
        input->value ? Py_False : Py_True,
        input->traceback ? Py_False : Py_True,
        handled ? handled : Py_None);
    Py_XDECREF(handled);
    if (row == NULL) {
        return -1;
    }
    PyObject *calls = item(owner, "calls");
    int result = calls == NULL ? -1 : PyList_Append(calls, row);
    Py_DECREF(row);
    if (result < 0 && !PyErr_Occurred()) {
        PyErr_SetString(PyExc_AssertionError, "missing test calls list");
    }
    return result;
}

static void
step_owner(PyObject *owner, PyObject *gen, const PySoacGeneratorInput *input,
           PySoacGeneratorResult *result)
{
    *result = (PySoacGeneratorResult){
        .outcome = PYGEN_ERROR,
        .state = PySoac_GENERATOR_UNCHANGED,
        .suspension = PySoac_SUSPEND_NONE,
        .value = NULL,
    };
    if (record_input(owner, gen, input) < 0) {
        return;
    }
    PyObject *observe = item(owner, "observe");
    if (observe != NULL && observe != Py_None) {
        PyObject *observed = PyObject_CallOneArg(observe, gen);
        if (observed == NULL) {
            return;
        }
        Py_DECREF(observed);
    }
    PyObject *terminal_owner = item(owner, "attempt_terminal_owner");
    if (terminal_owner != NULL) {
        if (PyGen_MarkSoacManagedTerminal(gen, terminal_owner) == 0) {
            PyErr_SetString(PyExc_AssertionError, "foreign terminal owner was accepted");
        }
        return;
    }
    PyObject *reenter = item(owner, "reenter");
    if (reenter != NULL && reenter != Py_None) {
        PyObject *method = PyObject_GetAttr(gen, reenter);
        if (method == NULL) {
            return;
        }
        PyObject *value;
        if (PyUnicode_CompareWithASCIIString(reenter, "close") == 0) {
            value = PyObject_CallNoArgs(method);
        }
        else {
            value = PyObject_CallOneArg(method, input->operation == PySoac_GENERATOR_THROW
                                               ? input->arg : Py_None);
        }
        Py_DECREF(method);
        Py_XDECREF(value);
        if (value != NULL || !PyErr_ExceptionMatches(PyExc_ValueError)) {
            PyErr_SetString(PyExc_AssertionError, "reentrant operation was not rejected");
            return;
        }
        PyObject *error = PyErr_GetRaisedException();
        int rc = put(owner, "reentrant_error", error);
        Py_DECREF(error);
        if (rc < 0) {
            return;
        }
    }
    if (input->operation == PySoac_GENERATOR_THROW && enabled(owner, "normalize_throw")) {
        PyObject *error = PyGen_NormalizeSoacThrow(input->arg, input->value,
                                                  input->traceback);
        if (error != NULL) {
            result->state = PySoac_GENERATOR_CLOSED;
            PyErr_SetRaisedException(error);
        }
        return;
    }
    PyObject *position = item(owner, "position");
    Py_ssize_t index = position == NULL ? 0 : PyLong_AsSsize_t(position);
    PyObject *steps = item(owner, "steps");
    if (index < 0 || steps == NULL || !PyList_CheckExact(steps)) {
        PyErr_SetString(PyExc_AssertionError, "invalid test script");
        return;
    }
    if (index >= PyList_GET_SIZE(steps)) {
        result->outcome = PYGEN_RETURN;
        result->state = PySoac_GENERATOR_CLOSED;
        result->value = Py_NewRef(Py_None);
        return;
    }
    PyObject *script = PyList_GET_ITEM(steps, index);
    int outcome, state, suspension = PySoac_SUSPEND_NONE;
    PyObject *value;
    if (PyTuple_Check(script) && PyTuple_GET_SIZE(script) == 4) {
        if (!PyArg_ParseTuple(script, "iiiO", &outcome, &state, &suspension, &value)) {
            return;
        }
    }
    else {
        if (!PyArg_ParseTuple(script, "iiO", &outcome, &state, &value)) {
            return;
        }
        suspension = outcome == PYGEN_NEXT ? PySoac_SUSPEND_DIRECT
                                           : PySoac_SUSPEND_NONE;
    }
    PyObject *next = PyLong_FromSsize_t(index + 1);
    if (next == NULL) {
        return;
    }
    int rc = put(owner, "position", next);
    Py_DECREF(next);
    if (rc < 0) {
        return;
    }
    result->outcome = (PySendResult)outcome;
    result->state = (PySoacGeneratorState)state;
    result->suspension = (PySoacGeneratorSuspension)suspension;
    if (outcome == PYGEN_ERROR) {
        if (value != Py_None) {
            PyErr_SetRaisedException(Py_NewRef(value));
        }
    }
    else {
        if (enabled(owner, "wrap_async_yield") &&
            suspension == PySoac_SUSPEND_ASYNC_YIELD) {
            result->value = enabled(owner, "fail_wrap")
                            ? wrap_with_failure(value) : PyAsyncGen_WrapSoacYield(value);
        }
        else {
            result->value = Py_NewRef(value);
        }
        if (result->value == NULL && enabled(owner, "recover_wrap") &&
            PyErr_ExceptionMatches(PyExc_MemoryError)) {
            PyObject *error = PyErr_GetRaisedException();
            int stored = put(owner, "caught_wrap_error", error);
            Py_DECREF(error);
            if (stored < 0 || PyGen_MatchesSoacOwner(gen, owner) != 1) {
                return;
            }
            PyObject *fallback = item(owner, "wrap_recovery_value");
            result->value = PyAsyncGen_WrapSoacYield(fallback ? fallback : Py_None);
        }
        if (result->value == NULL) {
            result->outcome = PYGEN_ERROR;
            result->state = PySoac_GENERATOR_CLOSED;
            result->suspension = PySoac_SUSPEND_NONE;
            return;
        }
        PyObject *error = item(owner, "step_error");
        if (error != NULL && error != Py_None) {
            PyErr_SetRaisedException(Py_NewRef(error));
        }
    }
    if (enabled(owner, "consume_step")) {
        /* The script is a producer, not an additional retained exception root. */
        if (PyList_SetItem(steps, index, Py_NewRef(Py_None)) < 0) {
            return;
        }
    }
    if (enabled(owner, "release_callback_payload")) {
        /* Model compiler terminal-local cleanup while the step is still on
         * the stack, before the native post-callback retirement boundary. */
        if (PyGen_MarkSoacManagedTerminal(gen, owner) < 0 ||
            PyGen_MarkSoacManagedTerminal(gen, owner) < 0) {
            return;
        }
        PyObject *error = PyErr_GetRaisedException();
        (void)PyDict_DelItemString(owner, "callback_payload");
        PyErr_SetRaisedException(error);
    }
}

static PyObject *
yield_from_owner(PyObject *owner)
{
    PyObject *delegate = item(owner, "delegate");
    return Py_NewRef(delegate == NULL ? Py_None : delegate);
}

static PyObject *
new_managed_impl(PyObject *args, int family)
{
    PyObject *function, *owner, *code = NULL, *name = NULL, *qualname = NULL;
    unsigned int abi = PySoac_GENERATOR_ABI_VERSION, reserved = 0;
    if (!PyArg_ParseTuple(args, "OO|OIIOO", &function, &owner, &code, &abi, &reserved,
                         &name, &qualname)) {
        return NULL;
    }
    if (code == NULL) {
        code = PyFunction_GetCode(function);
        if (code == NULL) {
            return NULL;
        }
    }
    const PySoacGeneratorSpec spec = {
        .abi_version = abi,
        .reserved = reserved,
        .bind = bind_owner,
        .step = step_owner,
        .yield_from = yield_from_owner,
        .clear = retire,
    };
    if (family == 0 && PyCode_Check(code)) {
        family = ((PyCodeObject *)code)->co_flags &
                 (CO_GENERATOR | CO_COROUTINE | CO_ASYNC_GENERATOR);
    }
    if (family == CO_COROUTINE) {
        return PyCoro_NewSoacManaged(function, (PyCodeObject *)code, name,
                                     qualname, owner, &spec);
    }
    if (family == CO_ASYNC_GENERATOR) {
        return PyAsyncGen_NewSoacManaged(function, (PyCodeObject *)code, name,
                                         qualname, owner, &spec);
    }
    return PyGen_NewSoacManaged(function, (PyCodeObject *)code, name,
                                qualname, owner, &spec);
}

static PyObject *
new_managed(PyObject *Py_UNUSED(module), PyObject *args)
{
    return new_managed_impl(args, 0);
}

static PyObject *
new_coroutine(PyObject *Py_UNUSED(module), PyObject *args)
{
    return new_managed_impl(args, CO_COROUTINE);
}

static PyObject *
new_async_generator(PyObject *Py_UNUSED(module), PyObject *args)
{
    return new_managed_impl(args, CO_ASYNC_GENERATOR);
}

static PyObject *
matches_owner(PyObject *Py_UNUSED(module), PyObject *args)
{
    PyObject *gen, *owner;
    if (!PyArg_ParseTuple(args, "OO", &gen, &owner)) {
        return NULL;
    }
    int result = PyGen_MatchesSoacOwner(gen, owner);
    return result < 0 ? NULL : PyLong_FromLong(result);
}

static PyObject *
mark_terminal(PyObject *Py_UNUSED(module), PyObject *args)
{
    PyObject *gen, *owner;
    if (!PyArg_ParseTuple(args, "OO", &gen, &owner) ||
        PyGen_MarkSoacManagedTerminal(gen, owner) < 0) {
        return NULL;
    }
    Py_RETURN_NONE;
}

static PyObject *
native_state(PyObject *Py_UNUSED(module), PyObject *arg)
{
    if (!PyGen_CheckExact(arg) && !PyCoro_CheckExact(arg) && !PyAsyncGen_CheckExact(arg)) {
        PyErr_SetString(PyExc_TypeError, "expected exact native suspended object");
        return NULL;
    }
    PyGenObject *gen = (PyGenObject *)arg;
    return Py_BuildValue("(iii)", gen->gi_frame_state,
                         gen->gi_exc_state.exc_value == NULL,
                         gen->gi_exc_state.previous_item == NULL);
}

static PyObject *
c_send(PyObject *Py_UNUSED(module), PyObject *args)
{
    PyObject *gen, *value, *result = NULL;
    if (!PyArg_ParseTuple(args, "OO", &gen, &value)) {
        return NULL;
    }
    PySendResult outcome = PyIter_Send(gen, value, &result);
    if (outcome == PYGEN_ERROR) {
        return NULL;
    }
    return Py_BuildValue("(iN)", (int)outcome, result);
}

static PyObject *
normalize_throw(PyObject *Py_UNUSED(module), PyObject *args)
{
    PyObject *typ, *value = NULL, *traceback = NULL;
    if (!PyArg_UnpackTuple(args, "normalize", 1, 3, &typ, &value, &traceback)) {
        return NULL;
    }
    return PyGen_NormalizeSoacThrow(typ, value, traceback);
}

static PyObject *
throw_delegate(PyObject *Py_UNUSED(module), PyObject *args)
{
    PyObject *delegate, *typ, *value = NULL, *traceback = NULL, *result = NULL;
    int close_on_genexit;
    if (!PyArg_ParseTuple(args, "OiO|OO", &delegate, &close_on_genexit,
                         &typ, &value, &traceback)) {
        return NULL;
    }
    int status = PyGen_ThrowSoacDelegate(delegate, close_on_genexit, typ, value,
                                       traceback, &result);
    PyObject *error = PyErr_GetRaisedException();
    return Py_BuildValue("(iNN)", status, result ? result : Py_NewRef(Py_None),
                         error ? error : Py_NewRef(Py_None));
}

static PyObject *
close_delegate(PyObject *Py_UNUSED(module), PyObject *delegate)
{
    if (PyGen_CloseSoacDelegate(delegate) < 0) {
        return NULL;
    }
    Py_RETURN_NONE;
}

typedef struct {
    PyMemAllocatorEx previous;
    int armed;
} WrapAllocationFailure;

static int
fail_allocation_once(WrapAllocationFailure *failure)
{
    int armed = failure->armed;
    failure->armed = 0;
    return armed;
}

static void *
fail_wrap_malloc(void *context, size_t size)
{
    WrapAllocationFailure *failure = context;
    return fail_allocation_once(failure) ? NULL
        : failure->previous.malloc(failure->previous.ctx, size);
}

static void *
fail_wrap_calloc(void *context, size_t count, size_t size)
{
    WrapAllocationFailure *failure = context;
    return fail_allocation_once(failure) ? NULL
        : failure->previous.calloc(failure->previous.ctx, count, size);
}

static void *
fail_wrap_realloc(void *context, void *pointer, size_t size)
{
    WrapAllocationFailure *failure = context;
    return fail_allocation_once(failure) ? NULL
        : failure->previous.realloc(failure->previous.ctx, pointer, size);
}

static void
fail_wrap_free(void *context, void *pointer)
{
    WrapAllocationFailure *failure = context;
    failure->previous.free(failure->previous.ctx, pointer);
}

static PyObject *
wrap_with_failure(PyObject *value)
{
    /* Keep every cached token alive, so the tested call must allocate. No
     * Python callback runs while the one-shot object allocator is installed. */
    PyInterpreterState *interpreter = PyThreadState_GetInterpreter(PyThreadState_Get());
    int cached = (int)interpreter->object_state.freelists.async_gens.size;
    PyObject *drained = PyList_New(cached > 0 ? cached : 0);
    if (drained == NULL) {
        return NULL;
    }
    for (int index = 0; index < cached; index++) {
        PyObject *token = PyAsyncGen_WrapSoacYield(Py_None);
        if (token == NULL) {
            Py_DECREF(drained);
            return NULL;
        }
        PyList_SET_ITEM(drained, index, token);
    }
    WrapAllocationFailure failure = {.armed = 1};
    PyMem_GetAllocator(PYMEM_DOMAIN_OBJ, &failure.previous);
    PyMemAllocatorEx allocator = {
        .ctx = &failure,
        .malloc = fail_wrap_malloc,
        .calloc = fail_wrap_calloc,
        .realloc = fail_wrap_realloc,
        .free = fail_wrap_free,
    };
    PyMem_SetAllocator(PYMEM_DOMAIN_OBJ, &allocator);
    PyObject *result = PyAsyncGen_WrapSoacYield(value);
    PyMem_SetAllocator(PYMEM_DOMAIN_OBJ, &failure.previous);
    PyObject *error = PyErr_GetRaisedException();
    Py_DECREF(drained);
    PyErr_SetRaisedException(error);
    if (failure.armed || result != NULL) {
        Py_XDECREF(result);
        PyErr_SetString(PyExc_AssertionError, "async wrapper allocation fault was not reached");
        return NULL;
    }
    return NULL;
}

static PyObject *
wrap_async_oom(PyObject *Py_UNUSED(module), PyObject *value)
{
    return wrap_with_failure(value);
}

static PyObject *
wrap_async_value(PyObject *Py_UNUSED(module), PyObject *value)
{
    return PyAsyncGen_WrapSoacYield(value);
}

static PyObject *
native_layout(PyObject *Py_UNUSED(module), PyObject *Py_UNUSED(args))
{
    return Py_BuildValue("{s:I,s:n,s:n,s:n,s:n,s:n,s:n,s:n,s:n,s:n,s:n,s:n}",
        "abi", (unsigned int)PySoac_GENERATOR_ABI_VERSION,
        "spec", (Py_ssize_t)sizeof(PySoacGeneratorSpec),
        "input", (Py_ssize_t)sizeof(PySoacGeneratorInput),
        "result", (Py_ssize_t)sizeof(PySoacGeneratorResult),
        "gen_metadata", (Py_ssize_t)offsetof(PyGenObject, gi_soac_managed),
        "coro_metadata", (Py_ssize_t)offsetof(PyCoroObject, cr_soac_managed),
        "asyncgen_metadata", (Py_ssize_t)offsetof(PyAsyncGenObject, ag_soac_managed),
        "coro_origin", (Py_ssize_t)offsetof(PyCoroObject, cr_origin_or_finalizer),
        "asyncgen_finalizer", (Py_ssize_t)offsetof(PyAsyncGenObject, ag_origin_or_finalizer),
        "gen_frame", (Py_ssize_t)offsetof(PyGenObject, gi_iframe),
        "coro_frame", (Py_ssize_t)offsetof(PyCoroObject, cr_iframe),
        "asyncgen_frame", (Py_ssize_t)offsetof(PyAsyncGenObject, ag_iframe));
}


static PyObject *
context_call(PyObject *Py_UNUSED(module), PyObject *args)
{
    PyObject *function, *callable, *arguments, *keywords, *pending;
    int object_call;
    if (!PyArg_ParseTuple(args, "OOOOpO", &function, &callable, &arguments,
                          &keywords, &object_call, &pending)) return NULL;
    if (!PyFunction_Check(function) || !PyTuple_Check(arguments) ||
        (keywords != Py_None && (object_call ? !PyDict_Check(keywords) : !PyTuple_Check(keywords))) ||
        (pending != Py_None && !PyExceptionInstance_Check(pending))) {
        PyErr_SetString(PyExc_TypeError, "invalid explicit contextual-call control");
        return NULL;
    }
    Py_ssize_t nargs = PyTuple_GET_SIZE(arguments);
    if (!object_call && keywords != Py_None) {
        nargs -= PyTuple_GET_SIZE(keywords);
        if (nargs < 0) {
            PyErr_SetString(PyExc_ValueError, "keyword count exceeds actual arguments");
            return NULL;
        }
    }
    /* Pin the actual function context while ordinary callbacks can execute. */
    PyObject *globals = PyFunction_GetGlobals(function);
    PyObject *builtins = ((PyFunctionObject *)function)->func_builtins;
    if (globals == NULL || builtins == NULL) {
        PyErr_SetString(PyExc_RuntimeError, "cleared explicit function context");
        return NULL;
    }
    Py_INCREF(globals);
    Py_INCREF(builtins);
    if (pending != Py_None) PyErr_SetRaisedException(Py_NewRef(pending));
    PyObject *result = object_call
        ? PySoac_ObjectCallWithContext(callable, arguments,
              keywords == Py_None ? NULL : keywords, globals, NULL, builtins)
        : PySoac_VectorcallWithContext(callable, ((PyTupleObject *)arguments)->ob_item,
              (size_t)nargs, keywords == Py_None ? NULL : keywords, globals, NULL, builtins);
    PyObject *error = PyErr_GetRaisedException();
    Py_DECREF(globals);
    Py_DECREF(builtins);
    PyErr_SetRaisedException(error);
    return result;
}

static PyObject *
ordinary_unraisable(PyObject *Py_UNUSED(module), PyObject *error)
{
    if (!PyExceptionInstance_Check(error)) {
        PyErr_SetString(PyExc_TypeError, "expected unraisable exception instance");
        return NULL;
    }
    PyErr_SetRaisedException(Py_NewRef(error));
    PyErr_WriteUnraisable(Py_None);
    if (PyErr_Occurred()) return NULL;
    Py_RETURN_NONE;
}

static PyObject *
ordinary_allocate_pending(PyObject *Py_UNUSED(module), PyObject *error)
{
    if (!PyExceptionInstance_Check(error)) {
        PyErr_SetString(PyExc_TypeError, "expected pending exception instance");
        return NULL;
    }
    PyErr_SetRaisedException(Py_NewRef(error));
    void *block = PyObject_Malloc(257);
    PyObject *actual = PyErr_GetRaisedException();
    PyObject_Free(block);
    if (block == NULL || actual != error) {
        Py_XDECREF(actual);
        PyErr_SetString(PyExc_AssertionError, "allocator tracing replaced the pending exception");
        return NULL;
    }
    return actual;
}


typedef struct {
    int primary_clears;
    int foreign_clears;
} MetadataQueryPayload;

static void
metadata_query_primary_clear(void *metadata)
{
    ((MetadataQueryPayload *)metadata)->primary_clears++;
}

static void
metadata_query_foreign_clear(void *metadata)
{
    ((MetadataQueryPayload *)metadata)->foreign_clears++;
}

static int
metadata_query_same_error(PyObject *marker)
{
    PyObject *error = PyErr_GetRaisedException();
    int same = error == marker;
    Py_XDECREF(error);
    return same;
}

#define METADATA_QUERY_CHECK(TEST, MESSAGE) do { \
    if (!(TEST)) { PyErr_SetString(PyExc_AssertionError, MESSAGE); goto done; } \
} while (0)

static PyObject *
metadata_query_checks(PyObject *Py_UNUSED(module), PyObject *args)
{
    PyObject *function, *marker;
    if (!PyArg_ParseTuple(args, "OO", &function, &marker) ||
        !PyFunction_Check(function) || !PyExceptionInstance_Check(marker)) {
        if (!PyErr_Occurred()) PyErr_SetString(PyExc_TypeError, "expected function and exception");
        return NULL;
    }
    if (PyFunction_GetSoacMetadata(function) != NULL) {
        PyErr_SetString(PyExc_ValueError, "metadata control requires an unassociated function");
        return NULL;
    }
    MetadataQueryPayload first = {0}, foreign = {0};
    PyObject *result = NULL;
    void *found = PyFunction_GetSoacMetadataForDestructorV1(function, metadata_query_primary_clear);
    METADATA_QUERY_CHECK(found == NULL && !PyErr_Occurred(), "absent metadata was not empty");
    PyErr_SetRaisedException(Py_NewRef(marker));
    found = PyFunction_GetSoacMetadataForDestructorV1(function, metadata_query_primary_clear);
    METADATA_QUERY_CHECK(found == NULL && metadata_query_same_error(marker), "absent metadata replaced error");
    for (int which = 0; which < 3; which++) {
        PyErr_SetRaisedException(Py_NewRef(marker));
        found = PyFunction_GetSoacMetadataForDestructorV1(
            which == 0 ? NULL : which == 1 ? Py_None : function,
            which == 2 ? NULL : metadata_query_primary_clear);
        METADATA_QUERY_CHECK(found == NULL && metadata_query_same_error(marker), "invalid query replaced error");
    }
    found = PyFunction_GetSoacMetadataForDestructorV1(function, NULL);
    METADATA_QUERY_CHECK(found == NULL && PyErr_ExceptionMatches(PyExc_TypeError), "NULL destructor accepted");
    PyErr_Clear();
    if (PyFunction_SetSoacMetadata(function, 17, &first, metadata_query_primary_clear) < 0) goto done;
    found = PyFunction_GetSoacMetadataForDestructorV1(function, metadata_query_primary_clear);
    METADATA_QUERY_CHECK(found == &first && !PyErr_Occurred(), "matching metadata pointer changed");
    PyErr_SetRaisedException(Py_NewRef(marker));
    found = PyFunction_GetSoacMetadataForDestructorV1(function, metadata_query_primary_clear);
    METADATA_QUERY_CHECK(found == &first && metadata_query_same_error(marker), "matching metadata replaced error");
    found = PyFunction_GetSoacMetadataForDestructorV1(function, metadata_query_foreign_clear);
    METADATA_QUERY_CHECK(found == NULL && PyErr_ExceptionMatches(PyExc_RuntimeError), "foreign destructor exposed payload");
    PyErr_Clear();
    METADATA_QUERY_CHECK(first.primary_clears == 0 && PyFunction_GetSoacFunctionId(function) == 17,
                         "query changed the actual association");
    if (PyFunction_SetSoacMetadata(function, 23, &foreign, metadata_query_foreign_clear) < 0) goto done;
    METADATA_QUERY_CHECK(first.primary_clears == 1, "replacement did not retire old metadata exactly once");
    found = PyFunction_GetSoacMetadataForDestructorV1(function, metadata_query_primary_clear);
    METADATA_QUERY_CHECK(found == NULL && PyErr_ExceptionMatches(PyExc_RuntimeError), "replaced metadata exposed old C type");
    PyErr_Clear();
    PyErr_SetRaisedException(Py_NewRef(marker));
    found = PyFunction_GetSoacMetadataForDestructorV1(function, metadata_query_primary_clear);
    METADATA_QUERY_CHECK(found == NULL && metadata_query_same_error(marker), "mismatch replaced pending error");
    found = PyFunction_GetSoacMetadataForDestructorV1(function, metadata_query_foreign_clear);
    METADATA_QUERY_CHECK(found == &foreign && !PyErr_Occurred(), "actual foreign destructor did not match");
    if (PyFunction_SetSoacMetadata(function, 0, NULL, NULL) < 0) goto done;
    METADATA_QUERY_CHECK(first.primary_clears == 1 && first.foreign_clears == 0 &&
                         foreign.primary_clears == 0 && foreign.foreign_clears == 1 &&
                         PyFunction_GetSoacMetadataForDestructorV1(function, metadata_query_primary_clear) == NULL &&
                         !PyErr_Occurred(), "metadata clear leaked or called the wrong destructor");
    result = Py_BuildValue("(iii)", first.primary_clears, foreign.foreign_clears, 1);
done:
    if (PyFunction_GetSoacMetadata(function) == &first || PyFunction_GetSoacMetadata(function) == &foreign) {
        PyObject *error = PyErr_GetRaisedException();
        if (PyFunction_SetSoacMetadata(function, 0, NULL, NULL) < 0) {
            Py_FatalError("metadata query fixture failed to detach its stack payload");
        }
        PyErr_SetRaisedException(error);
    }
    return result;
}

#undef METADATA_QUERY_CHECK

static PyMethodDef methods[] = {
    {"context_call", context_call, METH_VARARGS, NULL},
    {"ordinary_unraisable", ordinary_unraisable, METH_O, NULL},
    {"ordinary_allocate_pending", ordinary_allocate_pending, METH_O, NULL},
    {"metadata_query_checks", metadata_query_checks, METH_VARARGS, NULL},
    {"new", new_managed, METH_VARARGS, NULL},
    {"new_coroutine", new_coroutine, METH_VARARGS, NULL},
    {"new_asyncgen", new_async_generator, METH_VARARGS, NULL},
    {"matches", matches_owner, METH_VARARGS, NULL},
    {"mark_terminal", mark_terminal, METH_VARARGS, NULL},
    {"state", native_state, METH_O, NULL},
    {"send", c_send, METH_VARARGS, NULL},
    {"normalize", normalize_throw, METH_VARARGS, NULL},
    {"throw_delegate", throw_delegate, METH_VARARGS, NULL},
    {"close_delegate", close_delegate, METH_O, NULL},
    {"wrap_async", wrap_async_value, METH_O, NULL},
    {"wrap_async_oom", wrap_async_oom, METH_O, NULL},
    {"layout", native_layout, METH_NOARGS, NULL},
    {NULL, NULL, 0, NULL}
};

static struct PyModuleDef module = {
    PyModuleDef_HEAD_INIT,
    .m_name = "_strict_managed_generator",
    .m_size = 0,
    .m_methods = methods,
};

PyMODINIT_FUNC
PyInit__strict_managed_generator(void)
{
    return PyModule_Create(&module);
}
