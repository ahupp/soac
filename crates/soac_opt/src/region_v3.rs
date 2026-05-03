use soac_core::block_py::{
    BinOpKind, Block, BlockLabel, BlockPyFunction, BlockTerm, HasSemanticInstrId, InstrId, Load,
    ResolvedName, TermIf, UnaryOpKind,
};
use soac_ir_blockpy::{BlockPyModuleShape, InstrBlockPy};
use soac_ir_typed::plan_v3::RegionId;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedRegion {
    pub id: RegionId,
    pub block: BlockLabel,
    pub block_body_len: usize,
    pub store: Option<ExtractedStoreContext>,
    pub values: Vec<ExtractedValue>,
    pub exit: ExtractedExit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedStoreContext {
    pub target: ResolvedName,
    pub continuation: Option<BlockLabel>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedValue {
    pub id: ExtractedValueId,
    pub source: Option<InstrId>,
    pub kind: ExtractedValueKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExtractedValueId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtractedValueKind {
    LoadName {
        name: ResolvedName,
    },
    Binary {
        op: BinOpKind,
        left: ExtractedValueId,
        right: ExtractedValueId,
    },
    GetAttr {
        value: ExtractedValueId,
        attr: ExtractedValueId,
    },
    Truthiness {
        value: ExtractedValueId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtractedExit {
    Branch {
        source: Option<InstrId>,
        condition: ExtractedValueId,
        then_label: BlockLabel,
        else_label: BlockLabel,
    },
    Return {
        source: Option<InstrId>,
        value: ExtractedValueId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionExtractionAttempt {
    pub block: BlockLabel,
    pub result: Result<ExtractedRegion, RegionExtractionError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegionExtractionError {
    BlockHasBody {
        block: BlockLabel,
        len: usize,
    },
    BlockHasExceptionEdge {
        block: BlockLabel,
    },
    UnsupportedTerm {
        block: BlockLabel,
        term: &'static str,
    },
    UnsupportedInstr {
        source: Option<InstrId>,
        kind: &'static str,
    },
}

impl fmt::Display for RegionExtractionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlockHasBody { block, len } => {
                write!(f, "block {block} has {len} body instructions")
            }
            Self::BlockHasExceptionEdge { block } => {
                write!(f, "block {block} has an exception edge")
            }
            Self::UnsupportedTerm { block, term } => {
                write!(f, "block {block} has unsupported terminator {term}")
            }
            Self::UnsupportedInstr { source, kind } => {
                write!(f, "unsupported instruction {kind} at {source:?}")
            }
        }
    }
}

impl std::error::Error for RegionExtractionError {}

pub fn extract_function_regions_v3(
    function: &BlockPyFunction<BlockPyModuleShape>,
) -> Vec<RegionExtractionAttempt> {
    let mut attempts = Vec::new();
    let mut next_region_id = 0;
    for block in &function.blocks {
        for instr in &block.body {
            attempts.push(RegionExtractionAttempt {
                block: block.label,
                result: extract_block_body_instr_region_v3(
                    block,
                    instr,
                    next_primary_region_id(&mut next_region_id),
                ),
            });
        }
        attempts.push(RegionExtractionAttempt {
            block: block.label,
            result: extract_block_term_region_v3(
                block,
                next_primary_region_id(&mut next_region_id),
            ),
        });
    }
    attempts
}

pub fn extract_block_region_v3(
    block: &Block<InstrBlockPy>,
    id: RegionId,
) -> Result<ExtractedRegion, RegionExtractionError> {
    if !block.body.is_empty() {
        return Err(RegionExtractionError::BlockHasBody {
            block: block.label,
            len: block.body.len(),
        });
    }
    if block.exc_edge.is_some() {
        return Err(RegionExtractionError::BlockHasExceptionEdge { block: block.label });
    }

    extract_block_term_region_v3(block, id)
}

fn next_primary_region_id(next: &mut u32) -> RegionId {
    let id = RegionId(*next);
    *next += 2;
    id
}

fn extract_block_body_instr_region_v3(
    block: &Block<InstrBlockPy>,
    instr: &InstrBlockPy,
    id: RegionId,
) -> Result<ExtractedRegion, RegionExtractionError> {
    if block.exc_edge.is_some() {
        return Err(RegionExtractionError::BlockHasExceptionEdge { block: block.label });
    }

    let InstrBlockPy::Store(store) = instr else {
        return Err(unsupported_instr_error(instr));
    };
    let mut builder = RegionBuilder::new(id, block.label);
    let value = builder.linearize_instr(&store.value)?;
    let exit = ExtractedExit::Return {
        source: value_source(&builder.values, value),
        value,
    };
    Ok(ExtractedRegion {
        id,
        block: block.label,
        block_body_len: block.body.len(),
        store: Some(ExtractedStoreContext {
            target: store.name.clone(),
            continuation: block_jump_continuation(block),
        }),
        values: builder.values,
        exit,
    })
}

fn block_jump_continuation(block: &Block<InstrBlockPy>) -> Option<BlockLabel> {
    match &block.term {
        BlockTerm::Jump(edge) if edge.args.is_empty() => Some(edge.target),
        _ => None,
    }
}

fn unsupported_instr_error(instr: &InstrBlockPy) -> RegionExtractionError {
    RegionExtractionError::UnsupportedInstr {
        source: instr.try_semantic_instr_id(),
        kind: instr_codegen_kind(instr),
    }
}

fn instr_codegen_kind(instr: &InstrBlockPy) -> &'static str {
    match instr {
        InstrBlockPy::BinOp(_) => "BinOp",
        InstrBlockPy::UnaryOp(_) => "UnaryOp",
        InstrBlockPy::Tuple(_) => "Tuple",
        InstrBlockPy::Call(_) => "Call",
        InstrBlockPy::GetAttr(_) => "GetAttr",
        InstrBlockPy::SetAttr(_) => "SetAttr",
        InstrBlockPy::GetItem(_) => "GetItem",
        InstrBlockPy::SetItem(_) => "SetItem",
        InstrBlockPy::DelItem(_) => "DelItem",
        InstrBlockPy::Load(_) => "Load",
        InstrBlockPy::Store(_) => "Store",
        InstrBlockPy::Del(_) => "Del",
        InstrBlockPy::MakeCell(_) => "MakeCell",
        InstrBlockPy::IncrementCounter(_) => "IncrementCounter",
        InstrBlockPy::CellRef(_) => "CellRef",
        InstrBlockPy::MakeFunctionWithClosure(_) => "MakeFunctionWithClosure",
    }
}

fn extract_block_term_region_v3(
    block: &Block<InstrBlockPy>,
    id: RegionId,
) -> Result<ExtractedRegion, RegionExtractionError> {
    if block.exc_edge.is_some() {
        return Err(RegionExtractionError::BlockHasExceptionEdge { block: block.label });
    }

    let mut builder = RegionBuilder::new(id, block.label);
    let exit = match &block.term {
        BlockTerm::IfTerm(term) => builder.extract_if_term(term)?,
        BlockTerm::Return(value) => {
            let value = builder.linearize_instr(value)?;
            ExtractedExit::Return {
                source: value_source(&builder.values, value),
                value,
            }
        }
        BlockTerm::Jump(_) => {
            return Err(RegionExtractionError::UnsupportedTerm {
                block: block.label,
                term: "Jump",
            });
        }
        BlockTerm::BranchTable(_) => {
            return Err(RegionExtractionError::UnsupportedTerm {
                block: block.label,
                term: "BranchTable",
            });
        }
        BlockTerm::Raise(_) => {
            return Err(RegionExtractionError::UnsupportedTerm {
                block: block.label,
                term: "Raise",
            });
        }
    };
    Ok(ExtractedRegion {
        id,
        block: block.label,
        block_body_len: block.body.len(),
        store: None,
        values: builder.values,
        exit,
    })
}

struct RegionBuilder {
    next_value: u32,
    values: Vec<ExtractedValue>,
}

impl RegionBuilder {
    fn new(_id: RegionId, _block: BlockLabel) -> Self {
        Self {
            next_value: 0,
            values: Vec::new(),
        }
    }

    fn extract_if_term(
        &mut self,
        term: &TermIf<InstrBlockPy>,
    ) -> Result<ExtractedExit, RegionExtractionError> {
        let test = self.linearize_instr(&term.test)?;
        let condition = self.push(
            term.test.try_semantic_instr_id(),
            ExtractedValueKind::Truthiness { value: test },
        );
        Ok(ExtractedExit::Branch {
            source: term.test.try_semantic_instr_id(),
            condition,
            then_label: term.then_label,
            else_label: term.else_label,
        })
    }

    fn linearize_instr(
        &mut self,
        instr: &InstrBlockPy,
    ) -> Result<ExtractedValueId, RegionExtractionError> {
        match instr {
            InstrBlockPy::Load(load) => {
                Ok(self.linearize_load(load, instr.try_semantic_instr_id()))
            }
            InstrBlockPy::BinOp(op) => {
                let left = self.linearize_instr(&op.left)?;
                let right = self.linearize_instr(&op.right)?;
                Ok(self.push(
                    instr.try_semantic_instr_id(),
                    ExtractedValueKind::Binary {
                        op: op.kind,
                        left,
                        right,
                    },
                ))
            }
            InstrBlockPy::UnaryOp(op) if op.kind == UnaryOpKind::Truth => {
                let value = self.linearize_instr(&op.operand)?;
                Ok(self.push(
                    instr.try_semantic_instr_id(),
                    ExtractedValueKind::Truthiness { value },
                ))
            }
            InstrBlockPy::Tuple(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "Tuple",
            }),
            InstrBlockPy::UnaryOp(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "UnaryOp",
            }),
            InstrBlockPy::Call(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "Call",
            }),
            InstrBlockPy::GetAttr(op) => {
                let value = self.linearize_instr(&op.value)?;
                let attr = self.linearize_instr(&op.attr)?;
                Ok(self.push(
                    instr.try_semantic_instr_id(),
                    ExtractedValueKind::GetAttr { value, attr },
                ))
            }
            InstrBlockPy::SetAttr(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "SetAttr",
            }),
            InstrBlockPy::GetItem(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "GetItem",
            }),
            InstrBlockPy::SetItem(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "SetItem",
            }),
            InstrBlockPy::DelItem(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "DelItem",
            }),
            InstrBlockPy::Store(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "Store",
            }),
            InstrBlockPy::Del(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "Del",
            }),
            InstrBlockPy::MakeCell(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "MakeCell",
            }),
            InstrBlockPy::IncrementCounter(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "IncrementCounter",
            }),
            InstrBlockPy::CellRef(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "CellRef",
            }),
            InstrBlockPy::MakeFunctionWithClosure(_) => {
                Err(RegionExtractionError::UnsupportedInstr {
                    source: instr.try_semantic_instr_id(),
                    kind: "MakeFunctionWithClosure",
                })
            }
        }
    }

    fn linearize_load(
        &mut self,
        load: &Load<InstrBlockPy>,
        source: Option<InstrId>,
    ) -> ExtractedValueId {
        self.push(
            source,
            ExtractedValueKind::LoadName {
                name: load.name.clone(),
            },
        )
    }

    fn push(&mut self, source: Option<InstrId>, kind: ExtractedValueKind) -> ExtractedValueId {
        let value = ExtractedValueId(self.next_value);
        self.next_value += 1;
        self.values.push(ExtractedValue {
            id: value,
            source,
            kind,
        });
        value
    }
}

