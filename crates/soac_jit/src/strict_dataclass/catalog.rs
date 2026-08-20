//! Cold helper attestation and callback-free live witnesses.
//!
//! A Python function's spelling selects an independently compiled recipe; it
//! does not prove ownership. The actual code, complete defaults, common
//! globals/builtins, and ordinary native entry must agree. References retained
//! by the catalog are weak. Every privileged edge rechecks the live graph.

use std::ffi::{c_char, c_int};
use std::ptr;

use pyo3::ffi;
use pyo3::prelude::*;

use crate::strict_state::{StrictStateData, StrictStateRef};

use super::StdlibRecipes;
use super::code::CodeRecipe;
use super::edges::{CodeRole, Edge, ResolvedEdge, Template};

unsafe extern "C" {
    fn PyFunction_GetSoacStrictOwner(function: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn _PyFunction_Vectorcall(
        callable: *mut ffi::PyObject,
        args: *const *mut ffi::PyObject,
        nargsf: usize,
        kwnames: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
    fn PySoac_GetDataclassBuiltin(kind: u32) -> *mut ffi::PyObject;
    fn PySoac_MatchesBuiltinFunction(
        actual: *mut ffi::PyObject,
        name: *const c_char,
        name_length: ffi::Py_ssize_t,
    ) -> c_int;
}

/// A reader never materializes Python containers. Bound references only pin
/// existing edges while a validation operation is in progress.
pub(super) trait References<'py> {
    fn reference(&self, index: usize) -> PyResult<Bound<'py, PyAny>>;
}

impl<'py> References<'py> for Vec<Bound<'py, PyAny>> {
    fn reference(&self, index: usize) -> PyResult<Bound<'py, PyAny>> {
        self.get(index).cloned().ok_or_else(|| {
            pyo3::exceptions::PySystemError::new_err("dataclass witness index is absent")
        })
    }
}

impl<'py, T: StrictStateData> References<'py> for StrictStateRef<'py, T> {
    fn reference(&self, index: usize) -> PyResult<Bound<'py, PyAny>> {
        StrictStateRef::reference(self, index)
    }
}

#[derive(Clone, Copy)]
pub(super) struct WeakIdentity(usize);

impl WeakIdentity {
    pub(super) fn capture<'py>(
        py: Python<'py>,
        value: &Bound<'py, PyAny>,
        references: &mut Vec<Bound<'py, PyAny>>,
    ) -> PyResult<Self> {
        let weak = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                ffi::PyWeakref_NewRef(value.as_ptr(), ptr::null_mut()),
            )?
        };
        let index = references.len();
        references.push(weak);
        Ok(Self(index))
    }

    pub(super) fn upgrade<'py>(
        self,
        py: Python<'py>,
        references: &impl References<'py>,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        let weak = references.reference(self.0)?;
        let mut value = ptr::null_mut();
        match unsafe { ffi::PyWeakref_GetRef(weak.as_ptr(), &mut value) } {
            0 => Ok(None),
            1 => Ok(Some(unsafe { Bound::from_owned_ptr(py, value) })),
            _ => Err(PyErr::fetch(py)),
        }
    }

    pub(super) fn matches<'py>(
        self,
        py: Python<'py>,
        references: &impl References<'py>,
        actual: *mut ffi::PyObject,
    ) -> PyResult<bool> {
        Ok(self
            .upgrade(py, references)?
            .is_some_and(|value| value.as_ptr() == actual))
    }
}

/// Exact Unicode payload comparison, including lone surrogates. No hash,
/// UTF-8 cache allocation, rich comparison, or Python attribute lookup.
pub(super) unsafe fn text_is(value: *mut ffi::PyObject, expected: &str) -> bool {
    if value.is_null() || unsafe { ffi::PyUnicode_CheckExact(value) } == 0 {
        return false;
    }
    let length = unsafe { ffi::PyUnicode_GetLength(value) };
    length == expected.chars().count() as ffi::Py_ssize_t
        && expected.chars().enumerate().all(|(index, character)| {
            (unsafe { ffi::PyUnicode_ReadChar(value, index as ffi::Py_ssize_t) })
                == u32::from(character)
        })
}

/// Read an exact dictionary without invoking a hostile key's equality/hash.
/// A custom key anywhere makes this graph unsupported, even when it does not
/// currently collide with the selected name.
pub(super) unsafe fn dictionary_value(
    dictionary: *mut ffi::PyObject,
    name: &str,
) -> Option<*mut ffi::PyObject> {
    if dictionary.is_null() || unsafe { ffi::PyDict_CheckExact(dictionary) } == 0 {
        return None;
    }
    let mut position = 0;
    let mut key = ptr::null_mut();
    let mut value = ptr::null_mut();
    let mut found = None;
    while unsafe { ffi::PyDict_Next(dictionary, &mut position, &mut key, &mut value) } != 0 {
        if unsafe { ffi::PyUnicode_CheckExact(key) } == 0 {
            return None;
        }
        if unsafe { text_is(key, name) } {
            found = Some(value);
        }
    }
    found
}

