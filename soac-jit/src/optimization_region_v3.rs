use crate::optimization_plan_v3::RegionId;
use soac_core::block_py::{
    BinOpKind, Block, BlockLabel, BlockPyFunction, BlockTerm, HasSemanticInstrId, InstrId, Load,
    ResolvedName, TermIf, UnaryOpKind,
};
use soac_lowering::passes::{CodegenModuleShape, InstrCodegen, InstrCodegenOp};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedRegion {
    pub id: RegionId,
    pub block: BlockLabel,
    pub values: Vec<ExtractedValue>,
    pub exit: ExtractedExit,
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
    function: &BlockPyFunction<CodegenModuleShape>,
) -> Vec<RegionExtractionAttempt> {
    function
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| RegionExtractionAttempt {
            block: block.label,
            result: extract_block_region_v3(block, RegionId(index as u32)),
        })
        .collect()
}

pub fn extract_block_region_v3(
    block: &Block<InstrCodegen>,
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
        term: &TermIf<InstrCodegen>,
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
        instr: &InstrCodegen,
    ) -> Result<ExtractedValueId, RegionExtractionError> {
        match instr {
            InstrCodegenOp::Load(load) => {
                Ok(self.linearize_load(load, instr.try_semantic_instr_id()))
            }
            InstrCodegenOp::BinOp(op) => {
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
            InstrCodegenOp::UnaryOp(op) if op.kind == UnaryOpKind::Truth => {
                let value = self.linearize_instr(&op.operand)?;
                Ok(self.push(
                    instr.try_semantic_instr_id(),
                    ExtractedValueKind::Truthiness { value },
                ))
            }
            InstrCodegenOp::Tuple(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "Tuple",
            }),
            InstrCodegenOp::UnaryOp(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "UnaryOp",
            }),
            InstrCodegenOp::CalleeFunctionId(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "CalleeFunctionId",
            }),
            InstrCodegenOp::DirectFunctionIdGuardTest(_) => {
                Err(RegionExtractionError::UnsupportedInstr {
                    source: instr.try_semantic_instr_id(),
                    kind: "DirectFunctionIdGuardTest",
                })
            }
            InstrCodegenOp::DirectReceiverTypeVersionGuardTest(_) => {
                Err(RegionExtractionError::UnsupportedInstr {
                    source: instr.try_semantic_instr_id(),
                    kind: "DirectReceiverTypeVersionGuardTest",
                })
            }
            InstrCodegenOp::Call(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "Call",
            }),
            InstrCodegenOp::CallDirect(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "CallDirect",
            }),
            InstrCodegenOp::GetAttr(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "GetAttr",
            }),
            InstrCodegenOp::SetAttr(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "SetAttr",
            }),
            InstrCodegenOp::GetItem(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "GetItem",
            }),
            InstrCodegenOp::SetItem(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "SetItem",
            }),
            InstrCodegenOp::DelItem(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "DelItem",
            }),
            InstrCodegenOp::Store(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "Store",
            }),
            InstrCodegenOp::Del(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "Del",
            }),
            InstrCodegenOp::MakeCell(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "MakeCell",
            }),
            InstrCodegenOp::IncrementCounter(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "IncrementCounter",
            }),
            InstrCodegenOp::CellRef(_) => Err(RegionExtractionError::UnsupportedInstr {
                source: instr.try_semantic_instr_id(),
                kind: "CellRef",
            }),
            InstrCodegenOp::MakeFunctionWithClosure(_) => {
                Err(RegionExtractionError::UnsupportedInstr {
                    source: instr.try_semantic_instr_id(),
                    kind: "MakeFunctionWithClosure",
                })
            }
        }
    }

    fn linearize_load(
        &mut self,
        load: &Load<InstrCodegen>,
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
        BinOp, BlockEdge, BlockParam, BlockPyName, LocalLocation, Meta, NameLocation, TermIf,
        Tuple, UnaryOp, WithMeta,
    };

    fn label(index: usize) -> BlockLabel {
        BlockLabel::from_index(index)
    }

    fn instr_id(index: u32) -> InstrId {
        InstrId::new(label(0), index)
    }

    fn with_instr_id(instr: InstrCodegen, index: u32) -> InstrCodegen {
        instr.with_meta(Meta {
            instr_id: Some(instr_id(index)),
            ..Meta::synthetic()
        })
    }

    fn local(name: &str, slot: u32) -> InstrCodegen {
        InstrCodegen::Load(Load::new(ResolvedName {
            id: BlockPyName::new(name),
            location: NameLocation::Local(LocalLocation(slot)),
        }))
    }

    fn binary(op: BinOpKind, left: InstrCodegen, right: InstrCodegen, id: u32) -> InstrCodegen {
        with_instr_id(InstrCodegen::BinOp(BinOp::new(op, left, right)), id)
    }

    fn branch_block(test: InstrCodegen) -> Block<InstrCodegen> {
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
        let tuple = with_instr_id(InstrCodegen::Tuple(Tuple::new(Vec::new())), 0);
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
            InstrCodegen::UnaryOp(UnaryOp::new(
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
