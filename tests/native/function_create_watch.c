/* Test-only CREATE observation/invocation. No ctypes callback enters Python
 * before the watcher's pending exception has been saved and cleared. */
#include <Python.h>
#include <stddef.h>

typedef struct {
    PyObject *globals;
    PyObject *name;
    PyObject *arguments;
    PyObject *keywords;
    PyObject *events;
    int watcher_plus_one;
    int invoke;
    int seen;
} WatchState;

static struct PyModuleDef watch_module;

static int
clear_watch(WatchState *state)
{
    int watcher = state->watcher_plus_one - 1;
    state->watcher_plus_one = 0;
    state->seen = 1;
    int status = watcher < 0 ? 0 : PyFunction_ClearWatcher(watcher);
    Py_CLEAR(state->globals);
    Py_CLEAR(state->name);
    Py_CLEAR(state->arguments);
    Py_CLEAR(state->keywords);
    Py_CLEAR(state->events);
    return status;
}

static int
observe_create(PyFunction_WatchEvent event, PyFunctionObject *function,
               PyObject *new_value)
{
    (void)new_value;
    if (event != PyFunction_EVENT_CREATE) {
        return 0;  /* DESTROY may already have an exception pending. */
    }
    PyObject *pending = PyErr_GetRaisedException();
    PyObject *module = Py_XNewRef(PyState_FindModule(&watch_module));
    PyObject *arguments = NULL, *keywords = NULL, *events = NULL;
    PyObject *result = NULL, *row = NULL;
    int status = 0;
    if (module == NULL) {
        status = PyErr_Occurred() ? -1 : 0;
        goto done;
    }
    WatchState *state = PyModule_GetState(module);
    PyCodeObject *code = (PyCodeObject *)function->func_code;
    if (!state->watcher_plus_one || state->seen ||
        state->globals != function->func_globals) {
        goto done;
    }
    int comparison = PyUnicode_Compare(state->name, code->co_name);
    if (comparison != 0) {
        status = PyErr_Occurred() ? -1 : 0;
        goto done;
    }
    /* Consume the observation before allocation or the attempted call. The
     * function may execute ordinary code, including stop(), so pin its inputs. */
    state->seen = 1;
    arguments = Py_NewRef(state->arguments);
    keywords = Py_XNewRef(state->keywords);
    events = Py_NewRef(state->events);
    int invoke = state->invoke;
    int flags = code->co_flags;
    int freevars = code->co_nfreevars;
    int closure_present = function->func_closure != NULL;
    unsigned long long source_id = PyCode_GetSoacStrictSourceId((PyObject *)code);
    int owner_present = PyFunction_GetSoacStrictOwner((PyObject *)function) != NULL;
    int creation = PyFunction_HasSoacDataclassCreation((PyObject *)function);
    if (PyErr_Occurred() || creation < 0) {
        status = -1;
        goto done;
    }
    PyObject *success = Py_None;
    if (invoke) {
        result = PyObject_Call((PyObject *)function, arguments, keywords);
        success = result == NULL ? Py_False : Py_True;
        if (result == NULL) {
            result = PyErr_GetRaisedException();
            if (result == NULL) {
                PyErr_SetString(PyExc_AssertionError,
                                "CREATE call returned NULL without an exception");
                status = -1;
                goto done;
            }
        }
    }
    else {
        result = Py_NewRef(Py_None);
    }
    row = Py_BuildValue(
        "{s:O,s:i,s:K,s:O,s:i,s:O,s:i,s:O,s:O,s:O}",
        "function", (PyObject *)function,
        "flags", flags, "source_id", source_id,
        "owner_present", owner_present ? Py_True : Py_False,
        "creation", creation,
        "closure_present", closure_present ? Py_True : Py_False,
        "freevars", freevars, "invoked", invoke ? Py_True : Py_False,
        "success", success, "result", result);
    status = row == NULL ? -1 : PyList_Append(events, row);

done:
    Py_XDECREF(row);
    Py_XDECREF(result);
    Py_XDECREF(events);
    Py_XDECREF(keywords);
    Py_XDECREF(arguments);
    Py_XDECREF(module);
    if (pending != NULL) {
        if (status < 0) {
            PyErr_WriteUnraisable((PyObject *)function);
        }
        PyErr_SetRaisedException(pending);
        return 0;
    }
    return status;
}

static PyObject *
watch(PyObject *module, PyObject *args, PyObject *kwargs)
{
    static char *names[] = {"globals", "name", "args", "kwargs", "invoke", NULL};
    PyObject *globals, *name, *arguments;
    PyObject *keywords = Py_None, *invoke = Py_True;
    if (!PyArg_ParseTupleAndKeywords(args, kwargs, "OOO|OO:watch", names,
                                    &globals, &name, &arguments, &keywords, &invoke)) {
        return NULL;
    }
    if (!PyDict_CheckExact(globals) || !PyUnicode_CheckExact(name) ||
        !PyTuple_CheckExact(arguments) ||
        (keywords != Py_None && !PyDict_CheckExact(keywords)) ||
        !PyBool_Check(invoke)) {
        PyErr_SetString(PyExc_TypeError,
                        "watch needs exact globals/name/args/optional kwargs and bool invoke");
        return NULL;
    }
    WatchState *state = PyModule_GetState(module);
    if (state->watcher_plus_one) {
        PyErr_SetString(PyExc_RuntimeError, "a CREATE observation is already active");
        return NULL;
    }
    PyObject *events = PyList_New(0);
    if (events == NULL) {
        return NULL;
    }
    int watcher = PyFunction_AddWatcher(observe_create);
    if (watcher < 0) {
        Py_DECREF(events);
        return NULL;
    }
    state->globals = Py_NewRef(globals);
    state->name = Py_NewRef(name);
    state->arguments = Py_NewRef(arguments);
    state->keywords = keywords == Py_None ? NULL : Py_NewRef(keywords);
    state->events = events;
    state->invoke = invoke == Py_True;
    state->seen = 0;
    state->watcher_plus_one = watcher + 1;
    return Py_NewRef(events);
}

