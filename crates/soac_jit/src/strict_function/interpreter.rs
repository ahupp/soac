//! CPython implementation arm of the common strict function owner.
//! No Python source primary lives here. Actual function/frame/C operands support
//! borrowed reads; cached scalar addresses are compared, never dereferenced.

use super::*;
use crate::strict_interpreter_source::InterpreterCode;

const NATIVE_MODULE_POLICY: usize = 0;
const NATIVE_SELF_WEAK: usize = 1;
const NATIVE_PROVIDER_WEAK: usize = 2;
const NATIVE_CLASS_WEAK: usize = 3;
// Existing native code.h/opcode_utils.h ABI values, not a new source schema.
const CO_FUTURE_STRICT: i32 = 0x10000000;
const MAKE_FUNCTION_DEFAULTS: u32 = 0x01;
const MAKE_FUNCTION_KWDEFAULTS: u32 = 0x02;
const MAKE_FUNCTION_ANNOTATIONS: u32 = 0x04;
const MAKE_FUNCTION_CLOSURE: u32 = 0x08;
const MAKE_FUNCTION_ANNOTATE: u32 = 0x10;

fn source_origin(code: &InterpreterCode) -> Option<CallableSourceOrigin> {
    let role = match code.role() {
        InterpreterCodeRole::SourceFunction | InterpreterCodeRole::AsyncSourceFunction => {
            CallableSourceRole::SourceFunction
        }
        InterpreterCodeRole::ClassNamespace => CallableSourceRole::ClassNamespace,
        InterpreterCodeRole::AnnotationProvider
        | InterpreterCodeRole::TypeAlias
        | InterpreterCodeRole::TypeVariable => CallableSourceRole::AnnotationProvider,
        InterpreterCodeRole::TypeParameterScope => CallableSourceRole::TypeParameterScope,
        InterpreterCodeRole::Module
        | InterpreterCodeRole::Lambda
        | InterpreterCodeRole::Comprehension => return None,
    };
    Some(CallableSourceOrigin {
        definition: code.source().clone(),
        role,
    })
}

unsafe fn actual_function_owner<'py>(
    py: Python<'py>,
    function: *mut ffi::PyObject,
) -> PyResult<StrictStateRef<'py, StrictFunctionData>> {
    if function.is_null() || unsafe { ffi::PyFunction_Check(function) } == 0 {
        return Err(strict_runtime_unavailable(
            py,
            "native strict operand is not a function",
        ));
    }
    let owner = unsafe { PyFunction_GetSoacStrictOwner(function) };
    if owner.is_null() {
        return Err(if unsafe { ffi::PyErr_Occurred() }.is_null() {
            strict_runtime_unavailable(py, "native function has no actual birth owner")
        } else {
            PyErr::fetch(py)
        });
    }
    let owner = StrictStateRef::<StrictFunctionData>::from_owner(unsafe {
        Bound::from_borrowed_ptr(py, owner)
    })?;
    if owner.data().function_identity != function as usize || owner.is_failed_pending() {
        return Err(strict_runtime_unavailable(
            py,
            "native function birth is foreign or terminal",
        ));
    }
    owner.native_implementation()?;
    Ok(owner)
}

