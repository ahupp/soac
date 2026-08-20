use std::collections::{BTreeMap, BTreeSet};

use crate::ModuleContentId;
use serde::{Deserialize, Serialize};

use crate::identity::module_content_id_serde;
use crate::{
    ARTIFACT_SCHEMA_VERSION, AttributeSiteIdentity, CallSiteIdentity, CheckedFieldPolicy,
    ClassReference, ContractError, DefinitionKind, DependencyFingerprint, Fingerprint,
    ResolvedStrictPolicy, SourceIdentity, SourceRange, legacy_source_hash,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceDialect {
    OrdinaryPython,
    SoacStrict,
}

/// Source facts are proposals. Even an authenticated, fully precise record
/// does not mean that a corresponding live Python object has been protected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleTypeFacts {
    pub schema_version: u32,
    #[serde(with = "module_content_id_serde")]
    pub module: ModuleContentId,
    pub source_digest: Fingerprint,
    pub source_size: u32,
    pub source_dialect: SourceDialect,
    pub language_policy: ResolvedStrictPolicy,
    pub consumed_dependencies: Vec<DependencyFingerprint>,
    pub global_bindings: Vec<GlobalBindingFact>,
    pub classes: Vec<ClassTypeFact>,
    pub functions: Vec<FunctionTypeFact>,
    /// Each source annotation leaf retains its actual lexical value binding.
    /// A source class can execute more than once, so a ClassReference alone
    /// cannot identify the runtime target of a particular function annotation
    /// or field contract.
    pub nominal_bindings: Vec<NominalBindingFact>,
    pub attribute_sites: Vec<AttributeSiteFact>,
    pub call_sites: Vec<CallSiteFact>,
    pub diagnostics: Vec<StrictDiagnostic>,
}

impl ModuleTypeFacts {
    /// Establish source identities without attempting to infer source
    /// semantics. `source_dialect` must come from the authenticated parser,
    /// not a substring test or an inherited code-object future flag.
    pub fn new(
        module_name: impl Into<String>,
        source: &[u8],
        source_dialect: SourceDialect,
        language_policy: ResolvedStrictPolicy,
    ) -> Result<Self, ContractError> {
        let source_size = u32::try_from(source.len()).map_err(|_| {
            ContractError::InvalidSourceIdentity("source exceeds the parser's byte range".into())
        })?;
        Ok(Self {
            schema_version: ARTIFACT_SCHEMA_VERSION,
            module: ModuleContentId::new(module_name, legacy_source_hash(source)),
            source_digest: Fingerprint::digest(source),
            source_size,
            source_dialect,
            language_policy,
            consumed_dependencies: Vec::new(),
            global_bindings: Vec::new(),
            classes: Vec::new(),
            functions: Vec::new(),
            nominal_bindings: Vec::new(),
            attribute_sites: Vec::new(),
            call_sites: Vec::new(),
            diagnostics: Vec::new(),
        })
    }

    /// Normalize unordered catalogs and unions, and discard precise proposals
    /// in checker-suppressed regions and their affected references. Source
    /// identities, suppression diagnostics, and strict global policy survive;
    /// unrelated classes do not lose their independent eligibility.
    pub fn canonicalized(&self) -> Result<Self, ContractError> {
        let mut facts = self.clone();
        crate::validation::canonicalize_module(&mut facts)?;
        Ok(facts)
    }

    pub fn module_body_identity(&self) -> SourceIdentity {
        SourceIdentity::module_body(self.module.clone(), self.source_size)
    }

    /// Whether this exact locally defined method/property component belongs
    /// to a class already classified as dynamic by source analysis. Such a
    /// framework's methods retain ordinary annotation semantics from creation;
    /// a later runtime-only class decline cannot revoke a selected contract.
    ///
    /// This is source-policy classification, never runtime authority. A
    /// standalone function assigned into a class is not its source-owned
    /// method: catalog identity and immediate containing source scope must
    /// agree, without deriving ownership from names. A nested class or
    /// function scope remains its own lexical owner even if its function is
    /// later assigned into an enclosing dynamic class's descriptor.
    pub fn function_has_statically_dynamic_class_owner(&self, source: &SourceIdentity) -> bool {
        self.source_class_owner(source)
            .is_some_and(|class| matches!(class.participation, ParticipationProposal::Dynamic(_)))
    }

