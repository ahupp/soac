//! One-use, GC-visible cells for an explicit class-construction helper.
//!
//! Only compiler operands from the authenticated active source frame can mint
//! a carrier. Public closure/default/argument values are never its authority.

use super::*;
use soac_contracts::{ClassTypeFact, NominalBindingOwner};
use soac_core::block_py::ClassConstructionScope;

pub(crate) const DISCARD_CLASS_CONSTRUCTION_CAPTURES_SYMBOL: &str =
    "soac_jit_discard_class_construction_captures";

pub(super) enum CaptureState {
    Absent,
    Uninstalled(ClassConstructionScope),
    Ready(InstalledCaptures),
    Consumed,
}

impl CaptureState {
    pub(super) fn initial(scope: Option<&ClassConstructionScope>) -> Self {
        scope.cloned().map_or(Self::Absent, Self::Uninstalled)
    }
}

pub(super) struct InstalledCaptures {
    namespace_witness: usize,
    namespace_owner: usize,
    namespace_function: RuntimeFunctionId,
    cells: Vec<(Vec<NominalBindingFact>, usize)>,
}

/// Temporary owning views, captured before helper CREATE observers. Nothing
/// here is installed if ordinary metadata setup or JIT registration fails.
pub(crate) struct PreparedClassConstructionCaptures<'py> {
    projection: ClassConstructionScope,
    helper: RuntimeFunctionId,
    definition: SourceIdentity,
    shared: usize,
    template: usize,
    namespace_witness: Bound<'py, PyAny>,
    namespace_owner: usize,
    cells: Vec<(Vec<NominalBindingFact>, Bound<'py, PyAny>)>,
    creation: Option<Arc<crate::strict_namespace::NamespaceExecution>>,
}

pub(super) fn same_creation(
    left: Option<&Arc<crate::strict_namespace::NamespaceExecution>>,
    right: Option<&Arc<crate::strict_namespace::NamespaceExecution>>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => Arc::ptr_eq(left, right),
        _ => false,
    }
}

fn is_cell(value: &Bound<'_, PyAny>) -> bool {
    unsafe extern "C" {
        static mut PyCell_Type: ffi::PyTypeObject;
    }
    unsafe { ffi::Py_TYPE(value.as_ptr()) == ptr::addr_of_mut!(PyCell_Type) }
}