/// Native calls before CREATE watchers, with initialized fields/weakref support.
///
/// # Safety
/// The actual native birth function remains supported through allocation/reentry.
/// The returned metadata is not installed here: native commits exactly one edge.
pub(crate) unsafe fn prepare_interpreter_function_owner<'a, 'py>(
    py: Python<'py>,
    function: Borrowed<'a, 'py, PyAny>,
    source: Arc<StrictInterpreterSource>,
    execution: crate::StrictModuleExecutionRef,
    native_code_ordinal: u32,
    birth_execution: Arc<InterpreterInvocationIdentity>,
    creation_execution: Option<Arc<crate::strict_namespace::NamespaceExecution>>,
) -> PyResult<StrictStateRef<'py, StrictFunctionData>> {
    if unsafe { ffi::PyFunction_Check(function.as_ptr()) } == 0 {
        return Err(strict_runtime_unavailable(
            py,
            "native birth requires an actual function",
        ));
    }
    let raw = function.as_ptr().cast::<ffi::PyFunctionObject>();
    let original_code = unsafe { (*raw).func_code };
    let globals = unsafe { (*raw).func_globals };
    let builtins = unsafe { (*raw).func_builtins };
    if original_code.is_null()
        || globals.is_null()
        || builtins.is_null()
        || unsafe { ffi::PyDict_CheckExact(globals) } == 0
        || !unsafe { PyFunction_GetSoacStrictOwner(function.as_ptr()) }.is_null()
        || !unsafe { ffi::PyErr_Occurred() }.is_null()
    {
        return Err(strict_runtime_unavailable(
            py,
            "native birth code/environment/owner unavailable",
        ));
    }
    let code_view = unsafe { Borrowed::<PyAny>::from_ptr(py, original_code) };
    let code = source.code(py, &code_view)?;
    if code.ordinal() != native_code_ordinal || code.role() == InterpreterCodeRole::Module {
        return Err(strict_runtime_unavailable(
            py,
            "native birth source ordinal or role differs",
        ));
    }
    let origin = source_origin(code);
    let eligible = eligible_source_function(source.verified(), origin.as_ref());
    if let Some(creation) = &creation_execution {
        creation.validate_native_creation(py, source.verified(), &execution, globals as usize)?;
    }
    let globals_view =
        unsafe { Borrowed::<PyAny>::from_ptr(py, globals).cast_unchecked::<PyDict>() };
    let policy = execution.acquire_owner(py, &globals_view, source.verified())?;
    let self_weak = unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(
            py,
            crate::PyWeakref_NewRef(function.as_ptr(), ptr::null_mut()),
        )?
    };
    let class_weak = creation_execution.as_ref().map(|_| NATIVE_CLASS_WEAK);
    let mut references = vec![policy, self_weak.unbind(), py.None()];
    if class_weak.is_some() {
        references.push(py.None());
    }
    let owner = StrictStateRef::new(
        py,
        StrictFunctionData {
            source: origin,
            verified: Arc::clone(source.verified()),
            execution,
            function_identity: function.as_ptr() as usize,
            implementation: StrictFunctionImplementation::Cpython(
                InterpreterFunctionImplementation {
                    source,
                    native_code_ordinal,
                    original_code_identity: original_code as usize,
                    globals_identity: globals as usize,
                    builtins_identity: builtins as usize,
                    birth_execution,
                    frozen_metadata: OnceCell::new(),
                    original_code_entered: Cell::new(false),
                },
            ),
            references: FunctionMetadataReferences {
                module_policy: NATIVE_MODULE_POLICY,
                annotation_provider: NATIVE_PROVIDER_WEAK,
                self_weak: Some(NATIVE_SELF_WEAK),
                class_weak,
            },
            finalized: Cell::new(false),
            capability_globals_pending: Cell::new(false),
            failed_pending: Cell::new(false),
            eligible,
            call_counters: crate::strict_call::StrictCallCounters::default(),
            capability_nominals: Vec::new(),
            nominal_bindings: RefCell::new(BTreeMap::new()),
            creation_execution,
        },
        references,
    )?;
    // Revalidate after allocation; never dereference captured scalar addresses.
    if unsafe { (*raw).func_code } != original_code
        || unsafe { (*raw).func_globals } != globals
        || unsafe { (*raw).func_builtins } != builtins
        || !unsafe { PyFunction_GetSoacStrictOwner(function.as_ptr()) }.is_null()
        || !unsafe { ffi::PyErr_Occurred() }.is_null()
    {
        return Err(strict_runtime_unavailable(
            py,
            "native birth changed before owner publication",
        ));
    }
    owner
        .execution()
        .validate_owner(py, &owner.module_policy_owner()?, &owner.data().verified)?;
    Ok(owner)
}

/// # Safety
/// All values are actual borrowed frame operands captured before binding.
/// The captured code need not equal mutable idle code.
pub(crate) unsafe fn authenticate_interpreter_entry<'a, 'py>(
    py: Python<'py>,
    function: Borrowed<'a, 'py, PyAny>,
    captured_code: Borrowed<'a, 'py, PyAny>,
    globals: Borrowed<'a, 'py, PyDict>,
    builtins: Borrowed<'a, 'py, PyAny>,
) -> PyResult<AuthenticatedStrictFunction<'a, 'py>> {
    let owner = unsafe { actual_function_owner(py, function.as_ptr()) }?;
    let source_authority = validate_entry(py, &owner, function, captured_code, globals, builtins)?;
    Ok(AuthenticatedStrictFunction {
        owner,
        _function: SupportedOperand::Borrowed(function),
        implementation: AuthenticatedImplementation::Cpython { source_authority },
    })
}

