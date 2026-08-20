//! Native collection construction from explicitly ordered owned inputs.

use super::{
    ChildVisitable, HasMeta, Instr, MapInstr, Mappable, Meta, TryMapInstr, WithMeta, define_instr,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum BuildCollectionKind {
    List,
    Set,
    Dict,
}

define_instr! {
    /// Evaluate elements in order and construct the exact builtin collection.
    /// Dict elements are interleaved key/value pairs, not tuple objects.
    /// After entry to the native builder all input references are consumed on
    /// both exits, with the corresponding native BUILD_* cleanup order.
    pub struct BuildCollection<I> {
        kind: BuildCollectionKind,
        values: Vec<I>,
    }
}

impl<I: Instr> BuildCollection<I> {
    pub fn validate_shape(&self) -> Result<(), String> {
        if self.kind == BuildCollectionKind::Dict && self.values.len() % 2 != 0 {
            return Err("dict construction requires interleaved key/value pairs".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_py::{UnresolvedName, Visit, VisitMut};

    #[derive(Clone, Debug)]
    struct Expr(u8);

    impl Instr for Expr {
        type Name = UnresolvedName;
        type Extra = &'static str;
    }

    impl ChildVisitable<Self> for Expr {
        fn visit_children<V: Visit<Self> + ?Sized>(&self, _: &mut V) {}
        fn visit_children_mut<V: VisitMut<Self> + ?Sized>(&mut self, _: &mut V) {}
    }

    #[test]
    fn build_collection_preserves_interleaved_source_order_and_rejects_partial_pair() {
        struct Children(Vec<u8>);
        impl Visit<Expr> for Children {
            fn visit_instr(&mut self, value: &Expr) {
                self.0.push(value.0);
            }
        }
        let mut op = BuildCollection::new(
            BuildCollectionKind::Dict,
            vec![Expr(0), Expr(1), Expr(2), Expr(3)],
        )
        .with_extra("native-construction");
        let mut children = Children(Vec::new());
        op.visit_children(&mut children);
        assert_eq!(children.0, vec![0, 1, 2, 3]);
        assert!(op.validate_shape().is_ok());
        let same = op.clone().map_same_children(&mut |value| value);
        assert_eq!(*same.extra(), "native-construction");
        op.values.pop();
        assert!(op.validate_shape().is_err());
        op.kind = BuildCollectionKind::Set;
        assert!(op.validate_shape().is_ok());
        op.kind = BuildCollectionKind::List;
        assert!(op.validate_shape().is_ok());
    }
}
