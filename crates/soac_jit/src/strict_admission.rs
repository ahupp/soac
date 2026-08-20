//! Per-template compiler admission, distinct from runtime layout proofs.
//!
//! Native source matching mints this catalogue while verified source and the
//! original code tree are in hand. Helpers derive permission from explicit
//! MakeFunction operations, never from a module name or helper spelling.
//! Public entry must additionally authenticate the actual function execution.

use std::collections::HashMap;

use pyo3::{ffi, prelude::*};
use soac_contracts::Fingerprint;
use soac_core::block_py::{
    AnnotationProviderScope, BlockPyFunction, BlockPyModule, CallableSourceOrigin,
    CallableSourceRole, CellCaptureProjection, ChildVisitable, ClassConstructionScope, ClosureSlot,
    FunctionDefaultsProjection, FunctionKind, HasSemanticInstrId, InstrId, ModuleShape, ParamSpec,
    RuntimeFunctionId, StrictModuleSource, TypeParameterScope, Visit,
};
use soac_ir_blockpy::{BlockPyModuleShape, InstrBlockPy};

use crate::{VerifiedStrictModule, strict_runtime_unavailable};

mod lexical;

unsafe extern "C" {
    fn PyCode_GetSoacStrictSourceId(code: *mut ffi::PyObject) -> u64;
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TemplateShape {
    source: Option<CallableSourceOrigin>,
    generator_expression_code: Option<soac_core::block_py::GeneratorExpressionCode>,
    kind: FunctionKind,
    public_parameters: ParamSpec,
    body_parameters: ParamSpec,
    public_captures: Vec<ClosureSlot>,
    annotation_provider: Option<AnnotationProviderScope>,
    type_parameter_scope: Option<TypeParameterScope>,
    creation_defaults: FunctionDefaultsProjection,
    class_construction: Option<ClassConstructionScope>,
    class_bindings: Option<soac_core::block_py::ClassBindingScope>,
    private_lexical: Option<soac_core::block_py::PrivateLexicalScope>,
    capture_source_names: HashMap<String, String>,
    capture_projections: HashMap<String, CellCaptureProjection>,
    cell_value_aliases: HashMap<String, String>,
}

impl TemplateShape {
    fn for_function<S: ModuleShape>(function: &BlockPyFunction<S>) -> Self {
        Self {
            source: function.scope.source_origin.clone(),
            generator_expression_code: function.scope.generator_expression_code.clone(),
            kind: function.kind,
            public_parameters: function.params.clone(),
            body_parameters: function.body_params().clone(),
            public_captures: function
                .public_storage_layout()
                .map_or_else(Vec::new, |layout| layout.freevars.clone()),
            annotation_provider: function.scope.annotation_provider.clone(),
            type_parameter_scope: function.scope.type_parameter_scope.clone(),
            creation_defaults: function.scope.creation_defaults,
            class_construction: function.scope.class_construction.clone(),
            class_bindings: function.scope.class_bindings.clone(),
            private_lexical: function.scope.private_lexical.clone(),
            capture_source_names: function.scope.cell_capture_source_names.clone(),
            capture_projections: function.scope.cell_capture_projections.clone(),
            cell_value_aliases: function.scope.cell_value_aliases.clone(),
        }
    }

    fn matches<S: ModuleShape>(&self, function: &BlockPyFunction<S>) -> bool {
        self.source == function.scope.source_origin
            && self.generator_expression_code == function.scope.generator_expression_code
            && self.kind == function.kind
            && self.public_parameters == function.params
            && &self.body_parameters == function.body_params()
            && self.public_captures.as_slice()
                == function
                    .public_storage_layout()
                    .map_or(&[][..], |layout| layout.freevars.as_slice())
            && self.annotation_provider == function.scope.annotation_provider
            && self.type_parameter_scope == function.scope.type_parameter_scope
            && self.creation_defaults == function.scope.creation_defaults
            && self.class_construction == function.scope.class_construction
            && self.class_bindings == function.scope.class_bindings
            && self.private_lexical == function.scope.private_lexical
            && self.capture_source_names == function.scope.cell_capture_source_names
            && self.capture_projections == function.scope.cell_capture_projections
            && self.cell_value_aliases == function.scope.cell_value_aliases
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemplateProvenance {
    /// The original-code map owns this exact address.
    MatchedNativeCode { address: usize },
    /// Source-ID/stamp proof only: no borrowed native root address.
    VerifiedModuleBody,
    /// The producer already has individual admission. A public helper call is
    /// not this compiler-selected operation, even with the same operands.
    CompilerCreation {
        producer: RuntimeFunctionId,
        instruction: InstrId,
    },
}

struct TemplateWitness {
    shape: TemplateShape,
    provenance: TemplateProvenance,
}

/// Opaque native matching output. Public callers cannot manufacture or
/// deserialize it from source IDs, module membership, or annotations.
pub struct AuthenticatedCodeCatalog {
    source: StrictModuleSource,
    startup_identity: Fingerprint,
    interpreter_id: i64,
    native_source_id: u64,
    originals: HashMap<RuntimeFunctionId, Py<PyAny>>,
    // Public gi_code/ag_code only. These code objects never establish
    // SourceFunction admission or replace the helper's execution code.
    generator_expression_codes: HashMap<RuntimeFunctionId, Py<PyAny>>,
    templates: HashMap<RuntimeFunctionId, TemplateWitness>,
}

impl AuthenticatedCodeCatalog {
    pub(crate) fn from_compiled(
        py: Python<'_>,
        verified: &VerifiedStrictModule,
        module: &BlockPyModule<BlockPyModuleShape>,
        native_root: &Bound<'_, PyAny>,
        originals: HashMap<RuntimeFunctionId, Py<PyAny>>,
        generator_expression_codes: HashMap<RuntimeFunctionId, Py<PyAny>>,
    ) -> PyResult<Self> {
        let source = module.strict_source.as_ref().ok_or_else(|| {
            strict_runtime_unavailable(py, "code catalogue requires verified strict IR")
        })?;
        let interpreter_id =
            unsafe { ffi::PyInterpreterState_GetID(ffi::PyInterpreterState_Get()) };
        if interpreter_id != verified.interpreter_id()
            || !source.matches_verified(verified.type_facts())
            || unsafe { ffi::Py_TYPE(native_root.as_ptr()) }
                != std::ptr::addr_of_mut!(ffi::PyCode_Type)
        {
            return Err(strict_runtime_unavailable(
                py,
                "code catalogue native root mismatch",
            ));
        }
        let native_source_id = unsafe { PyCode_GetSoacStrictSourceId(native_root.as_ptr()) };
        if native_source_id == 0 {
            return Err(strict_runtime_unavailable(
                py,
                "code catalogue root is unauthenticated",
            ));
        }
        lexical::validate_creations(module, verified.type_facts().facts())?;
        let mut templates = HashMap::new();
        for function in &module.callable_defs {
            let provenance = if let Some(original) = originals.get(&function.function_id) {
                let Some(origin) = &function.scope.source_origin else {
                    return Err(strict_runtime_unavailable(
                        py,
                        "matched native code has no source origin",
                    ));
                };
                if !matches!(
                    origin.role,
                    CallableSourceRole::SourceFunction
                        | CallableSourceRole::AnnotationProvider
                        | CallableSourceRole::TypeParameterScope
                ) || unsafe { ffi::Py_TYPE(original.as_ptr()) }
                    != std::ptr::addr_of_mut!(ffi::PyCode_Type)
                    || unsafe { PyCode_GetSoacStrictSourceId(original.as_ptr()) }
                        != native_source_id
                {
                    return Err(strict_runtime_unavailable(
                        py,
                        "matched native code belongs to another source tree",
                    ));
                }
                Some(TemplateProvenance::MatchedNativeCode {
                    address: original.as_ptr() as usize,
                })
            } else if function.scope.source_origin.as_ref().is_some_and(|origin| {
                origin.role == CallableSourceRole::ModuleBody
                    && origin.definition == verified.type_facts().facts().module_body_identity()
            }) {
                Some(TemplateProvenance::VerifiedModuleBody)
            } else {
                None
            };
            if let Some(provenance) = provenance {
                templates.insert(
                    function.function_id,
                    TemplateWitness {
                        shape: TemplateShape::for_function(function),
                        provenance,
                    },
                );
            }
        }
        if originals.keys().any(|id| !templates.contains_key(id)) {
            return Err(strict_runtime_unavailable(
                py,
                "native catalogue has an unmatched function",
            ));
        }
        if templates
            .values()
            .filter(|witness| witness.provenance == TemplateProvenance::VerifiedModuleBody)
            .count()
            != 1
        {
            return Err(strict_runtime_unavailable(
                py,
                "catalogue has no unique authenticated module body",
            ));
        }
        let edges = compiler_creation_edges(module)?;
        // Reachability through explicit operations, not lexical names.
        // Unrooted cycles cannot manufacture permission.
        loop {
            let mut changed = false;
            for edge in &edges {
                if templates.contains_key(&edge.target) || !templates.contains_key(&edge.producer) {
                    continue;
                }
                let target = module
                    .callable_defs
                    .iter()
                    .find(|function| function.function_id == edge.target)
                    .ok_or_else(|| {
                        strict_runtime_unavailable(py, "compiler creation target is absent")
                    })?;
                if target.scope.source_origin.as_ref().is_some_and(|origin| {
                    matches!(
                        origin.role,
                        CallableSourceRole::SourceFunction
                            | CallableSourceRole::AnnotationProvider
                            | CallableSourceRole::TypeParameterScope
                    )
                }) {
                    // Missing source/provider native identity is not replaced
                    // with the weaker compiler-helper construction route.
                    continue;
                }
                templates.insert(
                    edge.target,
                    TemplateWitness {
                        shape: TemplateShape::for_function(target),
                        provenance: TemplateProvenance::CompilerCreation {
                            producer: edge.producer,
                            instruction: edge.instruction,
                        },
                    },
                );
                changed = true;
            }
            if !changed {
                break;
            }
        }
        for (id, code) in &generator_expression_codes {
            let function = module
                .callable_defs
                .iter()
                .find(|function| function.function_id == *id)
                .ok_or_else(|| strict_runtime_unavailable(py, "code exposure target is absent"))?;
            if function.scope.source_origin.is_some()
                || function.scope.generator_expression_code.is_none()
                || !matches!(
                    function.lowered_kind(),
                    FunctionKind::Generator | FunctionKind::AsyncGenerator
                )
                || originals.contains_key(id)
                || !templates.get(id).is_some_and(|witness| {
                    matches!(
                        witness.provenance,
                        TemplateProvenance::CompilerCreation { .. }
                    )
                })
                || unsafe { ffi::Py_TYPE(code.as_ptr()) }
                    != std::ptr::addr_of_mut!(ffi::PyCode_Type)
                || unsafe { PyCode_GetSoacStrictSourceId(code.as_ptr()) } != native_source_id
            {
                return Err(strict_runtime_unavailable(
                    py,
                    "generator code exposure has no matching compiler creation",
                ));
            }
        }
        Ok(Self {
            source: source.clone(),
            startup_identity: verified.startup_identity(),
            interpreter_id,
            native_source_id,
            originals,
            generator_expression_codes,
            templates,
        })
    }

    pub fn len(&self) -> usize {
        self.originals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.originals.is_empty()
    }

    pub(crate) fn matches_verified(&self, verified: &VerifiedStrictModule) -> bool {
        self.interpreter_id == verified.interpreter_id()
            && self.startup_identity == verified.startup_identity()
            && self.source.matches_verified(verified.type_facts())
            && self.native_source_id != 0
    }

    pub(crate) fn admits<S: ModuleShape>(
        &self,
        verified: &VerifiedStrictModule,
        function: &BlockPyFunction<S>,
    ) -> bool {
        if !self.matches_verified(verified) {
            return false;
        }
        let Some(witness) = self.templates.get(&function.function_id) else {
            return false;
        };
        if !witness.shape.matches(function) {
            return false;
        }
        match witness.provenance {
            TemplateProvenance::MatchedNativeCode { address } => self
                .originals
                .get(&function.function_id)
                .is_some_and(|code| code.as_ptr() as usize == address),
            TemplateProvenance::VerifiedModuleBody => true,
            TemplateProvenance::CompilerCreation { producer, .. } => {
                self.templates.contains_key(&producer)
            }
        }
    }
}

struct CreationEdge {
    producer: RuntimeFunctionId,
    instruction: InstrId,
    target: RuntimeFunctionId,
}

fn compiler_creation_edges(
    module: &BlockPyModule<BlockPyModuleShape>,
) -> PyResult<Vec<CreationEdge>> {
    struct Collector {
        producer: RuntimeFunctionId,
        edges: Vec<CreationEdge>,
        missing_id: bool,
    }
    impl Visit<InstrBlockPy> for Collector {
        fn visit_instr(&mut self, expression: &InstrBlockPy) {
            let target = match expression {
                InstrBlockPy::MakeFunctionWithClosure(op) => Some(op.function_id),
                _ => None,
            };
            if let Some(target) = target {
                if let Some(instruction) = expression.try_semantic_instr_id() {
                    self.edges.push(CreationEdge {
                        producer: self.producer,
                        instruction,
                        target,
                    });
                } else {
                    self.missing_id = true;
                }
            }
            expression.visit_children(self);
        }
    }
    let mut edges = Vec::new();
    for function in &module.callable_defs {
        let mut collector = Collector {
            producer: function.function_id,
            edges: Vec::new(),
            missing_id: false,
        };
        collector.visit_fn(function);
        if collector.missing_id {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "compiler creation operation has no semantic instruction identity",
            ));
        }
        edges.extend(collector.edges);
    }
    Ok(edges)
}

/// Ordinary originals can support inspection, but never runtime admission.
/// The production constructor accepts only AuthenticatedCodeCatalog.
pub(crate) enum OriginalCodeStorage {
    Inspection(HashMap<RuntimeFunctionId, Py<PyAny>>),
    Authenticated(AuthenticatedCodeCatalog),
}

impl OriginalCodeStorage {
    fn originals(&self) -> &HashMap<RuntimeFunctionId, Py<PyAny>> {
        match self {
            Self::Inspection(originals) => originals,
            Self::Authenticated(catalog) => &catalog.originals,
        }
    }
    pub(crate) fn get(&self, id: &RuntimeFunctionId) -> Option<&Py<PyAny>> {
        self.originals().get(id)
    }
    pub(crate) fn generator_expression_code(&self, id: &RuntimeFunctionId) -> Option<&Py<PyAny>> {
        match self {
            Self::Authenticated(catalog) => catalog.generator_expression_codes.get(id),
            Self::Inspection(_) => None,
        }
    }
    pub(crate) fn values(&self) -> impl Iterator<Item = &Py<PyAny>> {
        let exposed = match self {
            Self::Authenticated(catalog) => Some(&catalog.generator_expression_codes),
            Self::Inspection(_) => None,
        };
        self.originals()
            .values()
            .chain(exposed.into_iter().flat_map(|codes| codes.values()))
    }
    pub(crate) fn authenticated(&self) -> Option<&AuthenticatedCodeCatalog> {
        match self {
            Self::Authenticated(catalog) => Some(catalog),
            Self::Inspection(_) => None,
        }
    }
}