pub(super) fn validate_entry(
    py: Python<'_>,
    owner: &StrictStateRef<'_, StrictFunctionData>,
    function: Borrowed<'_, '_, PyAny>,
    captured_code: Borrowed<'_, '_, PyAny>,
    globals: Borrowed<'_, '_, PyDict>,
    builtins: Borrowed<'_, '_, PyAny>,
) -> PyResult<bool> {
    owner.ensure_live()?;
    let native = owner.native_implementation()?;
    let raw = function.as_ptr().cast::<ffi::PyFunctionObject>();
    if owner.is_failed_pending()
        || owner.data().function_identity != function.as_ptr() as usize
        || !unsafe { crate::PyFunction_GetSoacMetadata(function.as_ptr()) }.is_null()
        || native.globals_identity != globals.as_ptr() as usize
        || native.builtins_identity != builtins.as_ptr() as usize
        || unsafe { (*raw).func_globals } != globals.as_ptr()
        || unsafe { (*raw).func_builtins } != builtins.as_ptr()
        || unsafe { PyFunction_GetSoacStrictOwner(function.as_ptr()) } != owner.owner().as_ptr()
    {
        return Err(strict_runtime_unavailable(
            py,
            "native frame differs from its actual birth owner",
        ));
    }
    if unsafe { PyFunction_CheckSoacStrictDefaults(function.as_ptr()) } < 0 {
        return Err(PyErr::fetch(py));
    }
    let source_authority = captured_code.as_ptr() as usize == native.original_code_identity;
    if source_authority {
        let code = native.source.code(py, &captured_code)?;
        if code.ordinal() != native.native_code_ordinal
            || !Arc::ptr_eq(native.source.verified(), &owner.data().verified)
        {
            return Err(strict_runtime_unavailable(
                py,
                "native frame has foreign original code",
            ));
        }
    } else {
        let code = unsafe { crate::code_view::view(py, captured_code.as_ptr()) }?;
        if code.strict_source_id != 0
            || code.flags & CO_FUTURE_STRICT != 0
            || owner
                .source()
                .is_some_and(|origin| origin.role == CallableSourceRole::ClassNamespace)
        {
            return Err(strict_runtime_unavailable(
                py,
                "replacement code has no ordinary fallback",
            ));
        }
    }
    if let Some(frozen) = native.frozen_metadata.get() {
        if frozen.code != captured_code.as_ptr() as usize
            || frozen.defaults != unsafe { (*raw).func_defaults } as usize
            || frozen.keyword_defaults != unsafe { (*raw).func_kwdefaults } as usize
            || frozen.closure != unsafe { (*raw).func_closure } as usize
            || frozen.ordinary_replacement == source_authority
            || unsafe { PyFunction_GetSoacStrictId(function.as_ptr()) }
                != frozen.seal_identity.0.get()
        {
            return Err(strict_runtime_unavailable(
                py,
                "sealed native function metadata differs",
            ));
        }
    } else if owner.is_finalized() || unsafe { PyFunction_GetSoacStrictId(function.as_ptr()) } != 0
    {
        return Err(strict_runtime_unavailable(
            py,
            "native seal lacks matching frozen metadata",
        ));
    }
    let policy = owner
        .execution()
        .acquire_owner(py, &globals, &owner.data().verified)?;
    if policy.as_ptr() != unsafe { owner.reference_ptr(NATIVE_MODULE_POLICY)? }.as_ptr() {
        return Err(strict_runtime_unavailable(
            py,
            "native frame module execution changed",
        ));
    }
    // Only policy metadata was acquired; no execution value became a new
    // primary owner. Recheck actual ownership after possible allocation/reentry.
    if owner.is_failed_pending()
        || unsafe { PyFunction_GetSoacStrictOwner(function.as_ptr()) } != owner.owner().as_ptr()
    {
        return Err(strict_runtime_unavailable(
            py,
            "native owner retired during validation",
        ));
    }
    Ok(source_authority)
}