/// The caller supplies only cells read by explicit CellRef operands from the
/// current compiled/interpreted body. The catalogue separately validates those
/// locations against the producer and this complete signed projection.
pub(crate) fn prepare_class_construction_captures<'py>(
    py: Python<'py>,
    shared: &Arc<SharedModuleState>,
    template: &Arc<FunctionInstantiationTemplate>,
    active: Option<&StrictFunctionCall>,
    namespace: Option<&Bound<'_, PyAny>>,
    cells: &[Bound<'_, PyAny>],
) -> PyResult<Option<PreparedClassConstructionCaptures<'py>>> {
    let function = template.function();
    let Some(projection) = &function.scope.class_construction else {
        if namespace.is_some() || !cells.is_empty() {
            return Err(strict_runtime_unavailable(
                py,
                "unexpected private construction operands",
            ));
        }
        return Ok(None);
    };
    let source = function
        .scope
        .source_origin
        .as_ref()
        .filter(|origin| origin.role == CallableSourceRole::ClassConstruction)
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "private captures require a class-construction template")
        })?;
    let active = active.ok_or_else(|| {
        strict_runtime_unavailable(py, "private captures have no active source frame")
    })?;
    let producer = active.captured_owner(py)?;
    let namespace = namespace.ok_or_else(|| {
        strict_runtime_unavailable(py, "private captures have no original namespace function")
    })?;
    if !shared.admits_function(function)
        || !active
            .active_module_state()
            .is_some_and(|actual| Arc::ptr_eq(actual, shared))
        || producer.source() != Some(&projection.producer)
        || producer.data().function_identity != active.function() as usize
        || cells.len() != projection.captures.len()
        || cells.is_empty()
        || cells.iter().any(|cell| !is_cell(cell))
    {
        return Err(strict_runtime_unavailable(
            py,
            "private capture producer or original cell layout changed",
        ));
    }
    let verified = shared.verified_strict_module().ok_or_else(|| {
        strict_runtime_unavailable(py, "private captures require verified source")
    })?;
    let namespace_projection = shared
        .lookup_function(projection.namespace_function)
        .and_then(|function| function.scope.private_lexical.as_ref());
    // Pin original cell identities before weakref allocation or any later
    // code-factory/audit/CREATE callback. Never read cell contents here.
    let mut selected = Vec::with_capacity(cells.len());
    let mut seen = BTreeSet::new();
    for (slot, cell) in projection.captures.iter().zip(cells) {
        let mut leaves = Vec::with_capacity(slot.nominal_binding_indices.len());
        for &index in &slot.nominal_binding_indices {
            let leaf = super::private_lexical::required_leaf(
                verified.type_facts().facts(),
                index,
                &slot.binding,
            )
            .filter(|leaf| {
                matches!(&leaf.owner, NominalBindingOwner::Field { field }
                            if field.declaring_class.definition == source.definition)
                    || namespace_projection.is_some_and(|scope| {
                        scope.private_captures().any(|capture| {
                            capture.binding == slot.binding
                                && capture.nominal_binding_indices.contains(&index)
                        })
                    })
            })
            .ok_or_else(|| {
                strict_runtime_unavailable(py, "private capture lost its exact signed field leaf")
            })?;
            if !seen.insert(leaf.clone()) {
                return Err(strict_runtime_unavailable(
                    py,
                    "private capture repeats a signed field leaf",
                ));
            }
            leaves.push(leaf.clone());
        }
        if leaves.is_empty() {
            return Err(strict_runtime_unavailable(
                py,
                "private cell has no signed field leaf",
            ));
        }
        selected.push((leaves, unsafe {
            Bound::from_borrowed_ptr(py, cell.as_ptr())
        }));
    }
    let namespace = authenticate_strict_function(py, namespace)?.ok_or_else(|| {
        strict_runtime_unavailable(py, "construction namespace has no native owner")
    })?;
    if namespace.function_id()? != projection.namespace_function
        || !Arc::ptr_eq(namespace.module_state()?, shared)
        || namespace.origin().is_none_or(|origin| {
            origin.role != CallableSourceRole::ClassNamespace
                || origin.definition != source.definition
        })
        || namespace.globals()?.as_ptr() != producer.global_dictionary()?.as_ptr()
        || !same_creation(
            namespace.creation_execution(),
            active.environment().namespace_execution.as_ref(),
        )
    {
        return Err(strict_runtime_unavailable(
            py,
            "private capture namespace belongs to another creation",
        ));
    }
    let witness = unsafe {
        Bound::from_owned_ptr_or_err(
            py,
            crate::PyWeakref_NewRef(namespace.function().as_ptr(), ptr::null_mut()),
        )?
    };
    Ok(Some(PreparedClassConstructionCaptures {
        projection: projection.clone(),
        helper: function.function_id,
        definition: source.definition.clone(),
        shared: Arc::as_ptr(shared) as usize,
        template: Arc::as_ptr(template) as usize,
        namespace_witness: witness,
        namespace_owner: namespace.owner().as_ptr() as usize,
        cells: selected,
        creation: active.environment().namespace_execution.clone(),
    }))
}

/// Installed only after ordinary metadata setup and registration succeed.
/// An explicit guard still clears every new edge if later publication fails.
pub(crate) struct ClassCaptureInstallationGuard<'py> {
    owner: StrictStateRef<'py, StrictFunctionData>,
    committed: bool,
}

impl ClassCaptureInstallationGuard<'_> {
    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for ClassCaptureInstallationGuard<'_> {
    fn drop(&mut self) {
        if !self.committed {
            let _ = discard_owner_captures(&self.owner);
        }
    }
}