unsafe fn exact_text_dictionary(dictionary: *mut ffi::PyObject) -> bool {
    if dictionary.is_null() || unsafe { ffi::PyDict_CheckExact(dictionary) } == 0 {
        return false;
    }
    let mut position = 0;
    let mut key = ptr::null_mut();
    let mut value = ptr::null_mut();
    while unsafe { ffi::PyDict_Next(dictionary, &mut position, &mut key, &mut value) } != 0 {
        if unsafe { ffi::PyUnicode_CheckExact(key) } == 0 {
            return false;
        }
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub(super) enum Helper {
    Dataclass,
    ProcessClass,
    Field,
    FieldsInInitOrder,
    TupleString,
    BuilderInit,
    BuilderAdd,
    BuilderInstall,
    MakeAnnotate,
    FieldAssign,
    FieldInit,
    InitParameter,
    Init,
    Frozen,
    IsClassVar,
    IsInitVar,
    IsKeywordOnly,
    IsType,
    GetField,
    SetNewAttribute,
    HashNone,
    HashAdd,
    HashError,
    FieldInitMethod,
    FieldSetName,
    ParamsInit,
    RecursiveRepr,
    Replace,
    Fields,
    AddSlots,
    CreateSlots,
    GetSlots,
    UpdateClassCell,
    GetState,
    SetState,
}

impl Helper {
    const ALL: &[Self] = &[
        Self::Dataclass,
        Self::ProcessClass,
        Self::Field,
        Self::FieldsInInitOrder,
        Self::TupleString,
        Self::BuilderInit,
        Self::BuilderAdd,
        Self::BuilderInstall,
        Self::MakeAnnotate,
        Self::FieldAssign,
        Self::FieldInit,
        Self::InitParameter,
        Self::Init,
        Self::Frozen,
        Self::IsClassVar,
        Self::IsInitVar,
        Self::IsKeywordOnly,
        Self::IsType,
        Self::GetField,
        Self::SetNewAttribute,
        Self::HashNone,
        Self::HashAdd,
        Self::HashError,
        Self::FieldInitMethod,
        Self::FieldSetName,
        Self::ParamsInit,
        Self::RecursiveRepr,
        Self::Replace,
        Self::Fields,
        Self::AddSlots,
        Self::CreateSlots,
        Self::GetSlots,
        Self::UpdateClassCell,
        Self::GetState,
        Self::SetState,
    ];

    fn path(self) -> &'static str {
        match self {
            Self::Dataclass => "dataclass",
            Self::ProcessClass => "_process_class",
            Self::Field => "field",
            Self::FieldsInInitOrder => "_fields_in_init_order",
            Self::TupleString => "_tuple_str",
            Self::BuilderInit => "_FuncBuilder.__init__",
            Self::BuilderAdd => "_FuncBuilder.add_fn",
            Self::BuilderInstall => "_FuncBuilder.add_fns_to_class",
            Self::MakeAnnotate => "_make_annotate_function",
            Self::FieldAssign => "_field_assign",
            Self::FieldInit => "_field_init",
            Self::InitParameter => "_init_param",
            Self::Init => "_init_fn",
            Self::Frozen => "_frozen_get_del_attr",
            Self::IsClassVar => "_is_classvar",
            Self::IsInitVar => "_is_initvar",
            Self::IsKeywordOnly => "_is_kw_only",
            Self::IsType => "_is_type",
            Self::GetField => "_get_field",
            Self::SetNewAttribute => "_set_new_attribute",
            Self::HashNone => "_hash_set_none",
            Self::HashAdd => "_hash_add",
            Self::HashError => "_hash_exception",
            Self::FieldInitMethod => "Field.__init__",
            Self::FieldSetName => "Field.__set_name__",
            Self::ParamsInit => "_DataclassParams.__init__",
            Self::RecursiveRepr => "recursive_repr",
            Self::Replace => "_replace",
            Self::Fields => "fields",
            Self::AddSlots => "_add_slots",
            Self::CreateSlots => "_create_slots",
            Self::GetSlots => "_get_slots",
            Self::UpdateClassCell => "_update_func_cell_for__class__",
            Self::GetState => "_dataclass_getstate",
            Self::SetState => "_dataclass_setstate",
        }
    }

    fn recipe(self, recipes: &StdlibRecipes) -> Option<&CodeRecipe> {
        let module = if self == Self::RecursiveRepr {
            &recipes.reprlib
        } else {
            &recipes.dataclasses
        };
        module.definition(self.path())
    }

    fn defaults(self) -> &'static [ValueRule] {
        match self {
            Self::Dataclass => &[ValueRule::None],
            Self::RecursiveRepr => &[ValueRule::Text("...")],
            _ => &[],
        }
    }

    fn keyword_defaults(self) -> &'static [(&'static str, ValueRule)] {
        match self {
            Self::Dataclass => &[
                ("init", ValueRule::Bool(true)),
                ("repr", ValueRule::Bool(true)),
                ("eq", ValueRule::Bool(true)),
                ("order", ValueRule::Bool(false)),
                ("unsafe_hash", ValueRule::Bool(false)),
                ("frozen", ValueRule::Bool(false)),
                ("match_args", ValueRule::Bool(true)),
                ("kw_only", ValueRule::Bool(false)),
                ("slots", ValueRule::Bool(false)),
                ("weakref_slot", ValueRule::Bool(false)),
            ],
            Self::Field => &[
                ("default", ValueRule::Sentinel(Sentinel::Missing)),
                ("default_factory", ValueRule::Sentinel(Sentinel::Missing)),
                ("init", ValueRule::Bool(true)),
                ("repr", ValueRule::Bool(true)),
                ("hash", ValueRule::None),
                ("compare", ValueRule::Bool(true)),
                ("metadata", ValueRule::None),
                ("kw_only", ValueRule::Sentinel(Sentinel::Missing)),
                ("doc", ValueRule::None),
            ],
            Self::BuilderAdd => &[
                ("locals", ValueRule::None),
                ("return_type", ValueRule::Sentinel(Sentinel::Missing)),
                ("overwrite_error", ValueRule::Bool(false)),
                ("unconditional_add", ValueRule::Bool(false)),
                ("decorator", ValueRule::None),
                ("annotation_fields", ValueRule::None),
            ],
            _ => &[],
        }
    }
}

#[derive(Clone, Copy)]
#[repr(usize)]
pub(super) enum Sentinel {
    Missing,
    Factory,
    KeywordOnly,
    Field,
    ClassVar,
    InitVar,
}

impl Sentinel {
    const ALL: &[Self] = &[
        Self::Missing,
        Self::Factory,
        Self::KeywordOnly,
        Self::Field,
        Self::ClassVar,
        Self::InitVar,
    ];
    fn name(self) -> &'static str {
        match self {
            Self::Missing => "MISSING",
            Self::Factory => "_HAS_DEFAULT_FACTORY",
            Self::KeywordOnly => "KW_ONLY",
            Self::Field => "_FIELD",
            Self::ClassVar => "_FIELD_CLASSVAR",
            Self::InitVar => "_FIELD_INITVAR",
        }
    }
    fn class(self) -> &'static str {
        match self {
            Self::Missing => "_MISSING_TYPE",
            Self::Factory => "_HAS_DEFAULT_FACTORY_CLASS",
            Self::KeywordOnly => "_KW_ONLY_TYPE",
            Self::Field | Self::ClassVar | Self::InitVar => "_FIELD_BASE",
        }
    }
}

#[derive(Clone, Copy)]
enum ValueRule {
    None,
    Bool(bool),
    Text(&'static str),
    Sentinel(Sentinel),
}

struct FunctionWitness {
    function: WeakIdentity,
    code: WeakIdentity,
    recipe: CodeRecipe,
    globals: usize,
    builtins: usize,
}

struct TemplateWitness {
    code: WeakIdentity,
    recipe: CodeRecipe,
}

#[derive(Clone, Copy)]
#[repr(usize)]
pub(super) enum StructType {
    Field,
    Parameters,
    Builder,
    InitVar,
    FrozenError,
}

impl StructType {
    const ALL: &[Self] = &[
        Self::Field,
        Self::Parameters,
        Self::Builder,
        Self::InitVar,
        Self::FrozenError,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Field => "Field",
            Self::Parameters => "_DataclassParams",
            Self::Builder => "_FuncBuilder",
            Self::InitVar => "InitVar",
            Self::FrozenError => "FrozenInstanceError",
        }
    }

    fn slots(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Field => Some(&[
                "name",
                "type",
                "default",
                "default_factory",
                "repr",
                "hash",
                "init",
                "compare",
                "metadata",
                "kw_only",
                "doc",
                "_field_type",
            ]),
            Self::Parameters => Some(&[
                "init",
                "repr",
                "eq",
                "order",
                "unsafe_hash",
                "frozen",
                "match_args",
                "kw_only",
                "slots",
                "weakref_slot",
            ]),
            Self::Builder | Self::FrozenError => None,
            Self::InitVar => Some(&["type"]),
        }
    }
}

/// Slot offsets come from the exact public member descriptor, with owner,
/// type, flags, bounds, uniqueness and ordinary heap layout checked. No
/// Python property or guessed private instance mirror participates.
struct StructWitness {
    class: WeakIdentity,
    slots: Vec<(&'static str, usize)>,
}

impl StructWitness {
    fn capture<'py>(
        py: Python<'py>,
        class: &Bound<'py, PyAny>,
        shape: StructType,
        references: &mut Vec<Bound<'py, PyAny>>,
    ) -> PyResult<Option<Self>> {
        let Some(slots) = (unsafe { struct_slots(class.as_ptr(), shape) }) else {
            return Ok(None);
        };
        let witness = Self {
            class: WeakIdentity::capture(py, class, references)?,
            slots,
        };
        Ok(Some(witness))
    }

