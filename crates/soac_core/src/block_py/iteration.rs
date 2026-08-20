//! Borrowed stepping of an explicitly owned compiler iterator operand.
//!
//! This is the source-loop protocol, not a rewrite of a Python `next` call.
//! The operation reads the actual owner without cloning or consuming it. The
//! producer must retire that owner on every exit from the loop region.

use super::instr::validated_compiler_operand_location;
use super::{
    ChildVisitable, HasMeta, Instr, MapInstr, Mappable, Meta, OperandLocation, ResolvedName,
    StorageLayout, TryMapInstr, WithMeta, define_instr,
};

define_instr! {
    /// Return one owned item, or leave a pending StopIteration/other Python
    /// exception for the existing source-loop exhaustion/error continuation.
    /// The iterator primary remains owned and unchanged on either outcome.
    pub struct IteratorStep<I> {
        name: I::Name,
    }
}

impl<I: Instr> IteratorStep<I> {
    pub fn read_names(&self) -> impl Iterator<Item = &I::Name> {
        std::iter::once(&self.name)
    }
}

impl<I: Instr<Name = ResolvedName>> IteratorStep<I> {
    /// Validate the physical Operand owner, never its generated spelling.
    /// A checked borrowed read still rejects an uninitialized owner at runtime.
    pub fn validate_resolved(&self, layout: &StorageLayout) -> Result<OperandLocation, String> {
        validated_compiler_operand_location(&self.name, layout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_py::{LocalLocation, NameLocation};

    #[derive(Clone, Debug)]
    struct Expr;

    impl Instr for Expr {
        type Name = ResolvedName;
        type Extra = &'static str;
    }

    fn name(alias: &str, slot: u32) -> ResolvedName {
        ResolvedName {
            id: alias.into(),
            location: NameLocation::Local(LocalLocation(slot)),
        }
    }

    #[test]
    fn iterator_step_requires_the_physical_operand_not_a_source_or_named_alias() {
        let mut layout = StorageLayout {
            stack_slots: vec!["iterator_owner".into(), "source_iterator".into()],
            ..StorageLayout::default()
        };
        layout.mark_expression_temporary(LocalLocation(0));
        let op = IteratorStep::<Expr>::new(name("unrelated_display_alias", 0));
        assert_eq!(
            op.validate_resolved(&layout).unwrap(),
            OperandLocation::Local(LocalLocation(0)),
        );
        assert_eq!(op.read_names().next().unwrap().location, op.name.location);
        assert!(
            IteratorStep::<Expr>::new(name("_dp_iter_claimed_owner", 1))
                .validate_resolved(&layout)
                .is_err()
        );
        assert!(
            IteratorStep::<Expr>::new(name("iterator_owner", 7))
                .validate_resolved(&layout)
                .is_err()
        );
    }

    #[test]
    fn iterator_step_maps_its_explicit_borrow_without_creating_an_operand_move() {
        struct Rename;
        impl MapInstr<Expr, Expr> for Rename {
            fn map_instr(&mut self, value: Expr) -> Expr {
                value
            }
            fn map_name(&mut self, mut value: ResolvedName) -> ResolvedName {
                value.id = "renamed".into();
                value
            }
        }
        let op = IteratorStep::<Expr>::new(name("iterator", 0)).with_extra("step");
        let mapped = op.clone().map_same_children(&mut Rename);
        assert_eq!(mapped.name.id.as_str(), "renamed");
        assert_eq!(mapped.name.location, op.name.location);
        assert_eq!(*mapped.extra(), "step");
    }
}
