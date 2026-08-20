use soac_core::block_py::InstrId;

/// The canonical native operation, not a Python module/global lookup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeIteratorStage {
    Map,
    Filter,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeIteratorMaterializer {
    List,
    Tuple,
}

/// Exact builtin object identities checked against the evaluated source callee.
/// These names must never be resolved through `soac.runtime` or builtins dicts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeIteratorBuiltin {
    Map,
    Filter,
    List,
    Tuple,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeIteratorCallee {
    Materializer,
    Stage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeIteratorCalleeGuard {
    pub operand: NativeIteratorCallee,
    pub expected: NativeIteratorBuiltin,
}

/// A closed wrapper's only use in the current typed program.
///
/// Version one accepts the direct positional argument edge, not aliases inferred
/// from spelling or a proposed future rewrite. The validator must establish the
/// unique allocation origin and this sole use before eliminating the wrapper.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeIteratorMustEliminate {
    pub wrapper_origin: InstrId,
    pub consumer: InstrId,
    pub argument_index: u32,
}

/// A complete inline-only native-iterator template selected for one source call.
///
/// This is plan data, not callable-admission authority. Selection and emission
/// must validate it against the actual root/child calls and function-wide use
/// facts. There are no helper function IDs, source resume targets, or private
/// generator-state operands: the input remains an ordinary native iterator.
///
/// Version one's fixed CFG has these ownership/error phases:
/// 1. Evaluate materializer, stage, callback and iterable in source order. Guard
///    the two evaluated callees before iterator acquisition. The cold fallback
///    reuses those values and executes the original two calls exactly once.
/// 2. Acquire the iterator once, then retire construction-only operands. An
///    acquisition error propagates, including StopIteration. Do not precheck
///    callback callability.
/// 3. Create the materializer state (list capacity eight; tuple's eight owned
///    stack items followed by the native list buffer). Advance the native input
///    and apply the ordinary callback/truth operation without entering a Python
///    handled-exception region.
/// 4. Retire map's input after its callback; retire filter's predicate before
///    delivering or dropping the item. Only then may iteration StopIteration be
///    cleared. Materializer failures retire their partial result before stage
///    owners. List completion uses native capacity/shrink behavior.
/// 5. Finish the result, then retire the virtual wrapper's native owners: map
///    releases iterator before callback; filter releases callback before iterator.
///
/// Codegen expands this selected template mechanically. It must not discover a
/// different operation, use graph, ownership policy, or helper body at emission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedNativeIteratorPipelinePlan {
    pub source: InstrId,
    pub stage_source: InstrId,
    pub template_version: u32,
    pub stage: NativeIteratorStage,
    pub materializer: NativeIteratorMaterializer,
    pub entry_guards: [NativeIteratorCalleeGuard; 2],
    pub must_eliminate: NativeIteratorMustEliminate,
}

impl TypedNativeIteratorPipelinePlan {
    pub const TEMPLATE_VERSION: u32 = 1;

    /// Construct a proposal. This does not validate or commit it.
    pub fn proposal(
        source: InstrId,
        stage_source: InstrId,
        stage: NativeIteratorStage,
        materializer: NativeIteratorMaterializer,
    ) -> Self {
        let stage_builtin = match stage {
            NativeIteratorStage::Map => NativeIteratorBuiltin::Map,
            NativeIteratorStage::Filter => NativeIteratorBuiltin::Filter,
        };
        let materializer_builtin = match materializer {
            NativeIteratorMaterializer::List => NativeIteratorBuiltin::List,
            NativeIteratorMaterializer::Tuple => NativeIteratorBuiltin::Tuple,
        };
        Self {
            source,
            stage_source,
            template_version: Self::TEMPLATE_VERSION,
            stage,
            materializer,
            entry_guards: [
                NativeIteratorCalleeGuard {
                    operand: NativeIteratorCallee::Materializer,
                    expected: materializer_builtin,
                },
                NativeIteratorCalleeGuard {
                    operand: NativeIteratorCallee::Stage,
                    expected: stage_builtin,
                },
            ],
            must_eliminate: NativeIteratorMustEliminate {
                wrapper_origin: stage_source,
                consumer: source,
                argument_index: 0,
            },
        }
    }
}