/// # Safety
/// The actual native activation supports live_function throughout this callback.
/// ROOT separately compares captured code/maps to its actual invocation state.
pub(crate) unsafe fn authenticate_captured_interpreter_owner<'py>(
    py: Python<'py>,
    live_function: NonNull<ffi::PyObject>,
    expected_owner: usize,
    source: &Arc<StrictInterpreterSource>,
    execution: &crate::StrictModuleExecutionRef,
) -> PyResult<StrictStateRef<'py, StrictFunctionData>> {
    let actual = unsafe { PyFunction_GetSoacStrictOwner(live_function.as_ptr()) };
    if actual.is_null() || actual as usize != expected_owner {
        return Err(if unsafe { ffi::PyErr_Occurred() }.is_null() {
            strict_runtime_unavailable(py, "captured native owner is absent or foreign")
        } else {
            PyErr::fetch(py)
        });
    }
    let owner = unsafe { actual_function_owner(py, live_function.as_ptr()) }?;
    if !Arc::ptr_eq(owner.native_source()?, source)
        || !Arc::ptr_eq(&owner.data().verified, source.verified())
        || !owner.execution().same_execution(execution)
    {
        return Err(strict_runtime_unavailable(
            py,
            "captured native source execution differs",
        ));
    }
    execution.validate_owner(py, &owner.module_policy_owner()?, source.verified())?;
    Ok(owner)
}

/// # Safety
/// Native owns the actual operands and has published the field. Success is
/// callback/allocation-free. No borrowed read may follow an error allocation.
pub(crate) unsafe fn record_native_function_attribute(
    py: Python<'_>,
    function: NonNull<ffi::PyObject>,
    attribute_flag: u32,
    installed_value: NonNull<ffi::PyObject>,
    source: &Arc<StrictInterpreterSource>,
    execution: &crate::StrictModuleExecutionRef,
    actual_birth_execution: &Arc<InterpreterInvocationIdentity>,
) -> PyResult<()> {
    let owner = unsafe { actual_function_owner(py, function.as_ptr()) }?;
    if !Arc::ptr_eq(owner.native_source()?, source)
        || !owner.execution().same_execution(execution)
        || !Arc::ptr_eq(owner.native_birth_execution()?, actual_birth_execution)
    {
        return Err(strict_runtime_unavailable(
            py,
            "attribute belongs to another actual birth",
        ));
    }
    let raw = function.as_ptr().cast::<ffi::PyFunctionObject>();
    let actual = unsafe {
        match attribute_flag {
            MAKE_FUNCTION_DEFAULTS => (*raw).func_defaults,
            MAKE_FUNCTION_KWDEFAULTS => (*raw).func_kwdefaults,
            MAKE_FUNCTION_ANNOTATIONS => (*raw).func_annotations,
            MAKE_FUNCTION_CLOSURE => (*raw).func_closure,
            MAKE_FUNCTION_ANNOTATE => (*raw).func_annotate,
            _ => {
                return Err(strict_runtime_unavailable(
                    py,
                    "unknown native function attribute flag",
                ));
            }
        }
    };
    if actual != installed_value.as_ptr() {
        return Err(strict_runtime_unavailable(
            py,
            "native attribute was not published",
        ));
    }
    if attribute_flag != MAKE_FUNCTION_ANNOTATE {
        return Ok(());
    }
    if unsafe { (*raw).func_code } as usize != owner.native_implementation()?.original_code_identity
    {
        // An ordinary replacement receives no original annotation authority.
        return Ok(());
    }
    let provider = unsafe { actual_function_owner(py, installed_value.as_ptr()) }?;
    let provider_raw = installed_value.as_ptr().cast::<ffi::PyFunctionObject>();
    if !Arc::ptr_eq(provider.native_source()?, source)
        || !provider.execution().same_execution(execution)
        || !Arc::ptr_eq(provider.native_birth_execution()?, actual_birth_execution)
        || !provider.source().is_some_and(|origin| {
            origin.role == CallableSourceRole::AnnotationProvider
                && owner
                    .source()
                    .is_some_and(|target| target.definition == origin.definition)
        })
        || unsafe { (*provider_raw).func_code } as usize
            != provider.native_implementation()?.original_code_identity
    {
        return Err(strict_runtime_unavailable(
            py,
            "annotation operand has foreign producer identity",
        ));
    }
    let weak = unsafe { provider.reference_ptr(NATIVE_SELF_WEAK)? };
    owner.bind_reserved_reference(NATIVE_PROVIDER_WEAK, unsafe {
        Bound::from_borrowed_ptr(py, weak.as_ptr())
    })
}

