//! Explicit privileged call edges of the pinned standard-library producer.
//!
//! Locations are a versioned protocol manifest, not authority. Cold capture
//! resolves each against the complete native-build recipe and actual code;
//! native callbacks additionally check the executed code, actual callee and
//! role-specific operands. Calls absent here are ordinary, including callbacks
//! dispatched by C helpers. A missing/ambiguous edge declines the catalog.

use super::catalog::Helper;
use super::code::CallSpan;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub(super) enum Template {
    DataclassWrapper,
    AnnotationProvider,
    ReprDecorator,
    ReprWrapper,
}

impl Template {
    pub(super) const ALL: &[Self] = &[
        Self::DataclassWrapper,
        Self::AnnotationProvider,
        Self::ReprDecorator,
        Self::ReprWrapper,
    ];

    pub(super) fn parent_helper(self) -> Helper {
        match self {
            Self::DataclassWrapper => Helper::Dataclass,
            Self::AnnotationProvider => Helper::MakeAnnotate,
            Self::ReprDecorator | Self::ReprWrapper => Helper::RecursiveRepr,
        }
    }

    pub(super) fn qualname(self) -> &'static str {
        match self {
            Self::DataclassWrapper => "dataclass.<locals>.wrap",
            Self::AnnotationProvider => "_make_annotate_function.<locals>.__annotate__",
            Self::ReprDecorator => "recursive_repr.<locals>.decorating_function",
            Self::ReprWrapper => "recursive_repr.<locals>.decorating_function.<locals>.wrapper",
        }
    }

    pub(super) fn producer(self) -> CodeRole {
        match self {
            Self::DataclassWrapper => CodeRole::Helper(Helper::Dataclass),
            Self::AnnotationProvider => CodeRole::Helper(Helper::MakeAnnotate),
            Self::ReprDecorator => CodeRole::Helper(Helper::RecursiveRepr),
            Self::ReprWrapper => CodeRole::Template(Self::ReprDecorator),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CodeRole {
    Helper(Helper),
    Template(Template),
}

/// The operation also selects the required operand validator. Two calls to
/// add_fn from the same producer do not share a generic "builder allowed"
/// capability: init/repr/comparison/frozen-hook recipes are distinct roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub(super) enum Edge {
    BareDataclassApply,
    ProcessClass,
    PrepareInit,
    PrepareRepr,
    PrepareEquality,
    PrepareOrdering,
    PrepareFrozen,
    PrepareHash,
    InstallMethods,
    InstallReplace,
    InstallMatchArgs,
    AddInit,
    AddFrozenSetattr,
    AddFrozenDelattr,
    AddHash,
    PrepareField,
    RecordSource,
    ExecuteSource,
    InvokeFactory,
    MakeAnnotations,
    InstallUnconditional,
    InstallConditional,
    SetMember,
    PrepareSlots,
    NewSlots,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    Code(CodeRole),
    HashAction,
    GeneratedFactory,
    SourceBridge,
    ExecBridge,
    MemberBridge,
    SlotsBridge,
}

impl Edge {
    pub(super) const ALL: &[Self] = &[
        Self::BareDataclassApply,
        Self::ProcessClass,
        Self::PrepareInit,
        Self::PrepareRepr,
        Self::PrepareEquality,
        Self::PrepareOrdering,
        Self::PrepareFrozen,
        Self::PrepareHash,
        Self::InstallMethods,
        Self::InstallReplace,
        Self::InstallMatchArgs,
        Self::AddInit,
        Self::AddFrozenSetattr,
        Self::AddFrozenDelattr,
        Self::AddHash,
        Self::PrepareField,
        Self::RecordSource,
        Self::ExecuteSource,
        Self::InvokeFactory,
        Self::MakeAnnotations,
        Self::InstallUnconditional,
        Self::InstallConditional,
        Self::SetMember,
        Self::PrepareSlots,
        Self::NewSlots,
    ];

    pub(super) fn producer(self) -> CodeRole {
        CodeRole::Helper(match self {
            Self::BareDataclassApply => Helper::Dataclass,
            Self::ProcessClass => return CodeRole::Template(Template::DataclassWrapper),
            Self::PrepareInit
            | Self::PrepareRepr
            | Self::PrepareEquality
            | Self::PrepareOrdering
            | Self::PrepareFrozen
            | Self::PrepareHash
            | Self::InstallMethods
            | Self::InstallReplace
            | Self::InstallMatchArgs
            | Self::PrepareSlots => Helper::ProcessClass,
            Self::AddInit | Self::PrepareField => Helper::Init,
            Self::AddFrozenSetattr | Self::AddFrozenDelattr => Helper::Frozen,
            Self::AddHash => Helper::HashAdd,
            Self::RecordSource => Helper::BuilderAdd,
            Self::ExecuteSource
            | Self::InvokeFactory
            | Self::MakeAnnotations
            | Self::InstallUnconditional
            | Self::InstallConditional => Helper::BuilderInstall,
            Self::SetMember => Helper::SetNewAttribute,
            Self::NewSlots => Helper::AddSlots,
        })
    }

    pub(super) fn target(self) -> Target {
        Target::Code(CodeRole::Helper(match self {
            Self::BareDataclassApply => {
                return Target::Code(CodeRole::Template(Template::DataclassWrapper));
            }
            Self::ProcessClass => Helper::ProcessClass,
            Self::PrepareInit => Helper::Init,
            Self::PrepareFrozen => Helper::Frozen,
            Self::PrepareRepr
            | Self::PrepareEquality
            | Self::PrepareOrdering
            | Self::AddInit
            | Self::AddFrozenSetattr
            | Self::AddFrozenDelattr
            | Self::AddHash => Helper::BuilderAdd,
            Self::PrepareHash => return Target::HashAction,
            Self::InstallMethods => Helper::BuilderInstall,
            Self::InstallReplace | Self::InstallMatchArgs | Self::InstallConditional => {
                Helper::SetNewAttribute
            }
            Self::MakeAnnotations => Helper::MakeAnnotate,
            Self::PrepareField => Helper::FieldInit,
            Self::RecordSource => return Target::SourceBridge,
            Self::ExecuteSource => return Target::ExecBridge,
            Self::InvokeFactory => return Target::GeneratedFactory,
            Self::InstallUnconditional | Self::SetMember => return Target::MemberBridge,
            Self::PrepareSlots => Helper::AddSlots,
            Self::NewSlots => return Target::SlotsBridge,
        }))
    }

    pub(super) fn span(self) -> CallSpan {
        let [start, end, column, end_column] = match self {
            Self::BareDataclassApply => [28, 28, 11, 20],
            Self::ProcessClass => [1, 3, 15, 43],
            Self::PrepareInit => [150, 162, 8, 18],
            Self::PrepareRepr => [172, 178, 8, 72],
            Self::PrepareEquality => [186, 192, 8, 57],
            Self::PrepareOrdering => [208, 213, 12, 86],
            Self::PrepareFrozen => [216, 216, 8, 59],
            Self::PrepareHash => [224, 224, 23, 65],
            Self::InstallMethods => [229, 229, 4, 38],
            Self::InstallReplace => [164, 164, 4, 52],
            Self::InstallMatchArgs => [246, 247, 8, 66],
            Self::AddInit => [50, 55, 4, 60],
            Self::AddFrozenSetattr => [7, 13, 4, 45],
            Self::AddFrozenDelattr => [14, 20, 4, 45],
            Self::AddHash => [3, 6, 4, 47],
            Self::PrepareField => [27, 27, 15, 63],
            Self::RecordSource => [30, 31, 24, 84],
            Self::ExecuteSource => [28, 28, 8, 52],
            Self::InvokeFactory => [29, 29, 14, 48],
            Self::MakeAnnotations => [40, 40, 30, 96],
            Self::InstallUnconditional => [44, 44, 16, 61],
            Self::InstallConditional => [46, 46, 33, 66],
            Self::SetMember => [5, 5, 4, 52],
            Self::PrepareSlots => [253, 253, 14, 59],
            Self::NewSlots => [33, 33, 13, 88],
        };
        CallSpan::new(start, end, column, end_column)
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ResolvedEdge {
    pub(super) operation: Edge,
    pub(super) code_unit_offset: usize,
}