static PyObject *
stop(PyObject *module, PyObject *unused)
{
    (void)unused;
    if (clear_watch(PyModule_GetState(module)) < 0) {
        return NULL;
    }
    Py_RETURN_NONE;
}

/* These probes use the selected interpreter's headers, not a ctypes layout.
 * They exercise the supported handled-exception setter from ordinary calls. */
static PyObject *
handled_exception_layout(PyObject *module, PyObject *unused)
{
    (void)module;
    (void)unused;
    return Py_BuildValue(
        "{s:n,s:n,s:n,s:n,s:n}",
        "thread_exc_info", (Py_ssize_t)offsetof(PyThreadState, exc_info),
        "item_size", (Py_ssize_t)sizeof(_PyErr_StackItem),
        "item_alignment", (Py_ssize_t)_Alignof(_PyErr_StackItem),
        "item_value", (Py_ssize_t)offsetof(_PyErr_StackItem, exc_value),
        "item_previous", (Py_ssize_t)offsetof(_PyErr_StackItem, previous_item));
}

static PyObject *
set_handled_exception(PyObject *module, PyObject *value)
{
    (void)module;
    if (value != Py_None && !PyExceptionInstance_Check(value)) {
        PyErr_SetString(PyExc_TypeError, "expected an exception instance or None");
        return NULL;
    }
    PyErr_SetHandledException(value);
    Py_RETURN_NONE;
}

/* Existing exported stock entry, declared by the selected native source in
 * Include/internal/pycore_function.h. This external fixture deliberately does
 * not define Py_BUILD_CORE or include core-only frame/owner APIs. */
PyAPI_FUNC(PyObject *) _PyFunction_Vectorcall(
    PyObject *function, PyObject *const *arguments, size_t nargsf,
    PyObject *kwnames);

static PyObject *
forward_stock(PyObject *function, PyObject *const *arguments, size_t nargsf,
              PyObject *kwnames)
{
    return _PyFunction_Vectorcall(function, arguments, nargsf, kwnames);
}

static PyObject *
install_stock_forwarder(PyObject *module, PyObject *function)
{
    (void)module;
    if (!PyFunction_Check(function)) {
        PyErr_SetString(PyExc_TypeError, "expected a Python function");
        return NULL;
    }
    /* No owner, checked-entry registration, callback table, or Python edge.
     * The common native frame initialization must enforce the actual owner. */
    PyFunction_SetVectorcall((PyFunctionObject *)function, forward_stock);
    Py_RETURN_NONE;
}

static int
watch_traverse(PyObject *module, visitproc visit, void *arg)
{
    WatchState *state = PyModule_GetState(module);
    Py_VISIT(state->globals);
    Py_VISIT(state->name);
    Py_VISIT(state->arguments);
    Py_VISIT(state->keywords);
    Py_VISIT(state->events);
    return 0;
}

static int
watch_clear(PyObject *module)
{
    PyObject *pending = PyErr_GetRaisedException();
    if (clear_watch(PyModule_GetState(module)) < 0) {
        PyErr_WriteUnraisable(module);
    }
    PyErr_SetRaisedException(pending);
    return 0;
}

static void
watch_free(void *module)
{
    (void)watch_clear((PyObject *)module);
}

static PyMethodDef methods[] = {
    {"watch", _PyCFunction_CAST(watch), METH_VARARGS | METH_KEYWORDS, NULL},
    {"stop", stop, METH_NOARGS, NULL},
    {"handled_exception_layout", handled_exception_layout, METH_NOARGS, NULL},
    {"set_handled_exception", set_handled_exception, METH_O, NULL},
    {"install_stock_forwarder", install_stock_forwarder, METH_O, NULL},
    {NULL, NULL, 0, NULL}
};

static struct PyModuleDef watch_module = {
    PyModuleDef_HEAD_INIT,
    .m_name = "_strict_function_create_watch",
    .m_size = sizeof(WatchState),
    .m_methods = methods,
    .m_traverse = watch_traverse,
    .m_clear = watch_clear,
    .m_free = watch_free,
};

PyMODINIT_FUNC
PyInit__strict_function_create_watch(void)
{
#ifdef Py_GIL_DISABLED
    PyErr_SetString(PyExc_ImportError, "CREATE observation fixture requires the GIL");
    return NULL;
#else
    return PyModule_Create(&watch_module);
#endif
}
