#include <Python.h>

typedef struct {
    PyObject_HEAD
    PyObject *error;
} ErrorIterator;

static int iterator_traverse(ErrorIterator *self, visitproc visit, void *arg)
{
    Py_VISIT(self->error);
    return 0;
}

static int iterator_clear(ErrorIterator *self)
{
    Py_CLEAR(self->error);
    return 0;
}

static void iterator_dealloc(ErrorIterator *self)
{
    PyObject_GC_UnTrack(self);
    iterator_clear(self);
    Py_TYPE(self)->tp_free((PyObject *)self);
}

static PyObject *iterator_next(ErrorIterator *self)
{
    if (self->error == NULL) {
        return NULL;
    }
    PyErr_SetRaisedException(Py_NewRef(self->error));
    return NULL;
}

static PyTypeObject iterator_type = {
    PyVarObject_HEAD_INIT(NULL, 0)
    .tp_name = "_loop_error_native.Iterator",
    .tp_basicsize = sizeof(ErrorIterator),
    .tp_flags = Py_TPFLAGS_DEFAULT | Py_TPFLAGS_HAVE_GC,
    .tp_dealloc = (destructor)iterator_dealloc,
    .tp_traverse = (traverseproc)iterator_traverse,
    .tp_clear = (inquiry)iterator_clear,
    .tp_iter = PyObject_SelfIter,
    .tp_iternext = (iternextfunc)iterator_next,
};

static PyObject *make_iterator(PyObject *module, PyObject *error)
{
    (void)module;
    if (!PyExceptionInstance_Check(error)) {
        PyErr_SetString(PyExc_TypeError, "expected an exception instance");
        return NULL;
    }
    ErrorIterator *iterator = (ErrorIterator *)iterator_type.tp_alloc(&iterator_type, 0);
    if (iterator == NULL) {
        return NULL;
    }
    iterator->error = Py_NewRef(error);
    return (PyObject *)iterator;
}

static PyMethodDef methods[] = {
    {"make", make_iterator, METH_O, NULL},
    {NULL, NULL, 0, NULL},
};

static struct PyModuleDef module = {
    PyModuleDef_HEAD_INIT,
    .m_name = "_loop_error_native",
    .m_size = -1,
    .m_methods = methods,
};

PyMODINIT_FUNC PyInit__loop_error_native(void)
{
    if (PyType_Ready(&iterator_type) < 0) {
        return NULL;
    }
    return PyModule_Create(&module);
}