    /// The exact lexical class owning this method/property definition, if any.
    /// Class member aliases do not transfer source ownership. This is static
    /// catalogue classification only, never an actual runtime class witness.
    pub fn source_class_owner(&self, source: &SourceIdentity) -> Option<&ClassTypeFact> {
        if source.module != self.module
            || !matches!(
                source.definition_kind,
                DefinitionKind::Function | DefinitionKind::Lambda
            )
            || !self
                .functions
                .iter()
                .any(|function| &function.identity == source)
        {
            return None;
        }
        self.classes.iter().find(|class| {
            if class.identity.module != source.module
                || class.identity.definition_kind != DefinitionKind::Class
                || class.identity.source_range == source.source_range
                || !class.identity.source_range.contains(source.source_range)
            {
                return false;
            }
            let intervening_scope = |scope: &SourceIdentity| {
                scope.module == source.module
                    && scope.source_range != class.identity.source_range
                    && scope.source_range != source.source_range
                    && class.identity.source_range.contains(scope.source_range)
                    && scope.source_range.contains(source.source_range)
            };
            if self
                .classes
                .iter()
                .any(|nested| intervening_scope(&nested.identity))
                || self
                    .functions
                    .iter()
                    .any(|nested| intervening_scope(&nested.identity))
            {
                return false;
            }
            // A later assignment may overwrite the method before the final
            // member catalogue is produced. That does not turn its earlier
            // lexical definition into a free function.
            true
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinType {
    Object,
    Bool,
    Int,
    Float,
    Complex,
    Str,
    Bytes,
    ByteArray,
    Tuple,
    List,
    Dict,
    Set,
    FrozenSet,
    Type,
}

/// Integer values use canonical decimal strings to retain Python's arbitrary
/// precision. Floating values retain their exact bits, including signed zero.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum LiteralValue {
    None,
    Bool(bool),
    Int(String),
    FloatBits(u64),
    Str(String),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum StaticType {
    None,
    ExactBuiltin(BuiltinType),
    /// An ordinary builtin annotation generally accepts subclasses. In
    /// particular, a nominal int annotation must not exclude bool.
    NominalBuiltin {
        builtin: BuiltinType,
        allow_subclasses: bool,
    },
    /// Typing's numeric acceptance is not an exact native representation.
    NumericWidening {
        target: BuiltinType,
        accepted: BTreeSet<BuiltinType>,
    },
    NominalClass(ClassReference),
    ExactClass(ClassReference),
    Union(Vec<StaticType>),
    Optional(Box<StaticType>),
    Callable(Box<CallableSignature>),
    Literal(LiteralValue),
    TypeVariable(TypeVariableFact),
    StructuralProtocol(ProtocolFact),
    Any,
    Unknown,
    Todo,
    Divergent,
    Unsupported {
        kind: UnsupportedTypeKind,
        reason: UnsupportedReasonCode,
    },
}

impl StaticType {
    /// Canonicalize unions without treating uncertainty as a concrete type
    /// or silently collapsing Any, Unknown, TODO, and divergence together.
    pub fn normalized(&self) -> Result<Self, ContractError> {
        self.normalize_at_depth(0)
    }

    pub fn contains_uncertainty(&self) -> bool {
        match self {
            Self::Any
            | Self::Unknown
            | Self::Todo
            | Self::Divergent
            | Self::Unsupported { .. }
            | Self::TypeVariable(_)
            | Self::StructuralProtocol(_) => true,
            Self::Union(elements) => elements.iter().any(Self::contains_uncertainty),
            Self::Optional(element) => element.contains_uncertainty(),
            Self::Callable(signature) => {
                signature.return_type.contains_uncertainty()
                    || signature
                        .parameters
                        .iter()
                        .any(|parameter| parameter.value_type.contains_uncertainty())
            }
            _ => false,
        }
    }

    /// Whether a runtime value predicate can represent this logical shape.
    /// This does not check values, resolve live classes, or authorize removal
    /// of a runtime check. Mutable container element types are not supported.
    pub fn has_supported_value_shape(&self) -> bool {
        match self {
            Self::None
            | Self::ExactBuiltin(_)
            | Self::NominalBuiltin { .. }
            | Self::NumericWidening { .. }
            | Self::NominalClass(_)
            | Self::ExactClass(_) => true,
            Self::Union(elements) => {
                !elements.is_empty() && elements.iter().all(Self::has_supported_value_shape)
            }
            Self::Optional(element) => element.has_supported_value_shape(),
            _ => false,
        }
    }

    pub(crate) fn normalize_at_depth(&self, depth: usize) -> Result<Self, ContractError> {
        if depth > 64 {
            return Err(ContractError::InvalidType("type nesting exceeds 64".into()));
        }
        match self {
            Self::Optional(element) => {
                Self::Union(vec![(**element).clone(), Self::None]).normalize_at_depth(depth + 1)
            }
            Self::Union(elements) => {
                if elements.is_empty() {
                    return Err(ContractError::InvalidType("empty union".into()));
                }
                let mut normalized = BTreeSet::new();
                for element in elements {
                    match element.normalize_at_depth(depth + 1)? {
                        Self::Union(nested) => normalized.extend(nested),
                        element => {
                            normalized.insert(element);
                        }
                    }
                }
                let mut normalized: Vec<_> = normalized.into_iter().collect();
                if normalized.len() == 1 {
                    Ok(normalized.remove(0))
                } else {
                    Ok(Self::Union(normalized))
                }
            }
            Self::Literal(LiteralValue::None) => Ok(Self::None),
            Self::Callable(signature) => {
                let mut signature = (**signature).clone();
                signature.normalize_at_depth(depth + 1)?;
                Ok(Self::Callable(Box::new(signature)))
            }
            Self::TypeVariable(variable) => {
                let mut variable = variable.clone();
                variable.upper_bound = variable
                    .upper_bound
                    .as_ref()
                    .map(|bound| bound.normalize_at_depth(depth + 1).map(Box::new))
                    .transpose()?;
                variable.constraints = variable
                    .constraints
                    .iter()
                    .map(|constraint| constraint.normalize_at_depth(depth + 1))
                    .collect::<Result<_, _>>()?;
                variable.constraints.sort();
                variable.constraints.dedup();
                Ok(Self::TypeVariable(variable))
            }
            _ => Ok(self.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeVariableFact {
    pub identity: SourceIdentity,
    pub upper_bound: Option<Box<StaticType>>,
    pub constraints: Vec<StaticType>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolFact {
    pub definition: Option<ClassReference>,
    pub runtime_checkable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedTypeKind {
    MutableGeneric,
    GenericAlias,
    Intersection,
    TypeGuard,
    TypeIs,
    TypedDict,
    NewType,
    RecursiveType,
    CustomInstanceCheck,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedReasonCode {
    NoRuntimeEnforcement,
    AliasedMutableContents,
    UnresolvedTypeArguments,
    CheckerNotImplemented,
    UnsafeNarrowing,
    UserCodeRequired,
    UnknownProvenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UncertaintyReason {
    Any,
    Unknown,
    CheckerTodo,
    IgnoredDiagnostic,
    UnresolvedImport,
    DynamicDecorator,
    DynamicMetaclass,
    DynamicDescriptor,
    UnsafeNarrowing,
    UnsupportedType,
    OpenWorld,
    PartialInitialization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationOrigin {
    Explicit,
    Inferred,
    Absent,
    Unresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterKind {
    PositionalOnly,
    PositionalOrKeyword,
    VarArgs,
    KeywordOnly,
    VarKeywords,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterTypeFact {
    pub name: String,
    pub kind: ParameterKind,
    pub value_type: StaticType,
    pub annotation_origin: AnnotationOrigin,
    pub default: DefaultFact,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallableSignature {
    /// In Python binding order, never alphabetically sorted.
    pub parameters: Vec<ParameterTypeFact>,
    pub return_type: StaticType,
    pub return_annotation_origin: AnnotationOrigin,
    pub uncertainty: BTreeSet<UncertaintyReason>,
}

impl CallableSignature {
    pub(crate) fn normalize_at_depth(&mut self, depth: usize) -> Result<(), ContractError> {
        self.return_type = self.return_type.normalize_at_depth(depth + 1)?;
        for parameter in &mut self.parameters {
            parameter.value_type = parameter.value_type.normalize_at_depth(depth + 1)?;
            parameter.default.normalize_at_depth(depth + 1)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum DefaultFact {
    Missing,
    Value {
        value_type: Box<StaticType>,
        literal: Option<LiteralValue>,
    },
    Factory {
        implementation: Option<SourceIdentity>,
        return_type: Box<StaticType>,
    },
    Unknown,
}

impl DefaultFact {
    pub(crate) fn normalize_at_depth(&mut self, depth: usize) -> Result<(), ContractError> {
        match self {
            Self::Value { value_type, .. } => {
                **value_type = value_type.normalize_at_depth(depth + 1)?;
            }
            Self::Factory { return_type, .. } => {
                **return_type = return_type.normalize_at_depth(depth + 1)?;
            }
            Self::Missing | Self::Unknown => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobalMutability {
    FinalAfterSeal,
    ExplicitlyMutable,
    LateAppendOnly,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalBindingFact {
    pub name: String,
    pub mutability: GlobalMutability,
    pub value_type: StaticType,
    pub definition: Option<SourceIdentity>,
    pub uncertainty: BTreeSet<UncertaintyReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FunctionKind {
    Synchronous,
    Coroutine,
    Generator,
    AsyncGenerator,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionTypeFact {
    pub identity: SourceIdentity,
    pub function_kind: FunctionKind,
    pub signature: CallableSignature,
    pub decorators: Vec<DecoratorFact>,
    pub uncertainty: BTreeSet<UncertaintyReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AnnotationTarget {
    /// Index in the source signature's Python binding order.
    Parameter {
        index: u32,
    },
    Return,
}

/// One exact source field declaration. Inheritance preserves the declaring
/// class and annotated assignment; an actual runtime contract owner is still
/// required to distinguish repeated executions of the same source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldReference {
    pub declaring_class: ClassReference,
    pub annotation_definition: SourceIdentity,
    pub name: String,
}

/// The source annotation that owns a nominal leaf. Generated parameters
/// project through their actual field, not an invented source function.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum NominalBindingOwner {
    Function {
        function: SourceIdentity,
        annotation: AnnotationTarget,
    },
    Field {
        field: FieldReference,
    },
}

impl NominalBindingOwner {
    /// Return only genuine source-function annotations; field contracts have
    /// no synthetic parameter or return target.
    pub fn as_function(&self) -> Option<(&SourceIdentity, AnnotationTarget)> {
        match self {
            Self::Function {
                function,
                annotation,
            } => Some((function, *annotation)),
            Self::Field { .. } => None,
        }
    }
}

/// Checker-resolved provenance for one simple-name nominal annotation leaf.
/// This describes a lexical value source, never a runtime class capability.
/// Leaves stay distinct when normalization merges identical ClassReferences:
/// two aliases can refer to different executions of the same class source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NominalBindingFact {
    pub owner: NominalBindingOwner,
    pub expression_range: SourceRange,
    pub name: String,
    pub class: ClassReference,
    /// The unique local binding, preserving import aliases instead of
    /// replacing them with the imported class's definition.
    pub binding: SourceIdentity,
    /// Exact module, function, or class scope that declares `binding`.
    /// Runtime consumers must use the actual corresponding globals/cell/
    /// class-namespace operand; matching a name or source ID is insufficient.
    pub binding_scope: SourceIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DynamicClassReason {
    NonParticipatingMetaclass,
    UnknownDecorator,
    FrameworkManaged,
    UnknownBase,
    MutableBase,
    UnsupportedDescriptor,
    CustomAttributeHooks,
    ConflictingLayout,
    IgnoredDiagnostic,
    UnresolvedAnalysis,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "reasons", rename_all = "snake_case")]
pub enum ParticipationProposal {
    Candidate,
    Dynamic(BTreeSet<DynamicClassReason>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassDictionarySemantics {
    DictionaryBearing,
    ExplicitSlots,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassOpenness {
    OpenSubclassFamily,
    DeclaredFinal,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum MetaclassFact {
    BuiltinType,
    Class(ClassReference),
    Dynamic,
}

/// A resolved logical base identity, not permission to adopt its actual type.
/// Builtins come from the checker's semantic builtin mapping, not a module or
/// class spelling. Runtime construction still validates the actual base value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum BaseReference {
    Class(ClassReference),
    Builtin(BuiltinType),
}

impl BaseReference {
    pub fn as_class(&self) -> Option<&ClassReference> {
        match self {
            Self::Class(class) => Some(class),
            Self::Builtin(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InheritanceFact {
    /// Logical MRO excluding this class. This is not a physical field prefix.
    pub linearized_bases: Vec<BaseReference>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassTypeFact {
    pub identity: SourceIdentity,
    pub bases: Vec<BaseReference>,
    pub metaclass: MetaclassFact,
    /// In application order; decorator order is semantically significant.
    pub decorators: Vec<DecoratorFact>,
    pub participation: ParticipationProposal,
    pub dictionary: ClassDictionarySemantics,
    /// Source/transform field order, never a CPython offset or method slot.
    /// Pseudo-fields retain distinct FieldKind variants and are not storage.
    pub instance_fields: Vec<FieldTypeFact>,
    pub methods: Vec<MethodTypeFact>,
    pub class_members: Vec<ClassMemberFact>,
    pub inheritance: InheritanceFact,
    pub openness: ClassOpenness,
    pub transform: Option<ClassTransformFact>,
    pub uncertainty: BTreeSet<UncertaintyReason>,
}

impl ClassTypeFact {
    /// Exact source declaration membership, not a field-name or qualname
    /// heuristic. A method-only instance annotation is not a class-body field
    /// consumed by dataclasses, even if a different class member shares its name.
    pub fn declares_field_annotation(&self, field: &FieldTypeFact) -> bool {
        field.declaring_class.definition == self.identity
            && field
                .annotation_definition
                .as_ref()
                .is_some_and(|definition| {
                    self.class_members.iter().any(|member| {
                        member.name == field.name && member.definition.as_ref() == Some(definition)
                    })
                })
    }

    /// Own field declarations consumed by selected storage write predicates.
    /// Constructor signatures and InitVars do not select storage requirements.
    /// Inherited checked declarations keep their original owner.
    /// This logical selection never authenticates an actual class or cell.
    pub fn required_field_bindings(&self, policy: &ResolvedStrictPolicy) -> Vec<&FieldTypeFact> {
        self.instance_fields
            .iter()
            .filter(|field| field.declaring_class.definition == self.identity)
            .filter(|field| field.required_write_type(policy.checked_fields).is_some())
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    InstanceField,
    CallableInstanceField,
    ShadowableClassDefault,
    CachedDescriptorField,
    ClassVariable,
    InitOnly,
    FrameworkPrivate,
    Dynamic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldReadPolicy {
    PythonAttribute,
    InstanceThenClassDefault,
    DescriptorFirst,
    CachedDescriptor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldWritePolicy {
    PythonAttribute,
    DeclaredField,
    Descriptor,
    ClassVariableRejected,
    InitOnly,
    Dynamic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InitializationPolicy {
    MayBeAbsent,
    InitializedByConstructor,
    DescriptorManaged,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldTypeFact {
    pub name: String,
    pub declaring_class: ClassReference,
    pub value_type: StaticType,
    /// Origin of this field's value contract, not of a constructor parameter
    /// from which its type happened to be inferred. Only an explicit supported
    /// annotation can select mandatory checked-field enforcement.
    pub annotation_origin: AnnotationOrigin,
    /// The actual annotated assignment, not a generated constructor parameter
    /// or inferred value source. Null explicitly records unavailable or
    /// ambiguous semantic provenance; the field must still be serialized.
    #[serde(deserialize_with = "required_annotation_definition")]
    pub annotation_definition: Option<SourceIdentity>,
    pub field_kind: FieldKind,
    pub read_policy: FieldReadPolicy,
    pub write_policy: FieldWritePolicy,
    pub initialization: InitializationPolicy,
    pub default: DefaultFact,
    pub descriptor: DescriptorFact,
    pub uncertainty: BTreeSet<UncertaintyReason>,
}

fn required_annotation_definition<'de, D>(
    deserializer: D,
) -> Result<Option<SourceIdentity>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<SourceIdentity>::deserialize(deserializer)
}

impl FieldTypeFact {
    /// A selected ordinary instance-storage write predicate, not a layout or
    /// descriptor capability. Pseudo-fields do not become storage by annotation.
    pub fn required_write_type(&self, policy: CheckedFieldPolicy) -> Option<&StaticType> {
        if self.descriptor.kind != DescriptorKind::None
            || !matches!(
                self.field_kind,
                FieldKind::InstanceField
                    | FieldKind::CallableInstanceField
                    | FieldKind::ShadowableClassDefault
            )
        {
            return None;
        }
        policy.required_type(self.annotation_origin, &self.value_type)
    }

    /// This is source provenance only, never a live field-policy capability.
    pub fn annotation_reference(&self) -> Option<FieldReference> {
        Some(FieldReference {
            declaring_class: self.declaring_class.clone(),
            annotation_definition: self.annotation_definition.clone()?,
            name: self.name.clone(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DescriptorKind {
    None,
    Property,
    NonData,
    Data,
    StdlibCachedProperty,
    Unknown,
}

/// The checker's proposed descriptor classification. Runtime code must
/// inspect the actual descriptor type slots; typeshed is not that proof.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DescriptorFact {
    pub kind: DescriptorKind,
    pub descriptor_type: Option<Box<StaticType>>,
    pub getter: Option<SourceIdentity>,
    pub setter: Option<SourceIdentity>,
    pub deleter: Option<SourceIdentity>,
}

impl Default for DescriptorFact {
    fn default() -> Self {
        Self {
            kind: DescriptorKind::None,
            descriptor_type: None,
            getter: None,
            setter: None,
            deleter: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MethodBinding {
    Instance,
    Class,
    Static,
    PropertyGetter,
    Descriptor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverridePolicy {
    CompatibleSignatureRequired,
    DeclaredFinal,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedFunctionFact {
    pub class: ClassReference,
    pub transform: TransformKind,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MethodTypeFact {
    pub name: String,
    pub declaring_class: ClassReference,
    pub binding: MethodBinding,
    pub signature: CallableSignature,
    pub declared_final: bool,
    pub override_policy: OverridePolicy,
    pub implementation: Option<SourceIdentity>,
    pub generated: Option<GeneratedFunctionFact>,
    pub uncertainty: BTreeSet<UncertaintyReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassMemberKind {
    ClassVariable,
    ShadowableDefault,
    NestedClass,
    Descriptor,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassMemberFact {
    pub name: String,
    pub kind: ClassMemberKind,
    pub value_type: StaticType,
    pub definition: Option<SourceIdentity>,
    pub descriptor: DescriptorFact,
    pub uncertainty: BTreeSet<UncertaintyReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecoratorKind {
    StdlibDataclass,
    TypingFinal,
    StaticMethod,
    ClassMethod,
    Property,
    StdlibCachedProperty,
    DataclassTransform,
    Other,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecoratorFact {
    pub kind: DecoratorKind,
    pub expression_range: SourceRange,
    pub definition: Option<SourceIdentity>,
    pub source_digest: Option<Fingerprint>,
    pub arguments: BTreeMap<String, LiteralValue>,
    pub uncertainty: BTreeSet<UncertaintyReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformKind {
    StdlibDataclass,
    DataclassTransform,
    UnsupportedFramework,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataclassOptions {
    pub init: bool,
    pub repr: bool,
    pub eq: bool,
    pub order: bool,
    pub unsafe_hash: bool,
    pub frozen: bool,
    pub match_args: bool,
    pub kw_only: bool,
    pub slots: bool,
    pub weakref_slot: bool,
}

impl Default for DataclassOptions {
    fn default() -> Self {
        Self {
            init: true,
            repr: true,
            eq: true,
            order: false,
            unsafe_hash: false,
            frozen: false,
            match_args: true,
            kw_only: false,
            slots: false,
            weakref_slot: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassTransformFact {
    pub kind: TransformKind,
    pub provenance: Option<SourceIdentity>,
    pub dataclass_options: Option<DataclassOptions>,
    pub generated_methods: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiverTypeFact {
    pub value_type: StaticType,
    pub uncertainty: BTreeSet<UncertaintyReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallBindingFact {
    UnboundFunction,
    BoundInstanceMethod,
    BoundClassMethod,
    StaticMethod,
    CallableInstanceField,
    Descriptor,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum CallableTargetFact {
    SourceFunction(SourceIdentity),
    Method {
        class: ClassReference,
        name: String,
        implementation: Option<SourceIdentity>,
    },
    Generated(GeneratedFunctionFact),
    Dynamic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallUncertainty {
    ExactStaticTarget,
    OpenSubclassFamily,
    FiniteUnion,
    CallableInstanceField,
    CustomDescriptor,
    StructuralProtocol,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallSiteFact {
    pub identity: CallSiteIdentity,
    pub receiver: Option<ReceiverTypeFact>,
    pub attribute_name: Option<String>,
    pub candidate_targets: Vec<CallableTargetFact>,
    pub binding: CallBindingFact,
    pub signature: CallableSignature,
    pub result_type: StaticType,
    pub uncertainty: CallUncertainty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributeAccess {
    Read,
    Write,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttributeSiteFact {
    pub identity: AttributeSiteIdentity,
    pub name: String,
    pub access: AttributeAccess,
    pub receiver_type: StaticType,
    pub value_type: Option<StaticType>,
    pub declaring_class: Option<ClassReference>,
    pub uncertainty: BTreeSet<UncertaintyReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticCode {
    StrictFinalGlobalRebind,
    StrictFinalGlobalDelete,
    StrictClassMutation,
    StrictFinalClassSubclass,
    StrictFinalMethodOverride,
    StrictInstanceMethodShadow,
    StrictClassvarInstanceWrite,
    StrictUndeclaredField,
    StrictIncompatibleFieldWrite,
    StrictIncompatibleOverride,
    StrictDictReplacement,
    StrictUnsupportedMetaclass,
    StrictUnsupportedDecorator,
    StrictUnsupportedDescriptor,
    StrictLayoutInheritanceConflict,
    StrictUncheckedDynamicType,
    StrictConstructionContractMismatch,
    CheckerError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Information,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum DiagnosticScope {
    Module,
    Definition(SourceIdentity),
    Site(SourceRange),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrictDiagnostic {
    pub code: DiagnosticCode,
    pub severity: DiagnosticSeverity,
    pub source_range: SourceRange,
    pub scope: DiagnosticScope,
    pub related_definitions: Vec<SourceIdentity>,
    pub suppressed: bool,
    /// Diagnostic prose is never parsed to recover semantic facts.
    pub message: String,
}