/// # Safety
/// Finish all provider/closure reads without Python callbacks or target mutation.
/// The actual parent function supports its current provider only in that interval.
pub(super) unsafe fn borrowed_annotation_provider<'a, 'py>(
    auth: &'a AuthenticatedStrictFunction<'_, 'py>,
) -> PyResult<Option<AuthenticatedStrictFunction<'a, 'py>>> {
    let py = auth.function().py();
    let expected = unsafe { auth.reference_ptr(NATIVE_PROVIDER_WEAK)? };
    if expected.as_ptr() == unsafe { ffi::Py_None() } {
        return Ok(None);
    }
    let raw = auth.function().as_ptr().cast::<ffi::PyFunctionObject>();
    let actual = unsafe { (*raw).func_annotate };
    if actual.is_null() || unsafe { ffi::PyFunction_Check(actual) } == 0 {
        return Ok(None);
    }
    let actual_owner = unsafe { PyFunction_GetSoacStrictOwner(actual) };
    if actual_owner.is_null() {
        return if unsafe { ffi::PyErr_Occurred() }.is_null() {
            Ok(None)
        } else {
            Err(PyErr::fetch(py))
        };
    }
    // A foreign, non-source provider is unresolved; it grants no nominal target.
    let Some(owner) = StrictStateRef::<StrictFunctionData>::try_from_owner(unsafe {
        Bound::from_borrowed_ptr(py, actual_owner)
    })?
    else {
        return Ok(None);
    };
    let Some(self_weak) = owner.data().references.self_weak else {
        return Ok(None);
    };
    // Compare the retained weakref OBJECT, not a weak target address. A dead
    // original and subsequent address reuse cannot manufacture this witness.
    if unsafe { owner.reference_ptr(self_weak)? } != expected {
        return Ok(None);
    }
    if owner.data().function_identity != actual as usize
        || owner.is_failed_pending()
        || !Arc::ptr_eq(owner.native_source()?, auth.native_source()?)
        || !owner.execution().same_execution(auth.execution_ref())
        || !owner.source().is_some_and(|origin| {
            origin.role == CallableSourceRole::AnnotationProvider
                && auth
                    .source()
                    .is_some_and(|target| target.definition == origin.definition)
        })
    {
        return Err(strict_runtime_unavailable(
            py,
            "original native provider execution differs",
        ));
    }
    let provider = unsafe { Borrowed::<PyAny>::from_ptr(py, actual) };
    let raw = actual.cast::<ffi::PyFunctionObject>();
    let code = unsafe { Borrowed::from_ptr(py, (*raw).func_code) };
    let globals = unsafe { Borrowed::from_ptr(py, (*raw).func_globals).cast_unchecked::<PyDict>() };
    let builtins = unsafe { Borrowed::from_ptr(py, (*raw).func_builtins) };
    let authority = validate_entry(py, &owner, provider, code, globals, builtins)?;
    if !authority {
        return Ok(None);
    }
    Ok(Some(AuthenticatedStrictFunction {
        owner,
        _function: SupportedOperand::Borrowed(provider),
        implementation: AuthenticatedImplementation::Cpython {
            source_authority: true,
        },
    }))
}

fn frozen_record(
    py: Python<'_>,
    auth: &AuthenticatedStrictFunction<'_, '_>,
) -> PyResult<NativeFrozenFunctionIdentity> {
    let function = auth.function();
    let native = auth.native_implementation()?;
    let raw = function.as_ptr().cast::<ffi::PyFunctionObject>();
    let code = unsafe { Borrowed::from_ptr(py, (*raw).func_code) };
    let globals = unsafe { Borrowed::from_ptr(py, (*raw).func_globals).cast_unchecked::<PyDict>() };
    let builtins = unsafe { Borrowed::from_ptr(py, (*raw).func_builtins) };
    let original = validate_entry(
        py,
        auth.owner_ref(),
        function.as_borrowed(),
        code,
        globals,
        builtins,
    )?;
    Ok(NativeFrozenFunctionIdentity {
        code: code.as_ptr() as usize,
        defaults: unsafe { (*raw).func_defaults } as usize,
        keyword_defaults: unsafe { (*raw).func_kwdefaults } as usize,
        closure: unsafe { (*raw).func_closure } as usize,
        ordinary_replacement: !original,
        seal_identity: NativeFunctionSealIdentity(
            NonZeroU64::new(u64::from(native.native_code_ordinal) + 1).ok_or_else(|| {
                strict_runtime_unavailable(py, "native function seal is reserved")
            })?,
        ),
    })
}