impl<'py> PreparedClassConstructionCaptures<'py> {
    pub(crate) fn install(
        self,
        function: &Bound<'py, PyAny>,
    ) -> PyResult<ClassCaptureInstallationGuard<'py>> {
        let py = function.py();
        let auth = authenticate_strict_function(py, function)?
            .ok_or_else(|| strict_runtime_unavailable(py, "created helper has no native owner"))?;
        if auth.function_id()? != self.helper
            || auth.soac_implementation()?.shared_state_identity != self.shared
            || auth.soac_implementation()?.template_identity != self.template
            || auth.origin().is_none_or(|origin| {
                origin.role != CallableSourceRole::ClassConstruction
                    || origin.definition != self.definition
            })
            || !same_creation(auth.creation_execution(), self.creation.as_ref())
            || !matches!(&*auth.soac_implementation()?.class_captures.borrow(), CaptureState::Uninstalled(scope) if scope == &self.projection)
        {
            return Err(strict_runtime_unavailable(
                py,
                "created helper changed its private capture installation identity",
            ));
        }
        let mut namespace = ptr::null_mut();
        if unsafe { crate::PyWeakref_GetRef(self.namespace_witness.as_ptr(), &mut namespace) } < 0 {
            return Err(PyErr::fetch(py));
        }
        if namespace.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "original class namespace was released during helper creation",
            ));
        }
        let namespace = unsafe { Bound::from_owned_ptr(py, namespace) };
        let namespace_auth = authenticate_strict_function(py, &namespace)?.ok_or_else(|| {
            strict_runtime_unavailable(
                py,
                "original class namespace changed during helper creation",
            )
        })?;
        if namespace_auth.owner().as_ptr() as usize != self.namespace_owner
            || namespace_auth.function_id()? != self.projection.namespace_function
        {
            return Err(strict_runtime_unavailable(
                py,
                "original namespace birth changed during helper creation",
            ));
        }
        let namespace_witness = auth.add_reference(self.namespace_witness)?;
        let mut installed = InstalledCaptures {
            namespace_witness,
            namespace_owner: self.namespace_owner,
            namespace_function: self.projection.namespace_function,
            cells: Vec::with_capacity(self.cells.len()),
        };
        // No Python callback occurs between these GC-vector appends and the
        // phase publication. The temporary views pin all cells until then.
        for (leaves, cell) in self.cells {
            installed.cells.push((leaves, auth.add_reference(cell)?));
        }
        *auth.soac_implementation()?.class_captures.borrow_mut() = CaptureState::Ready(installed);
        Ok(ClassCaptureInstallationGuard {
            owner: auth.owner,
            committed: false,
        })
    }
}

fn clear_installed(
    owner: &StrictStateRef<'_, StrictFunctionData>,
    installed: &InstalledCaptures,
) -> PyResult<()> {
    let py = owner.owner().py();
    let mut result = owner.set_reference(installed.namespace_witness, py.None().into_bound(py));
    for (_, index) in &installed.cells {
        let cleared = owner.set_reference(*index, py.None().into_bound(py));
        if result.is_ok() {
            result = cleared;
        }
    }
    result
}

fn discard_owner_captures(owner: &StrictStateRef<'_, StrictFunctionData>) -> PyResult<()> {
    let state = std::mem::replace(
        &mut *owner.soac_implementation()?.class_captures.borrow_mut(),
        CaptureState::Consumed,
    );
    if matches!(state, CaptureState::Absent) {
        *owner.soac_implementation()?.class_captures.borrow_mut() = CaptureState::Absent;
        return Ok(());
    }
    if let CaptureState::Ready(installed) = state {
        clear_installed(owner, &installed)?;
    }
    Ok(())
}