    fn matches<'py>(
        &self,
        py: Python<'py>,
        references: &impl References<'py>,
        class: *mut ffi::PyObject,
        shape: StructType,
    ) -> PyResult<bool> {
        if !self.class.matches(py, references, class)? {
            return Ok(false);
        }
        if matches!(shape, StructType::FrozenError) {
            return Ok(unsafe { plain_exception_type(class) });
        }
        if !unsafe { plain_object_type(class) } {
            return Ok(false);
        }
        let kind = class.cast::<ffi::PyTypeObject>();
        if let Some(names) = shape.slots() {
            let expected_size = std::mem::size_of::<ffi::PyObject>()
                + names.len() * std::mem::size_of::<*mut ffi::PyObject>();
            if unsafe { (*kind).tp_basicsize } != expected_size as ffi::Py_ssize_t
                || unsafe { (*kind).tp_itemsize } != 0
                || unsafe { (*kind).tp_dictoffset } != 0
                || unsafe { (*kind).tp_weaklistoffset } != 0
            {
                return Ok(false);
            }
            let dictionary = unsafe { (*kind).tp_dict };
            let Some(slots) = (unsafe { dictionary_value(dictionary, "__slots__") }) else {
                return Ok(false);
            };
            if unsafe { ffi::PyTuple_CheckExact(slots) } == 0
                || unsafe { ffi::PyTuple_Size(slots) } != names.len() as ffi::Py_ssize_t
            {
                return Ok(false);
            }
            for (index, name) in names.iter().enumerate() {
                if !unsafe { text_is(ffi::PyTuple_GetItem(slots, index as ffi::Py_ssize_t), name) }
                {
                    return Ok(false);
                }
            }
            for (name, offset) in &self.slots {
                if unsafe { member_offset(class, name) } != Some(*offset) {
                    return Ok(false);
                }
            }
        } else {
            // The builder's state must be normal instance data. We do not read
            // it via getattr here: explicit frame arguments/transcripts prove
            // the generation operations that follow.
            let dictionary = unsafe { (*kind).tp_dict };
            for name in [
                "__slots__",
                "names",
                "src",
                "globals",
                "locals",
                "overwrite_errors",
                "unconditional_adds",
                "method_annotations",
            ] {
                if unsafe { dictionary_value(dictionary, name) }.is_some() {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    fn member<'py>(
        &self,
        py: Python<'py>,
        references: &impl References<'py>,
        object: &Bound<'py, PyAny>,
        shape: StructType,
        name: &str,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        let kind = unsafe { ffi::Py_TYPE(object.as_ptr()) }.cast::<ffi::PyObject>();
        if !self.matches(py, references, kind, shape)? {
            return Ok(None);
        }
        let Some((_, offset)) = self.slots.iter().find(|(slot, _)| *slot == name) else {
            return Ok(None);
        };
        let value = unsafe {
            *object
                .as_ptr()
                .cast::<u8>()
                .add(*offset)
                .cast::<*mut ffi::PyObject>()
        };
        Ok(if value.is_null() {
            None
        } else {
            Some(unsafe { Bound::from_borrowed_ptr(py, value) })
        })
    }
}

/// Only Rust values; all Python edges are supplied by the enclosing owner.
pub(super) struct HelperCatalog {
    functions: Vec<FunctionWitness>,
    templates: Vec<TemplateWitness>,
    edges: Vec<ResolvedEdge>,
    sentinels: Vec<WeakIdentity>,
    structures: Vec<StructWitness>,
    globals: usize,
    builtins: usize,
}

pub(super) struct CapturedCatalog<'py> {
    pub(super) catalog: HelperCatalog,
    pub(super) references: Vec<Bound<'py, PyAny>>,
}

impl HelperCatalog {
    /// Unknown initial graphs decline before any class contract exists. This
    /// cold operation can allocate, so its final step rechecks all witnesses.
    /// The caller must do the same immediately before the native root invoke.
    pub(super) fn capture<'py>(
        py: Python<'py>,
        root: &Bound<'py, PyAny>,
        recipes: &StdlibRecipes,
    ) -> PyResult<Option<CapturedCatalog<'py>>> {
        if unsafe { ffi::PyFunction_Check(root.as_ptr()) } == 0 {
            return Ok(None);
        }
        let raw = root.as_ptr().cast::<ffi::PyFunctionObject>();
        let globals = unsafe { (*raw).func_globals };
        let builtins = unsafe { (*raw).func_builtins };
        if unsafe { ffi::PyDict_CheckExact(globals) } == 0
            || unsafe { ffi::PyDict_CheckExact(builtins) } == 0
        {
            return Ok(None);
        }
        let mut references = Vec::new();
        let mut structures = Vec::new();
        for shape in StructType::ALL {
            let Some(class) = (unsafe { dictionary_value(globals, shape.name()) }) else {
                return Ok(None);
            };
            let class = unsafe { Bound::from_borrowed_ptr(py, class) };
            let Some(witness) = StructWitness::capture(py, &class, *shape, &mut references)? else {
                return Ok(None);
            };
            structures.push(witness);
        }
        let mut sentinels = Vec::new();
        for sentinel in Sentinel::ALL {
            let Some(value) = (unsafe { dictionary_value(globals, sentinel.name()) }) else {
                return Ok(None);
            };
            let Some(kind) = (unsafe { dictionary_value(globals, sentinel.class()) }) else {
                return Ok(None);
            };
            if unsafe { ffi::Py_TYPE(value) }.cast::<ffi::PyObject>() != kind
                || !unsafe { plain_object_type(kind) }
            {
                return Ok(None);
            }
            let value = unsafe { Bound::from_borrowed_ptr(py, value) };
            sentinels.push(WeakIdentity::capture(py, &value, &mut references)?);
        }
        let mut functions = Vec::new();
        // Keep actual functions pinned only during cold attestation. The final
        // catalog stores weak witnesses, never these temporary strong roots.
        let mut actual_functions = Vec::new();
        let mut module_filename = None;
        for helper in Helper::ALL {
            let actual = if *helper == Helper::Dataclass {
                root.as_ptr()
            } else {
                let Some(actual) = (unsafe { function_at_path(globals, helper.path()) }) else {
                    return Ok(None);
                };
                actual
            };
            if unsafe { ffi::PyFunction_Check(actual) } == 0 {
                return Ok(None);
            }
            let actual = unsafe { Bound::from_borrowed_ptr(py, actual) };
            let raw = actual.as_ptr().cast::<ffi::PyFunctionObject>();
            let actual_globals = unsafe { (*raw).func_globals };
            let actual_builtins = unsafe { (*raw).func_builtins };
            if (*helper != Helper::RecursiveRepr && actual_globals != globals)
                || actual_builtins != builtins
                || unsafe { ffi::PyDict_CheckExact(actual_globals) } == 0
            {
                return Ok(None);
            }
            let code = unsafe { Bound::from_borrowed_ptr(py, (*raw).func_code) };
            let Some(filename) = CodeRecipe::filename(py, &code)? else {
                return Ok(None);
            };
            if *helper != Helper::RecursiveRepr {
                if module_filename
                    .as_ref()
                    .is_some_and(|expected| expected != &filename)
                {
                    return Ok(None);
                }
                module_filename.get_or_insert_with(|| filename.clone());
            }
            let Some(recipe) = helper.recipe(recipes) else {
                return Ok(None);
            };
            if !recipe.matches(py, &code, &filename)? {
                return Ok(None);
            }
            functions.push(FunctionWitness {
                function: WeakIdentity::capture(py, &actual, &mut references)?,
                code: WeakIdentity::capture(py, &code, &mut references)?,
                recipe: recipe.clone(),
                globals: actual_globals as usize,
                builtins: actual_builtins as usize,
            });
            actual_functions.push(actual);
        }
        let mut templates = Vec::new();
        for template in Template::ALL {
            let parent = &functions[template.parent_helper() as usize];
            let Some(parent_code) = parent.code.upgrade(py, &references)? else {
                return Ok(None);
            };
            let Some(code) =
                parent
                    .recipe
                    .definition_code(py, &parent_code, template.qualname())?
            else {
                return Ok(None);
            };
            let Some(recipe) = parent.recipe.definition(template.qualname()) else {
                return Ok(None);
            };
            templates.push(TemplateWitness {
                code: WeakIdentity::capture(py, &code, &mut references)?,
                recipe: recipe.clone(),
            });
        }
        let mut catalog = Self {
            functions,
            templates,
            edges: Vec::new(),
            sentinels,
            structures,
            globals: globals as usize,
            builtins: builtins as usize,
        };
        for operation in Edge::ALL {
            let role = operation.producer();
            let Some(code) = catalog.code(py, &references, role)? else {
                return Ok(None);
            };
            let Some(offset) = catalog
                .recipe(role)
                .call_site(py, &code, operation.span())?
            else {
                return Ok(None);
            };
            if catalog.edge(role, offset).is_some() {
                return Ok(None);
            }
            catalog.edges.push(ResolvedEdge {
                operation: *operation,
                code_unit_offset: offset,
            });
        }
        if !catalog.validate(py, &references, root)? {
            return Ok(None);
        }
        drop(actual_functions);
        Ok(Some(CapturedCatalog {
            catalog,
            references,
        }))
    }

    pub(super) fn validate<'py>(
        &self,
        py: Python<'py>,
        references: &impl References<'py>,
        root: &Bound<'py, PyAny>,
    ) -> PyResult<bool> {
        if !self.matches_function(py, references, Helper::Dataclass, root.as_ptr())? {
            return Ok(false);
        }
        let globals = self.globals as *mut ffi::PyObject;
        for shape in StructType::ALL {
            let Some(class) = (unsafe { dictionary_value(globals, shape.name()) }) else {
                return Ok(false);
            };
            if !self.structures[*shape as usize].matches(py, references, class, *shape)? {
                return Ok(false);
            }
        }
        for sentinel in Sentinel::ALL {
            let Some(actual) = (unsafe { dictionary_value(globals, sentinel.name()) }) else {
                return Ok(false);
            };
            let Some(kind) = (unsafe { dictionary_value(globals, sentinel.class()) }) else {
                return Ok(false);
            };
            if !self.sentinels[*sentinel as usize].matches(py, references, actual)?
                || unsafe { ffi::Py_TYPE(actual) }.cast::<ffi::PyObject>() != kind
                || !unsafe { plain_object_type(kind) }
            {
                return Ok(false);
            }
        }
        for helper in Helper::ALL {
            if *helper == Helper::Dataclass {
                continue;
            }
            let Some(actual) = (unsafe { function_at_path(globals, helper.path()) }) else {
                return Ok(false);
            };
            if !self.matches_function(py, references, *helper, actual)? {
                return Ok(false);
            }
        }
        for (name, expected) in [
            ("__name__", "dataclasses"),
            ("_FIELDS", "__dataclass_fields__"),
            ("_PARAMS", "__dataclass_params__"),
            ("_POST_INIT_NAME", "__post_init__"),
        ] {
            if !unsafe {
                dictionary_value(globals, name).is_some_and(|value| text_is(value, expected))
            } {
                return Ok(false);
            }
        }
        if !self.validate_builtins(py, self.globals)?
            || !self
                .validate_builtins(py, self.functions[Helper::RecursiveRepr as usize].globals)?
            || !self.validate_hash_table(py, references)?
        {
            return Ok(false);
        }
        for (name, kind) in [
            ("_dataclass_record_source", 1),
            ("_dataclass_exec", 2),
            ("_dataclass_set_member", 3),
            ("_dataclass_new_slots", 8),
        ] {
            let expected = unsafe { PySoac_GetDataclassBuiltin(kind) };
            if expected.is_null() {
                if unsafe { !ffi::PyErr_Occurred().is_null() } {
                    return Err(PyErr::fetch(py));
                }
                return Ok(false);
            }
            if unsafe { dictionary_value(globals, name) } != Some(expected) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn recipe(&self, role: CodeRole) -> &CodeRecipe {
        match role {
            CodeRole::Helper(helper) => &self.functions[helper as usize].recipe,
            CodeRole::Template(template) => &self.templates[template as usize].recipe,
        }
    }

    pub(super) fn code<'py>(
        &self,
        py: Python<'py>,
        references: &impl References<'py>,
        role: CodeRole,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        let witness = match role {
            CodeRole::Helper(helper) => self.functions[helper as usize].code,
            CodeRole::Template(template) => self.templates[template as usize].code,
        };
        witness.upgrade(py, references)
    }

    /// Code is one part of a frame proof. The caller must also authenticate
    /// its actual function/environment/creation record and role operands.
    pub(super) fn matches_code<'py>(
        &self,
        py: Python<'py>,
        references: &impl References<'py>,
        role: CodeRole,
        actual: *mut ffi::PyObject,
    ) -> PyResult<bool> {
        let Some(expected) = self.code(py, references, role)? else {
            return Ok(false);
        };
        Ok(expected.as_ptr() == actual && self.recipe(role).matches_layout(py, actual)?)
    }

    pub(super) fn local_index(&self, role: CodeRole, name: &str) -> Option<usize> {
        self.recipe(role).local_index(name)
    }

    /// Only an operation selector. Matching a source span or call offset alone
    /// never authenticates a callee, member body, or invocation.
    pub(super) fn edge(&self, role: CodeRole, code_unit_offset: usize) -> Option<Edge> {
        self.edges.iter().find_map(|edge| {
            (edge.operation.producer() == role && edge.code_unit_offset == code_unit_offset)
                .then_some(edge.operation)
        })
    }

    pub(super) fn matches_function<'py>(
        &self,
        py: Python<'py>,
        references: &impl References<'py>,
        helper: Helper,
        actual: *mut ffi::PyObject,
    ) -> PyResult<bool> {
        if actual.is_null() || unsafe { ffi::PyFunction_Check(actual) } == 0 {
            return Ok(false);
        }
        // Semantic helper attestation is not source, generated-function, or
        // JIT ownership. A preexisting native owner must not be laundered into
        // this invocation by an otherwise equivalent code/environment graph.
        // Cleared/foreign native state remains an error, not an ordinary miss.
        let owner = unsafe { PyFunction_GetSoacStrictOwner(actual) };
        if unsafe { !ffi::PyErr_Occurred().is_null() } {
            return Err(PyErr::fetch(py));
        }
        if !owner.is_null()
            || unsafe { crate::PyFunction_GetSoacStrictId(actual) } != 0
            || unsafe { crate::PyFunction_GetSoacFunctionId(actual) } != 0
            || unsafe { !crate::PyFunction_GetSoacMetadata(actual).is_null() }
        {
            return Ok(false);
        }
        let witness = &self.functions[helper as usize];
        if !witness.function.matches(py, references, actual)? {
            return Ok(false);
        }
        let raw = actual.cast::<ffi::PyFunctionObject>();
        if unsafe { (*raw).func_globals } as usize != witness.globals
            || unsafe { (*raw).func_builtins } as usize != witness.builtins
            || !unsafe { (*raw).vectorcall }.is_some_and(|entry| {
                ptr::fn_addr_eq(entry, _PyFunction_Vectorcall as ffi::vectorcallfunc)
            })
            || !witness
                .code
                .matches(py, references, unsafe { (*raw).func_code })?
            || !witness
                .recipe
                .matches_layout(py, unsafe { (*raw).func_code })?
        {
            return Ok(false);
        }
        let closure = unsafe { (*raw).func_closure };
        if !closure.is_null()
            && (unsafe { ffi::PyTuple_CheckExact(closure) } == 0
                || unsafe { ffi::PyTuple_Size(closure) } != 0)
        {
            return Ok(false);
        }
        let defaults = unsafe { (*raw).func_defaults };
        let rules = helper.defaults();
        if defaults.is_null() {
            if !rules.is_empty() {
                return Ok(false);
            }
        } else {
            if unsafe { ffi::PyTuple_CheckExact(defaults) } == 0
                || unsafe { ffi::PyTuple_Size(defaults) } != rules.len() as ffi::Py_ssize_t
            {
                return Ok(false);
            }
            for (index, rule) in rules.iter().enumerate() {
                if !self.matches_value(py, references, *rule, unsafe {
                    ffi::PyTuple_GetItem(defaults, index as ffi::Py_ssize_t)
                })? {
                    return Ok(false);
                }
            }
        }
        let keywords = unsafe { (*raw).func_kwdefaults };
        let rules = helper.keyword_defaults();
        if keywords.is_null() {
            return Ok(rules.is_empty());
        }
        if unsafe { ffi::PyDict_CheckExact(keywords) } == 0
            || unsafe { ffi::PyDict_Size(keywords) } != rules.len() as ffi::Py_ssize_t
        {
            return Ok(false);
        }
        for (name, rule) in rules {
            let Some(actual) = (unsafe { dictionary_value(keywords, name) }) else {
                return Ok(false);
            };
            if !self.matches_value(py, references, *rule, actual)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn function<'py>(
        &self,
        py: Python<'py>,
        references: &impl References<'py>,
        helper: Helper,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        self.functions[helper as usize]
            .function
            .upgrade(py, references)
    }

    pub(super) fn matches_sentinel<'py>(
        &self,
        py: Python<'py>,
        references: &impl References<'py>,
        sentinel: Sentinel,
        actual: *mut ffi::PyObject,
    ) -> PyResult<bool> {
        self.sentinels[sentinel as usize].matches(py, references, actual)
    }

    pub(super) fn matches_structure<'py>(
        &self,
        py: Python<'py>,
        references: &impl References<'py>,
        shape: StructType,
        actual: *mut ffi::PyObject,
    ) -> PyResult<bool> {
        if actual.is_null() {
            return Ok(false);
        }
        self.structures[shape as usize].matches(
            py,
            references,
            unsafe { ffi::Py_TYPE(actual) }.cast(),
            shape,
        )
    }

    pub(super) fn structure<'py>(
        &self,
        py: Python<'py>,
        references: &impl References<'py>,
        shape: StructType,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        let witness = &self.structures[shape as usize];
        let Some(class) = witness.class.upgrade(py, references)? else {
            return Ok(None);
        };
        Ok(witness
            .matches(py, references, class.as_ptr(), shape)?
            .then_some(class))
    }

    fn matches_value<'py>(
        &self,
        py: Python<'py>,
        references: &impl References<'py>,
        rule: ValueRule,
        actual: *mut ffi::PyObject,
    ) -> PyResult<bool> {
        Ok(match rule {
            ValueRule::None => actual == unsafe { ffi::Py_None() },
            ValueRule::Bool(value) => {
                actual
                    == unsafe {
                        if value {
                            ffi::Py_True()
                        } else {
                            ffi::Py_False()
                        }
                    }
            }
            ValueRule::Text(value) => unsafe { text_is(actual, value) },
            ValueRule::Sentinel(value) => {
                self.sentinels[value as usize].matches(py, references, actual)?
            }
        })
    }

    pub(super) fn member<'py>(
        &self,
        py: Python<'py>,
        references: &impl References<'py>,
        object: &Bound<'py, PyAny>,
        shape: StructType,
        name: &str,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        self.structures[shape as usize].member(py, references, object, shape, name)
    }

    fn validate_builtins(&self, py: Python<'_>, globals: usize) -> PyResult<bool> {
        let is_reprlib = globals != self.globals;
        let globals = globals as *mut ffi::PyObject;
        let builtins = self.builtins as *mut ffi::PyObject;
        if !unsafe { exact_text_dictionary(globals) && exact_text_dictionary(builtins) } {
            return Ok(false);
        }
        let names = if is_reprlib {
            &["getattr", "id"][..]
        } else {
            &[
                "getattr",
                "setattr",
                "delattr",
                "hasattr",
                "isinstance",
                "len",
                "repr",
                "hash",
                "exec",
            ][..]
        };
        for name in names {
            let actual = unsafe {
                dictionary_value(globals, name).or_else(|| dictionary_value(builtins, name))
            };
            let Some(actual) = actual else {
                return Ok(false);
            };
            let matched = unsafe {
                PySoac_MatchesBuiltinFunction(
                    actual,
                    name.as_ptr().cast(),
                    name.len() as ffi::Py_ssize_t,
                )
            };
            if matched < 0 {
                return Err(PyErr::fetch(py));
            }
            if matched != 1 {
                return Ok(false);
            }
            // A semantic builtin copy can match ordinary helper behavior, but
            // the explicit exec/member bridges require the canonical object.
            let kind = match *name {
                "exec" => Some(4),
                "setattr" => Some(5),
                _ => None,
            };
            if let Some(kind) = kind {
                if unsafe { PySoac_GetDataclassBuiltin(kind) } != actual {
                    return Ok(false);
                }
            }
        }
        for (name, expected) in unsafe {
            [
                ("object", ptr::addr_of_mut!(ffi::PyBaseObject_Type)),
                ("type", ptr::addr_of_mut!(ffi::PyType_Type)),
                ("bool", ptr::addr_of_mut!(ffi::PyBool_Type)),
                ("str", ptr::addr_of_mut!(ffi::PyUnicode_Type)),
                ("tuple", ptr::addr_of_mut!(ffi::PyTuple_Type)),
                ("list", ptr::addr_of_mut!(ffi::PyList_Type)),
                ("dict", ptr::addr_of_mut!(ffi::PyDict_Type)),
                ("set", ptr::addr_of_mut!(ffi::PySet_Type)),
                ("super", ptr::addr_of_mut!(ffi::PySuper_Type)),
                ("zip", ptr::addr_of_mut!(ffi::PyZip_Type)),
                ("map", ptr::addr_of_mut!(ffi::PyMap_Type)),
                ("property", ptr::addr_of_mut!(ffi::PyProperty_Type)),
            ]
        } {
            if is_reprlib && name != "set" {
                continue;
            }
            if unsafe {
                dictionary_value(globals, name).or_else(|| dictionary_value(builtins, name))
            } != Some(expected.cast())
            {
                return Ok(false);
            }
        }
        for (name, expected) in unsafe {
            [
                ("TypeError", ffi::PyExc_TypeError),
                ("ValueError", ffi::PyExc_ValueError),
                ("AttributeError", ffi::PyExc_AttributeError),
                ("KeyError", ffi::PyExc_KeyError),
            ]
        } {
            if is_reprlib {
                continue;
            }
            if unsafe {
                dictionary_value(globals, name).or_else(|| dictionary_value(builtins, name))
            } != Some(expected)
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn validate_hash_table<'py>(
        &self,
        py: Python<'py>,
        references: &impl References<'py>,
    ) -> PyResult<bool> {
        let Some(table) =
            (unsafe { dictionary_value(self.globals as *mut ffi::PyObject, "_hash_action") })
        else {
            return Ok(false);
        };
        if unsafe { ffi::PyDict_CheckExact(table) } == 0 || unsafe { ffi::PyDict_Size(table) } != 16
        {
            return Ok(false);
        }
        let mut position = 0;
        let mut key = ptr::null_mut();
        let mut value = ptr::null_mut();
        let mut seen = 0u16;
        while unsafe { ffi::PyDict_Next(table, &mut position, &mut key, &mut value) } != 0 {
            if unsafe { ffi::PyTuple_CheckExact(key) } == 0
                || unsafe { ffi::PyTuple_Size(key) } != 4
            {
                return Ok(false);
            }
            let mut flags = [false; 4];
            for (index, flag) in flags.iter_mut().enumerate() {
                let value = unsafe { ffi::PyTuple_GetItem(key, index as ffi::Py_ssize_t) };
                if unsafe { ffi::PyBool_Check(value) } == 0 {
                    return Ok(false);
                }
                *flag = value == unsafe { ffi::Py_True() };
            }
            let index = flags
                .iter()
                .fold(0usize, |index, flag| (index << 1) | usize::from(*flag));
            if seen & (1 << index) != 0 {
                return Ok(false);
            }
            seen |= 1 << index;
            let [unsafe_hash, eq, frozen, explicit] = flags;
            let expected = if unsafe_hash {
                Some(if explicit {
                    Helper::HashError
                } else {
                    Helper::HashAdd
                })
            } else if !explicit && eq {
                Some(if frozen {
                    Helper::HashAdd
                } else {
                    Helper::HashNone
                })
            } else {
                None
            };
            if let Some(expected) = expected {
                if !self.functions[expected as usize]
                    .function
                    .matches(py, references, value)?
                {
                    return Ok(false);
                }
            } else if value != unsafe { ffi::Py_None() } {
                return Ok(false);
            }
        }
        Ok(seen == u16::MAX)
    }
}