fn commit_frozen(
    py: Python<'_>,
    auth: &AuthenticatedStrictFunction<'_, '_>,
    frozen: NativeFrozenFunctionIdentity,
) -> PyResult<()> {
    let function = auth.function();
    if unsafe { PyFunction_SealSoacStrict(function.as_ptr(), frozen.seal_identity.0.get()) } < 0 {
        return Err(PyErr::fetch(py));
    }
    let raw = function.as_ptr().cast::<ffi::PyFunctionObject>();
    // Inspect fresh fields only, never an old captured pointer after callbacks.
    if unsafe { (*raw).func_code } as usize != frozen.code
        || unsafe { (*raw).func_defaults } as usize != frozen.defaults
        || unsafe { (*raw).func_kwdefaults } as usize != frozen.keyword_defaults
        || unsafe { (*raw).func_closure } as usize != frozen.closure
        || unsafe { PyFunction_GetSoacStrictOwner(function.as_ptr()) } != auth.owner().as_ptr()
    {
        return Err(strict_runtime_unavailable(
            py,
            "native function changed while sealing",
        ));
    }
    let native = auth.native_implementation()?;
    if let Some(existing) = native.frozen_metadata.get() {
        if *existing != frozen {
            return Err(strict_runtime_unavailable(
                py,
                "native function frozen metadata conflicts",
            ));
        }
    } else {
        native.frozen_metadata.set(frozen).map_err(|_| {
            strict_runtime_unavailable(py, "native function frozen metadata installed twice")
        })?;
    }
    auth.data().finalized.set(true);
    Ok(())
}

pub(super) fn freeze(py: Python<'_>, auth: &AuthenticatedStrictFunction<'_, '_>) -> PyResult<()> {
    commit_frozen(py, auth, frozen_record(py, auth)?)
}

/// Commit the original provider before the containing function's source Store.
/// The exact one-posonly-arg native provider has no value-type promises.
/// An exact keyword-default dictionary
/// can use native READ_ONLY sealing: it allocates raw policy metadata but no
/// Python object, hashes/compares no keys and calls no Python. The target's real
/// provider edge therefore supports the borrowed provider throughout this call.
pub(super) fn finalize_provider(
    py: Python<'_>,
    target: &AuthenticatedStrictFunction<'_, '_>,
) -> PyResult<()> {
    let Some(provider) = (unsafe { borrowed_annotation_provider(target) })? else {
        return Ok(());
    };
    let raw = provider.function().as_ptr().cast::<ffi::PyFunctionObject>();
    let actual_code = unsafe { Borrowed::from_ptr(py, (*raw).func_code) };
    let code = provider.native_source()?.code(py, &actual_code)?;
    if code.role() != InterpreterCodeRole::AnnotationProvider
        || code.layout().positional_count != 1
        || code.layout().positional_only_count != 1
        || code.layout().keyword_only_count != 0
        || code.layout().parameters.len() != 1
        || !provider.capability_nominal_bindings().is_empty()
        || (!unsafe { (*raw).func_kwdefaults }.is_null()
            && unsafe { ffi::PyDict_CheckExact((*raw).func_kwdefaults) } == 0)
        || !unsafe { crate::PyFunction_GetSoacMetadata(provider.function().as_ptr()) }.is_null()
    {
        return Err(strict_runtime_unavailable(
            py,
            "native provider finalization lacks its callback-free original metadata shape",
        ));
    }
    // No compiled source entry or arbitrary dictionary validator participates.
    // Native READ_ONLY policy preparation preserves the actual complete mapping,
    // including unused non-string keys, without adding a provider/code owner.
    // No Python callback intervenes before publication or on idempotent sealing.
    commit_frozen(py, &provider, frozen_record(py, &provider)?)
}