fn value_source(values: &[ExtractedValue], value: ExtractedValueId) -> Option<InstrId> {
    values
        .iter()
        .find(|entry| entry.id == value)
        .and_then(|entry| entry.source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_core::block_py::{
        BinOp, BlockEdge, BlockParam, BlockPyName, FunctionName, LocalLocation, Meta,
        ModuleNameGen, NameLocation, ParamSpec, Store, TermIf, Tuple, UnaryOp, WithMeta,
    };

    fn label(index: usize) -> BlockLabel {
        BlockLabel::from_index(index)
    }

    fn instr_id(index: u32) -> InstrId {
        InstrId::new(index)
    }

    fn instr_id_in_label(_block: BlockLabel, index: u32) -> InstrId {
        InstrId::new(index)
    }

    fn with_instr_id(instr: InstrBlockPy, index: u32) -> InstrBlockPy {
        with_instr_id_in_label(instr, label(0), index)
    }

    fn with_instr_id_in_label(instr: InstrBlockPy, block: BlockLabel, index: u32) -> InstrBlockPy {
        instr.with_meta(Meta {
            instr_id: Some(instr_id_in_label(block, index)),
            ..Meta::synthetic()
        })
    }

    fn local(name: &str, slot: u32) -> InstrBlockPy {
        InstrBlockPy::Load(Load::new(ResolvedName {
            id: BlockPyName::new(name),
            location: NameLocation::Local(LocalLocation(slot)),
        }))
    }

    fn binary(op: BinOpKind, left: InstrBlockPy, right: InstrBlockPy, id: u32) -> InstrBlockPy {
        with_instr_id(InstrBlockPy::BinOp(BinOp::new(op, left, right)), id)
    }

    fn branch_block(test: InstrBlockPy) -> Block<InstrBlockPy> {
        Block::new(
            label(0),
            Vec::new(),
            BlockTerm::IfTerm(TermIf {
                test,
                then_label: label(1),
                else_label: label(2),
            }),
            Vec::<BlockParam>::new(),
            None,
        )
    }

    fn test_function(blocks: Vec<Block<InstrBlockPy>>) -> BlockPyFunction<BlockPyModuleShape> {
        let name_gen = ModuleNameGen::new(0).next_function_name_gen();
        BlockPyFunction {
            function_id: name_gen.function_id(),
            name_gen,
            names: FunctionName::new("f", "f", "f", "f"),
            kind: soac_core::block_py::FunctionKind::Function,
            execution_mode: Default::default(),
            params: ParamSpec::default(),
            blocks,
            doc: None,
            storage_layout: None,
            scope: Default::default(),
        }
    }

    #[test]
    fn extracts_branch_expression_in_evaluation_order() {
        let add = binary(
            BinOpKind::Add,
            with_instr_id(local("a", 0), 0),
            with_instr_id(local("b", 1), 1),
            2,
        );
        let test = binary(BinOpKind::Gt, add, with_instr_id(local("zero", 2), 3), 4);
        let region = extract_block_region_v3(&branch_block(test), RegionId(7)).unwrap();

        assert_eq!(region.id, RegionId(7));
        assert_eq!(region.values.len(), 6);
        assert!(matches!(
            region.values[0].kind,
            ExtractedValueKind::LoadName { .. }
        ));
        assert!(matches!(
            region.values[1].kind,
            ExtractedValueKind::LoadName { .. }
        ));
        assert_eq!(
            region.values[2].kind,
            ExtractedValueKind::Binary {
                op: BinOpKind::Add,
                left: ExtractedValueId(0),
                right: ExtractedValueId(1),
            }
        );
        assert!(matches!(
            region.values[3].kind,
            ExtractedValueKind::LoadName { .. }
        ));
        assert_eq!(
            region.values[4].kind,
            ExtractedValueKind::Binary {
                op: BinOpKind::Gt,
                left: ExtractedValueId(2),
                right: ExtractedValueId(3),
            }
        );
        assert_eq!(
            region.values[5].kind,
            ExtractedValueKind::Truthiness {
                value: ExtractedValueId(4),
            }
        );
        assert_eq!(
            region.exit,
            ExtractedExit::Branch {
                source: Some(instr_id(4)),
                condition: ExtractedValueId(5),
                then_label: label(1),
                else_label: label(2),
            }
        );
    }

    #[test]
    fn extracts_return_expression() {
        let value = binary(
            BinOpKind::Add,
            with_instr_id(local("a", 0), 0),
            with_instr_id(local("b", 1), 1),
            2,
        );
        let block = Block::new(
            label(0),
            Vec::new(),
            BlockTerm::Return(value),
            Vec::<BlockParam>::new(),
            None,
        );
        let region = extract_block_region_v3(&block, RegionId(0)).unwrap();

        assert_eq!(region.values.len(), 3);
        assert_eq!(
            region.exit,
            ExtractedExit::Return {
                source: Some(instr_id(2)),
                value: ExtractedValueId(2),
            }
        );
    }

    #[test]
    fn function_extraction_includes_store_rhs_and_later_terminator() {
        let c = ResolvedName {
            id: BlockPyName::new("c"),
            location: NameLocation::Local(LocalLocation(2)),
        };
        let add = binary(
            BinOpKind::Add,
            with_instr_id(local("a", 0), 0),
            with_instr_id(local("b", 1), 1),
            2,
        );
        let entry = Block::new(
            label(0),
            vec![with_instr_id(
                InstrBlockPy::Store(Store::new(c.clone(), add)),
                3,
            )],
            BlockTerm::Jump(BlockEdge::new(label(1))),
            Vec::<BlockParam>::new(),
            None,
        );
        let test_label = label(1);
        let compare = with_instr_id_in_label(
            InstrBlockPy::BinOp(BinOp::new(
                BinOpKind::Gt,
                with_instr_id_in_label(InstrBlockPy::Load(Load::new(c.clone())), test_label, 4),
                with_instr_id_in_label(local("zero", 3), test_label, 5),
            )),
            test_label,
            6,
        );
        let test = Block::new(
            test_label,
            Vec::new(),
            BlockTerm::IfTerm(TermIf {
                test: compare,
                then_label: label(2),
                else_label: label(3),
            }),
            Vec::<BlockParam>::new(),
            None,
        );

        let attempts = extract_function_regions_v3(&test_function(vec![entry, test]));

        assert_eq!(attempts.len(), 3);
        let store_rhs = attempts[0].result.as_ref().unwrap();
        assert_eq!(store_rhs.id, RegionId(0));
        assert_eq!(
            store_rhs.store,
            Some(ExtractedStoreContext {
                target: c.clone(),
                continuation: Some(label(1)),
            })
        );
        assert_eq!(
            store_rhs.exit,
            ExtractedExit::Return {
                source: Some(instr_id(2)),
                value: ExtractedValueId(2),
            }
        );
        assert_eq!(
            attempts[1].result.as_ref().unwrap_err().to_string(),
            "block bb0 has unsupported terminator Jump"
        );
        let branch = attempts[2].result.as_ref().unwrap();
        assert_eq!(branch.id, RegionId(4));
        assert!(matches!(branch.exit, ExtractedExit::Branch { .. }));
    }

    #[test]
    fn rejects_blocks_with_body_instructions() {
        let block = Block::new(
            label(0),
            vec![local("side_effect", 3)],
            BlockTerm::Return(local("a", 0)),
            Vec::<BlockParam>::new(),
            None,
        );
        let err = extract_block_region_v3(&block, RegionId(0)).unwrap_err();
        assert_eq!(
            err,
            RegionExtractionError::BlockHasBody {
                block: label(0),
                len: 1,
            }
        );
    }

    #[test]
    fn rejects_unsupported_expression_kinds() {
        let tuple = with_instr_id(InstrBlockPy::Tuple(Tuple::new(Vec::new())), 0);
        let err = extract_block_region_v3(&branch_block(tuple), RegionId(0)).unwrap_err();
        assert_eq!(
            err,
            RegionExtractionError::UnsupportedInstr {
                source: Some(instr_id(0)),
                kind: "Tuple",
            }
        );
    }

    #[test]
    fn rejects_exception_edges() {
        let block = Block::new(
            label(0),
            Vec::new(),
            BlockTerm::Return(local("a", 0)),
            Vec::<BlockParam>::new(),
            Some(BlockEdge::new(label(9))),
        );
        let err = extract_block_region_v3(&block, RegionId(0)).unwrap_err();
        assert_eq!(
            err,
            RegionExtractionError::BlockHasExceptionEdge { block: label(0) }
        );
    }

    #[test]
    fn extracts_explicit_truth_unary_before_branch_truthiness() {
        let truth = with_instr_id(
            InstrBlockPy::UnaryOp(UnaryOp::new(
                UnaryOpKind::Truth,
                with_instr_id(local("value", 0), 0),
            )),
            1,
        );
        let region = extract_block_region_v3(&branch_block(truth), RegionId(0)).unwrap();

        assert_eq!(
            region.values[1].kind,
            ExtractedValueKind::Truthiness {
                value: ExtractedValueId(0),
            }
        );
        assert_eq!(
            region.values[2].kind,
            ExtractedValueKind::Truthiness {
                value: ExtractedValueId(1),
            }
        );
    }
}