unsafe fn function_at_path(globals: *mut ffi::PyObject, path: &str) -> Option<*mut ffi::PyObject> {
    if let Some((class, member)) = path.split_once('.') {
        let class = unsafe { dictionary_value(globals, class)? };
        if unsafe { ffi::Py_TYPE(class) } != ptr::addr_of_mut!(ffi::PyType_Type) {
            return None;
        }
        unsafe { dictionary_value((*class.cast::<ffi::PyTypeObject>()).tp_dict, member) }
    } else {
        unsafe { dictionary_value(globals, path) }
    }
}

/// Sentinel classes use identity semantics. Their diagnostic repr is an
/// ordinary leaf, but custom equality/truth/attribute hooks cannot influence
/// producer decisions before the next privileged boundary.
unsafe fn plain_object_type(class: *mut ffi::PyObject) -> bool {
    if class.is_null() || unsafe { ffi::Py_TYPE(class) } != ptr::addr_of_mut!(ffi::PyType_Type) {
        return false;
    }
    let kind = class.cast::<ffi::PyTypeObject>();
    if unsafe { (*kind).tp_base } != ptr::addr_of_mut!(ffi::PyBaseObject_Type)
        || !unsafe { (*kind).tp_getattro }.is_some_and(|slot| {
            ptr::fn_addr_eq(slot, ffi::PyObject_GenericGetAttr as ffi::getattrofunc)
        })
        || !unsafe { (*kind).tp_setattro }.is_some_and(|slot| {
            ptr::fn_addr_eq(slot, ffi::PyObject_GenericSetAttr as ffi::setattrofunc)
        })
    {
        return false;
    }
    let dictionary = unsafe { (*kind).tp_dict };
    let bases = unsafe { (*kind).tp_bases };
    let mro = unsafe { (*kind).tp_mro };
    if !unsafe { exact_text_dictionary(dictionary) }
        || bases.is_null()
        || unsafe { ffi::PyTuple_CheckExact(bases) } == 0
        || unsafe { ffi::PyTuple_Size(bases) } != 1
        || unsafe { ffi::PyTuple_GetItem(bases, 0) }
            != ptr::addr_of_mut!(ffi::PyBaseObject_Type).cast()
        || mro.is_null()
        || unsafe { ffi::PyTuple_CheckExact(mro) } == 0
        || unsafe { ffi::PyTuple_Size(mro) } != 2
        || unsafe { ffi::PyTuple_GetItem(mro, 0) } != class
        || unsafe { ffi::PyTuple_GetItem(mro, 1) }
            != ptr::addr_of_mut!(ffi::PyBaseObject_Type).cast()
    {
        return false;
    }
    for name in [
        "__new__",
        "__eq__",
        "__ne__",
        "__bool__",
        "__len__",
        "__getattr__",
        "__getattribute__",
        "__setattr__",
        "__delattr__",
    ] {
        if unsafe { dictionary_value(dictionary, name) }.is_some() {
            return false;
        }
    }
    true
}