pub(crate) fn take_class_construction_captures<'py>(
    py: Python<'py>,
    active: &StrictFunctionCall,
    actual_namespace: &AuthenticatedStrictFunction<'_, 'py>,
    fact: &ClassTypeFact,
) -> PyResult<Option<ClassConstructionCaptures<'py>>> {
    let owner = active.captured_owner(py)?;
    let namespace_module = actual_namespace.module_state()?;
    if owner.source().is_none_or(|origin| {
        origin.role != CallableSourceRole::ClassConstruction || origin.definition != fact.identity
    }) || !active
        .active_module_state()
        .is_some_and(|shared| Arc::ptr_eq(shared, namespace_module))
        || actual_namespace.origin().is_none_or(|origin| {
            origin.role != CallableSourceRole::ClassNamespace || origin.definition != fact.identity
        })
        || owner.data().function_identity != active.function() as usize
    {
        return Err(strict_runtime_unavailable(
            py,
            "class capture consumption has no actual helper/namespace owner",
        ));
    }
    let state = std::mem::replace(
        &mut *owner.soac_implementation()?.class_captures.borrow_mut(),
        CaptureState::Consumed,
    );
    let installed = match state {
        CaptureState::Absent => {
            // No selected cells grants no new authority; ordinary no-capture
            // helper invocations retain their existing construction behavior.
            *owner.soac_implementation()?.class_captures.borrow_mut() = CaptureState::Absent;
            return Ok(None);
        }
        CaptureState::Uninstalled(_) => {
            return Err(strict_runtime_unavailable(
                py,
                "class construction captures are not installed",
            ));
        }
        CaptureState::Consumed => {
            return Err(strict_runtime_unavailable(
                py,
                "class construction captures were already consumed or discarded",
            ));
        }
        CaptureState::Ready(installed) => installed,
    };
    let result = (|| {
        let witness = owner.reference(installed.namespace_witness)?;
        let mut namespace = ptr::null_mut();
        if unsafe { crate::PyWeakref_GetRef(witness.as_ptr(), &mut namespace) } < 0 {
            return Err(PyErr::fetch(py));
        }
        if namespace.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "paired class namespace is no longer live",
            ));
        }
        let namespace = unsafe { Bound::<PyAny>::from_owned_ptr(py, namespace) };
        if namespace.as_ptr() != actual_namespace.function().as_ptr()
            || installed.namespace_owner != actual_namespace.owner().as_ptr() as usize
            || installed.namespace_function != actual_namespace.function_id()?
        {
            return Err(strict_runtime_unavailable(
                py,
                "class construction replayed another namespace function",
            ));
        }
        let interpreter = unsafe { ffi::PyInterpreterState_GetID(ffi::PyInterpreterState_Get()) };
        if interpreter < 0 {
            return Err(PyErr::fetch(py));
        }
        let mut cells = BTreeMap::new();
        for (leaves, index) in &installed.cells {
            let cell = owner.reference(*index)?;
            if !is_cell(&cell) {
                return Err(strict_runtime_unavailable(
                    py,
                    "private original cell is unavailable",
                ));
            }
            for leaf in leaves {
                cells.insert(leaf.clone(), cell.clone());
            }
        }
        Ok(Some(ClassConstructionCaptures {
            py,
            interpreter,
            cells,
            namespace: unsafe {
                Bound::from_borrowed_ptr(py, actual_namespace.function().as_ptr())
            },
            namespace_owner: actual_namespace.owner().as_ptr() as usize,
            namespace_cells_taken: Cell::new(false),
        }))
    })();
    // Publish consumed before clearing GC edges. The returned view pins the
    // original cells, and contents are sampled only after the namespace body.
    let cleared = clear_installed(&owner, &installed);
    match result {
        Ok(value) => {
            cleared?;
            Ok(value)
        }
        Err(error) => Err(error),
    }
}

/// Idempotent cleanup of a compiler-private helper. This is not a new call or
/// capability check: a later public __code__ write cannot suppress release.
pub(crate) unsafe extern "C" fn discard_class_construction_captures(
    function: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> PyResult<()> {
        if function.is_null() || unsafe { ffi::PyFunction_Check(function) } == 0 {
            return Err(strict_runtime_unavailable(
                py,
                "class capture cleanup has no original helper",
            ));
        }
        let pointer = unsafe { PyFunction_GetSoacStrictOwner(function) };
        if pointer.is_null() {
            if !unsafe { ffi::PyErr_Occurred() }.is_null() {
                return Err(PyErr::fetch(py));
            }
            return Ok(());
        }
        let owner = StrictStateRef::<StrictFunctionData>::from_owner(unsafe {
            Bound::from_borrowed_ptr(py, pointer)
        })?;
        if owner.data().function_identity != function as usize
            || owner
                .source()
                .is_none_or(|origin| origin.role != CallableSourceRole::ClassConstruction)
        {
            return Err(strict_runtime_unavailable(
                py,
                "class capture cleanup has the wrong native owner",
            ));
        }
        discard_owner_captures(&owner)
    }));
    match result {
        Ok(Ok(())) => py.None().into_ptr(),
        Ok(Err(error)) => {
            error.restore(py);
            ptr::null_mut()
        }
        Err(_) => {
            strict_runtime_unavailable(py, "panic while clearing class construction captures")
                .restore(py);
            ptr::null_mut()
        }
    }
}