unsafe fn member_offset(class: *mut ffi::PyObject, name: &str) -> Option<usize> {
    let kind = class.cast::<ffi::PyTypeObject>();
    let descriptor = unsafe { dictionary_value((*kind).tp_dict, name)? };
    if unsafe { ffi::Py_TYPE(descriptor) } != ptr::addr_of_mut!(ffi::PyMemberDescr_Type) {
        return None;
    }
    let descriptor = descriptor.cast::<ffi::PyMemberDescrObject>();
    let member = unsafe { (*descriptor).d_member };
    if unsafe { (*descriptor).d_common.d_type } != kind
        || member.is_null()
        || !unsafe { text_is((*descriptor).d_common.d_name, name) }
        || unsafe { (*member).type_code } != ffi::Py_T_OBJECT_EX
        || unsafe { (*member).flags } != 0
    {
        return None;
    }
    let offset = usize::try_from(unsafe { (*member).offset }).ok()?;
    let size = usize::try_from(unsafe { (*kind).tp_basicsize }).ok()?;
    if offset < std::mem::size_of::<ffi::PyObject>()
        || offset % std::mem::align_of::<*mut ffi::PyObject>() != 0
        || offset.checked_add(std::mem::size_of::<*mut ffi::PyObject>())? > size
    {
        return None;
    }
    Some(offset)
}

/// FrozenInstanceError is the ordinary empty AttributeError subclass. It is
/// not made immutable; each privileged use must recheck that no custom
/// construction, attribute, display, or finalization hook was introduced.
unsafe fn plain_exception_type(class: *mut ffi::PyObject) -> bool {
    if class.is_null() || unsafe { ffi::Py_TYPE(class) } != ptr::addr_of_mut!(ffi::PyType_Type) {
        return false;
    }
    let kind = class.cast::<ffi::PyTypeObject>();
    let base = unsafe { ffi::PyExc_AttributeError }.cast::<ffi::PyTypeObject>();
    if unsafe { (*kind).tp_base } != base {
        return false;
    }
    let bases = unsafe { (*kind).tp_bases };
    let mro = unsafe { (*kind).tp_mro };
    let base_mro = unsafe { (*base).tp_mro };
    if bases.is_null()
        || mro.is_null()
        || base_mro.is_null()
        || unsafe { ffi::PyTuple_CheckExact(bases) } == 0
        || unsafe { ffi::PyTuple_Size(bases) } != 1
        || unsafe { ffi::PyTuple_GetItem(bases, 0) } != base.cast()
        || unsafe { ffi::PyTuple_CheckExact(mro) } == 0
        || unsafe { ffi::PyTuple_Size(mro) } != unsafe { ffi::PyTuple_Size(base_mro) } + 1
        || unsafe { ffi::PyTuple_GetItem(mro, 0) } != class
    {
        return false;
    }
    for index in 0..unsafe { ffi::PyTuple_Size(base_mro) } {
        if unsafe { ffi::PyTuple_GetItem(mro, index + 1) }
            != unsafe { ffi::PyTuple_GetItem(base_mro, index) }
        {
            return false;
        }
    }
    macro_rules! same_slot {
        ($slot:ident) => {
            match (unsafe { (*kind).$slot }, unsafe { (*base).$slot }) {
                (Some(actual), Some(expected)) => ptr::fn_addr_eq(actual, expected),
                (None, None) => true,
                _ => false,
            }
        };
    }
    if !same_slot!(tp_new)
        || !same_slot!(tp_init)
        || !same_slot!(tp_getattro)
        || !same_slot!(tp_setattro)
        || !same_slot!(tp_repr)
        || !same_slot!(tp_str)
        || !same_slot!(tp_finalize)
    {
        return false;
    }
    let dictionary = unsafe { (*kind).tp_dict };
    if !unsafe { exact_text_dictionary(dictionary) } {
        return false;
    }
    for name in [
        "__new__",
        "__init__",
        "__call__",
        "__repr__",
        "__str__",
        "__getattr__",
        "__getattribute__",
        "__setattr__",
        "__delattr__",
        "__del__",
        "__slots__",
    ] {
        if unsafe { dictionary_value(dictionary, name) }.is_some() {
            return false;
        }
    }
    true
}

unsafe fn struct_slots(
    class: *mut ffi::PyObject,
    shape: StructType,
) -> Option<Vec<(&'static str, usize)>> {
    if matches!(shape, StructType::FrozenError) {
        return unsafe { plain_exception_type(class) }.then(Vec::new);
    }
    if !unsafe { plain_object_type(class) } {
        return None;
    }
    let Some(names) = shape.slots() else {
        return Some(Vec::new());
    };
    let mut slots = Vec::with_capacity(names.len());
    for name in names {
        let offset = unsafe { member_offset(class, name)? };
        if slots.iter().any(|(_, previous)| *previous == offset) {
            return None;
        }
        slots.push((*name, offset));
    }
    Some(slots)
}

#[cfg(test)]
mod tests {
    use pyo3::types::{PyDict, PyModule};

    use super::*;

    fn root<'py>(py: Python<'py>) -> Bound<'py, PyAny> {
        PyModule::import(py, "dataclasses")
            .unwrap()
            .getattr("dataclass")
            .unwrap()
    }

    fn copy_function<'py>(
        py: Python<'py>,
        original: &Bound<'py, PyAny>,
        globals: Option<&Bound<'py, PyDict>>,
    ) -> Bound<'py, PyAny> {
        let raw = original.as_ptr().cast::<ffi::PyFunctionObject>();
        let function = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                ffi::PyFunction_New(
                    (*raw).func_code,
                    globals.map_or((*raw).func_globals, |globals| globals.as_ptr()),
                ),
            )
        }
        .unwrap();
        unsafe {
            if !(*raw).func_defaults.is_null() {
                assert_eq!(
                    ffi::PyFunction_SetDefaults(function.as_ptr(), (*raw).func_defaults),
                    0
                );
            }
            if !(*raw).func_kwdefaults.is_null() {
                let keywords = Bound::<PyAny>::from_owned_ptr_or_err(
                    py,
                    ffi::PyDict_Copy((*raw).func_kwdefaults),
                )
                .unwrap();
                assert_eq!(
                    ffi::PyFunction_SetKwDefaults(function.as_ptr(), keywords.as_ptr()),
                    0
                );
            }
        }
        function
    }

    #[test]
    fn dataclass_catalog_attests_actual_helpers_and_equivalent_precatalog_function_copies() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let recipes = StdlibRecipes::load(py).unwrap();
            let original = root(py);
            let copied = copy_function(py, &original, None);
            for function in [&original, &copied] {
                let actual = HelperCatalog::capture(py, function, &recipes)
                    .unwrap()
                    .unwrap();
                assert!(
                    actual
                        .catalog
                        .validate(py, &actual.references, function)
                        .unwrap()
                );
            }
            // Body equivalence grants only this catalog's helper role. A
            // different actual function cannot replay a prior live witness.
            let actual = HelperCatalog::capture(py, &original, &recipes)
                .unwrap()
                .unwrap();
            assert!(
                !actual
                    .catalog
                    .matches_function(py, &actual.references, Helper::Dataclass, copied.as_ptr())
                    .unwrap()
            );
        });
    }

    #[test]
    fn dataclass_catalog_resolves_privileged_edges_against_actual_executed_code() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let recipes = StdlibRecipes::load(py).unwrap();
            let root = root(py);
            let captured = HelperCatalog::capture(py, &root, &recipes)
                .unwrap()
                .unwrap();
            for edge in &captured.catalog.edges {
                let role = edge.operation.producer();
                let code = captured
                    .catalog
                    .code(py, &captured.references, role)
                    .unwrap()
                    .unwrap();
                assert!(
                    captured
                        .catalog
                        .matches_code(py, &captured.references, role, code.as_ptr())
                        .unwrap()
                );
                assert_eq!(
                    captured.catalog.edge(role, edge.code_unit_offset),
                    Some(edge.operation)
                );
                assert_eq!(
                    captured
                        .catalog
                        .recipe(role)
                        .call_site(py, &code, edge.operation.span())
                        .unwrap(),
                    Some(edge.code_unit_offset)
                );
                assert_eq!(
                    super::super::code::compiled_call_site(
                        py,
                        code.as_ptr(),
                        edge.operation.span()
                    )
                    .unwrap(),
                    Some(edge.code_unit_offset)
                );
                // A copied code object has the same span/layout but cannot
                // impersonate the actual captured frame/code witness.
                let copy = code.call_method0("replace").unwrap();
                assert!(
                    !captured
                        .catalog
                        .matches_code(py, &captured.references, role, copy.as_ptr())
                        .unwrap()
                );
            }
            let wrapper = CodeRole::Template(Template::DataclassWrapper);
            let annotation = CodeRole::Template(Template::AnnotationProvider);
            assert!(captured.catalog.local_index(wrapper, "cls").is_some());
            assert!(
                captured
                    .catalog
                    .local_index(annotation, "__class__")
                    .is_some()
            );
            assert_eq!(captured.catalog.edge(wrapper, usize::MAX), None);
        });
    }

    #[test]
    fn dataclass_catalog_declines_changed_globals_defaults_or_native_entry() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let recipes = StdlibRecipes::load(py).unwrap();
            let original = root(py);
            let globals = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(
                    py,
                    ffi::PyDict_Copy(
                        (*original.as_ptr().cast::<ffi::PyFunctionObject>()).func_globals,
                    ),
                )
            }
            .unwrap()
            .cast_into::<PyDict>()
            .unwrap();
            let foreign_environment = copy_function(py, &original, Some(&globals));
            assert!(
                HelperCatalog::capture(py, &foreign_environment, &recipes)
                    .unwrap()
                    .is_none()
            );

            let changed_defaults = copy_function(py, &original, None);
            changed_defaults
                .getattr("__kwdefaults__")
                .unwrap()
                .set_item("frozen", true)
                .unwrap();
            assert!(
                HelperCatalog::capture(py, &changed_defaults, &recipes)
                    .unwrap()
                    .is_none()
            );

            unsafe extern "C" fn alternative_entry(
                _: *mut ffi::PyObject,
                _: *const *mut ffi::PyObject,
                _: usize,
                _: *mut ffi::PyObject,
            ) -> *mut ffi::PyObject {
                unsafe { ffi::Py_NewRef(ffi::Py_None()) }
            }
            let changed_entry = copy_function(py, &original, None);
            unsafe {
                crate::PyFunction_SetVectorcall(
                    changed_entry.as_ptr().cast(),
                    Some(alternative_entry),
                )
            };
            assert!(
                HelperCatalog::capture(py, &changed_entry, &recipes)
                    .unwrap()
                    .is_none()
            );
        });
    }

    #[test]
    fn dataclass_catalog_declines_precatalog_native_owners_and_postcapture_seals() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            unsafe extern "C" {
                fn PyFunction_SetSoacStrictOwner(
                    function: *mut ffi::PyObject,
                    owner: *mut ffi::PyObject,
                ) -> c_int;
                fn PyFunction_SealSoacStrict(function: *mut ffi::PyObject, identity: u64) -> c_int;
            }

            let recipes = StdlibRecipes::load(py).unwrap();
            let original = root(py);
            let owned = copy_function(py, &original, None);
            let captured = HelperCatalog::capture(py, &owned, &recipes)
                .unwrap()
                .unwrap();
            let unrelated_owner = PyDict::new(py);
            assert_eq!(
                unsafe { PyFunction_SetSoacStrictOwner(owned.as_ptr(), unrelated_owner.as_ptr()) },
                0
            );
            assert!(
                HelperCatalog::capture(py, &owned, &recipes)
                    .unwrap()
                    .is_none()
            );
            assert!(
                !captured
                    .catalog
                    .validate(py, &captured.references, &owned)
                    .unwrap()
            );

            // A seal with no source owner is still not an ordinary shared
            // helper. Native metadata cannot acquire invocation authority from
            // matching code, defaults, globals, or a restored public entry.
            let sealed = copy_function(py, &original, None);
            let captured = HelperCatalog::capture(py, &sealed, &recipes)
                .unwrap()
                .unwrap();
            assert_eq!(unsafe { PyFunction_SealSoacStrict(sealed.as_ptr(), 1) }, 0);
            assert!(
                HelperCatalog::capture(py, &sealed, &recipes)
                    .unwrap()
                    .is_none()
            );
            assert!(
                !captured
                    .catalog
                    .validate(py, &captured.references, &sealed)
                    .unwrap()
            );
            assert!(
                HelperCatalog::capture(py, &original, &recipes)
                    .unwrap()
                    .is_some()
            );
        });
    }

    #[test]
    fn dataclass_catalog_rechecks_actual_helper_identity_after_initial_attestation() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let recipes = StdlibRecipes::load(py).unwrap();
            let root = root(py);
            let actual = HelperCatalog::capture(py, &root, &recipes)
                .unwrap()
                .unwrap();
            let globals = root
                .getattr("__globals__")
                .unwrap()
                .cast_into::<PyDict>()
                .unwrap();
            let previous = globals.get_item("_field_init").unwrap().unwrap();
            let replacement = copy_function(py, &previous, None);
            globals.set_item("_field_init", &replacement).unwrap();
            let matched = actual.catalog.validate(py, &actual.references, &root);
            globals.set_item("_field_init", previous).unwrap();
            assert!(!matched.unwrap());
            assert!(
                actual
                    .catalog
                    .validate(py, &actual.references, &root)
                    .unwrap()
            );
        });
    }

    #[test]
    fn dataclass_catalog_rejects_custom_dictionary_keys_without_comparing_or_hashing_them() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let recipes = StdlibRecipes::load(py).unwrap();
            let function = copy_function(py, &root(py), None);
            let module = PyModule::from_code(py, c"events = []\nclass Key:\n def __hash__(self):\n  events.append('hash')\n  return 0\n def __eq__(self, other):\n  events.append('eq')\n  return True\nkey = Key()\n", c"<catalog hostile key>", c"catalog_hostile_key").unwrap();
            let keywords = function.getattr("__kwdefaults__").unwrap();
            keywords
                .set_item(module.getattr("key").unwrap(), true)
                .unwrap();
            module
                .getattr("events")
                .unwrap()
                .call_method0("clear")
                .unwrap();
            assert!(
                HelperCatalog::capture(py, &function, &recipes)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(module.getattr("events").unwrap().len().unwrap(), 0);
        });
    }

    #[test]
    fn dataclass_catalog_does_not_retain_the_actual_root_function() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let recipes = StdlibRecipes::load(py).unwrap();
            let original = root(py);
            let function = copy_function(py, &original, None);
            let actual = HelperCatalog::capture(py, &function, &recipes)
                .unwrap()
                .unwrap();
            let weak = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(
                    py,
                    ffi::PyWeakref_NewRef(function.as_ptr(), ptr::null_mut()),
                )
            }
            .unwrap();
            drop(function);
            let mut value = ptr::null_mut();
            assert_eq!(
                unsafe { ffi::PyWeakref_GetRef(weak.as_ptr(), &mut value) },
                0
            );
            assert!(value.is_null());
            assert!(
                !actual
                    .catalog
                    .validate(py, &actual.references, &original)
                    .unwrap()
            );
        });
    }

    #[test]
    fn dataclass_catalog_reads_exact_field_slots_and_rejects_descriptor_replacement() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let recipes = StdlibRecipes::load(py).unwrap();
            let root = root(py);
            let actual = HelperCatalog::capture(py, &root, &recipes)
                .unwrap()
                .unwrap();
            let module = PyModule::import(py, "dataclasses").unwrap();
            let kwargs = PyDict::new(py);
            kwargs.set_item("default", 42).unwrap();
            let field = module
                .getattr("field")
                .unwrap()
                .call((), Some(&kwargs))
                .unwrap();
            assert_eq!(
                actual
                    .catalog
                    .member(py, &actual.references, &field, StructType::Field, "default")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap(),
                42
            );
            let field_type = module.getattr("Field").unwrap();
            let previous = field_type.getattr("default").unwrap();
            let trap = PyModule::from_code(py, c"events = []\ndef get(self):\n events.append('get')\n return 42\nreplacement = property(get)\n", c"<catalog field descriptor>", c"catalog_field_descriptor").unwrap();
            field_type
                .setattr("default", trap.getattr("replacement").unwrap())
                .unwrap();
            let member =
                actual
                    .catalog
                    .member(py, &actual.references, &field, StructType::Field, "default");
            field_type.setattr("default", previous).unwrap();
            assert!(member.unwrap().is_none());
            assert_eq!(trap.getattr("events").unwrap().len().unwrap(), 0);
            assert!(
                actual
                    .catalog
                    .validate(py, &actual.references, &root)
                    .unwrap()
            );
        });
    }

    #[test]
    fn dataclass_catalog_rechecks_frozen_error_construction_without_calling_it() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let recipes = StdlibRecipes::load(py).unwrap();
            let root = root(py);
            let captured = HelperCatalog::capture(py, &root, &recipes)
                .unwrap()
                .unwrap();
            let error = PyModule::import(py, "dataclasses")
                .unwrap()
                .getattr("FrozenInstanceError")
                .unwrap();
            let trap = PyModule::from_code(
                py,
                c"events = []\ndef initialize(self, *args):\n events.append('init')\n",
                c"<frozen error catalog>",
                c"frozen_error_catalog",
            )
            .unwrap();
            error
                .setattr("__init__", trap.getattr("initialize").unwrap())
                .unwrap();
            let matched = captured.catalog.validate(py, &captured.references, &root);
            error.delattr("__init__").unwrap();
            assert!(!matched.unwrap());
            assert_eq!(trap.getattr("events").unwrap().len().unwrap(), 0);
            assert!(
                captured
                    .catalog
                    .validate(py, &captured.references, &root)
                    .unwrap()
            );
        });
    }
}
