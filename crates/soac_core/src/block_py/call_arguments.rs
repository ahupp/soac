//! Source-ordered call preparation and explicit owned-input selection.
//!
//! The named slots are compiler Operand owners, not source bindings or callable
//! capabilities. These operations never authorize a Python callable or assume
//! that an ordinary module's globals/builtins are immutable.

use super::instr::validated_compiler_operand_location;
use super::{
    CallArgPositional, ChildVisitable, FrameNamespace, HasMeta, Instr, MapInstr, Mappable, Meta,
    OperandLocation, ResolvedName, StorageLayout, TakeOperandInstruction, TryMapInstr, WithMeta,
    define_instr, instr_any,
};

/// Whether this positional call already supplies one owned expression reference
/// for each input. This is an ownership shape, not callable admission or native
/// borrow ancestry. The caller classifies only its IR's fresh call-result nodes.
pub fn call_has_owned_operand_inputs<I>(
    callable: &I,
    args: &[CallArgPositional<I>],
    has_keywords: bool,
    has_namespace: bool,
    layout: &StorageLayout,
    is_fresh_call_result: impl Fn(&I) -> bool,
) -> Result<bool, String>
where
    I: TakeOperandInstruction<Name = ResolvedName>,
{
    if has_keywords || has_namespace {
        return Ok(false);
    }
    let Some(callable) = callable.as_take_operand() else {
        return Ok(false);
    };
    let mut moved = vec![callable.validate_resolved(layout)?];
    for arg in args {
        let CallArgPositional::Positional(arg) = arg else {
            return Ok(false);
        };
        if let Some(take) = arg.as_take_operand() {
            let owner = take.validate_resolved(layout)?;
            if moved.contains(&owner) {
                return Err("outgoing call consumes one physical Operand more than once".into());
            }
            moved.push(owner);
        } else if !is_fresh_call_result(arg) {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum CallArgumentOpKind {
    /// LIST_EXTEND: consume the update after expansion/error formatting. The
    /// list primary stays live, including a partially expanded prefix on error.
    ExtendPositional,
    /// DICT_MERGE: reject duplicate keys and consume the update after native
    /// error formatting. Both callable and keyword-dict primary stay live.
    MergeKeywords,
    /// LIST_TO_TUPLE: take/clear the list primary before conversion, consume its
    /// contents and then the list itself, publish a tuple only on success.
    FinishPositionalList,
    /// CALL_FUNCTION_EX singleton-star conversion, after keyword preparation:
    /// borrow the raw primary; on error leave it untouched. On success publish
    /// the tuple before releasing the old raw argument.
    NormalizeSingletonStar,
}

impl CallArgumentOpKind {
    pub fn has_value(self) -> bool {
        matches!(self, Self::ExtendPositional | Self::MergeKeywords)
    }

    pub fn replaces_buffer(self) -> bool {
        matches!(
            self,
            Self::FinishPositionalList | Self::NormalizeSingletonStar
        )
    }

    /// This is also an exceptional ownership effect. In particular, it must not
    /// be inferred from `replaces_buffer`: singleton normalization is NO_POP.
    pub fn consumes_buffer_before_helper(self) -> bool {
        self == Self::FinishPositionalList
    }
}

define_instr! {
    pub struct CallArgumentOp<I> {
        kind: CallArgumentOpKind,
        callable: I::Name,
        buffer: I::Name,
        value: Option<Box<I>>,
    }
}

impl<I: Instr> CallArgumentOp<I> {
    pub fn validate_shape(&self) -> Result<(), String> {
        if self.kind.has_value() != self.value.is_some() {
            return Err("call-argument phase has an invalid update shape".into());
        }
        Ok(())
    }

    pub fn read_names(&self) -> impl Iterator<Item = &I::Name> {
        [&self.callable, &self.buffer].into_iter()
    }

    pub fn written_names(&self) -> impl Iterator<Item = &I::Name> {
        self.kind
            .replaces_buffer()
            .then_some(&self.buffer)
            .into_iter()
    }
}

impl<I: TakeOperandInstruction<Name = ResolvedName>> CallArgumentOp<I> {
    /// Physical ownership is independent of displayed names and Python types.
    /// The native helper must still check the required exact list/dict type.
    pub fn validate_resolved(
        &self,
        layout: &StorageLayout,
    ) -> Result<(OperandLocation, OperandLocation), String> {
        self.validate_shape()?;
        let callable = validated_compiler_operand_location(&self.callable, layout)?;
        let buffer = validated_compiler_operand_location(&self.buffer, layout)?;
        if callable.name_location() == buffer.name_location() {
            return Err("call-argument phase aliases its callable and buffer owners".into());
        }
        if self.value.as_deref().is_some_and(|value| {
            instr_any(value, |instr| {
                instr.as_take_operand().is_some_and(|take| {
                    take.name.location == callable.name_location()
                        || take.name.location == buffer.name_location()
                })
            })
        }) {
            return Err("call-argument update consumes a borrowed phase owner".into());
        }
        Ok((callable, buffer))
    }
}

define_instr! {
    /// Invoke an already prepared exact tuple and optional exact dict. Every
    /// runtime operand is an explicit move from a distinct Operand primary;
    /// this operation must not re-expand positional or keyword arguments.
    pub struct PreparedCall<I> {
        func: Box<I>,
        arguments: Box<I>,
        keywords: Option<Box<I>>,
        frame_namespace: Option<FrameNamespace<I>>,
    }
}

impl<I: TakeOperandInstruction<Name = ResolvedName>> PreparedCall<I> {
    pub fn validate_resolved(&self, layout: &StorageLayout) -> Result<(), String> {
        let mut owners = Vec::with_capacity(3);
        for input in std::iter::once(self.func.as_ref())
            .chain(std::iter::once(self.arguments.as_ref()))
            .chain(self.keywords.as_deref())
        {
            let take = input
                .as_take_operand()
                .ok_or("prepared call requires an exact Operand move for each input")?;
            let owner = take.validate_resolved(layout)?.name_location();
            if owners.contains(&owner) {
                return Err("prepared call consumes one physical Operand more than once".into());
            }
            owners.push(owner);
        }
        if self
            .frame_namespace
            .as_ref()
            .and_then(FrameNamespace::mapping)
            .is_some_and(|namespace| {
                instr_any(namespace, |instr| instr.as_take_operand().is_some())
            })
        {
            return Err("prepared call namespace cannot consume an Operand".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_py::{LocalLocation, NameLocation, TakeOperand, Visit, VisitMut};

    #[derive(Clone, Debug)]
    enum Expr {
        Leaf,
        FreshCallResult,
        Take(TakeOperand<Self>),
        Sequence(Vec<Self>),
    }

    impl Instr for Expr {
        type Name = ResolvedName;
        type Extra = &'static str;
    }

    impl ChildVisitable<Self> for Expr {
        fn visit_children<V: Visit<Self> + ?Sized>(&self, visitor: &mut V) {
            if let Self::Sequence(values) = self {
                for value in values {
                    visitor.visit_instr(value);
                }
            }
        }
        fn visit_children_mut<V: VisitMut<Self> + ?Sized>(&mut self, visitor: &mut V) {
            if let Self::Sequence(values) = self {
                for value in values {
                    visitor.visit_instr_mut(value);
                }
            }
        }
    }

    impl TakeOperandInstruction for Expr {
        fn as_take_operand(&self) -> Option<&TakeOperand<Self>> {
            match self {
                Self::Take(op) => Some(op),
                _ => None,
            }
        }
    }

    fn name(alias: &str, slot: u32) -> ResolvedName {
        ResolvedName {
            id: alias.into(),
            location: NameLocation::Local(LocalLocation(slot)),
        }
    }

    fn take(slot: u32) -> Box<Expr> {
        Box::new(Expr::Take(TakeOperand::new(name("display_alias", slot))))
    }

    fn layout() -> StorageLayout {
        let mut layout = StorageLayout {
            stack_slots: vec![
                "callee".into(),
                "args".into(),
                "kwargs".into(),
                "source".into(),
            ],
            ..StorageLayout::default()
        };
        for slot in 0..3 {
            layout.mark_expression_temporary(LocalLocation(slot));
        }
        layout
    }

    #[test]
    fn call_argument_phase_rejects_physical_aliases_and_borrowed_owner_takes() {
        let layout = layout();
        let mut op = CallArgumentOp::new(
            CallArgumentOpKind::MergeKeywords,
            name("func_alias", 0),
            name("buffer_alias", 1),
            Some(take(2)),
        );
        assert!(op.validate_resolved(&layout).is_ok());
        op.buffer = name("different_name_same_slot", 0);
        assert!(op.validate_resolved(&layout).is_err());
        op.buffer = name("buffer_alias", 1);
        for slot in [0, 1] {
            op.value = Some(Box::new(Expr::Sequence(vec![Expr::Leaf, *take(slot)])));
            assert!(op.validate_resolved(&layout).is_err());
        }
        op.value = Some(take(2));
        op.buffer = name("_dp_tmp_fake_authority", 3);
        assert!(op.validate_resolved(&layout).is_err());
    }

    #[test]
    fn call_argument_phase_maps_names_and_preserves_distinct_failure_effects() {
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
        let finish = CallArgumentOp::<Expr>::new(
            CallArgumentOpKind::FinishPositionalList,
            name("func", 0),
            name("args", 1),
            None,
        )
        .with_extra("ownership-phase");
        let same = finish.clone().map_same_children(&mut Rename);
        assert_eq!(same.callable.location, finish.callable.location);
        assert_eq!(same.buffer.location, finish.buffer.location);
        assert_eq!(same.callable.id.as_str(), "renamed");
        assert_eq!(*same.extra(), "ownership-phase");
        assert!(same.kind.consumes_buffer_before_helper());
        assert_eq!(same.written_names().count(), 1);
        let mut normalize = same;
        normalize.kind = CallArgumentOpKind::NormalizeSingletonStar;
        assert!(!normalize.kind.consumes_buffer_before_helper());
        assert_eq!(normalize.written_names().count(), 1);
        normalize.value = Some(take(2));
        assert!(normalize.validate_shape().is_err());
    }

    #[test]
    fn positional_call_owned_inputs_reject_local_reads_and_physical_aliases() {
        let layout = layout();
        let selected = |callable: &Expr, args: &[CallArgPositional<Expr>], keywords, namespace| {
            call_has_owned_operand_inputs(callable, args, keywords, namespace, &layout, |input| {
                matches!(input, Expr::FreshCallResult)
            })
        };
        let args = [
            CallArgPositional::Positional(*take(1)),
            CallArgPositional::Positional(Expr::FreshCallResult),
        ];
        assert_eq!(selected(&take(0), &args, false, false), Ok(true));
        assert_eq!(selected(&Expr::Leaf, &args, false, false), Ok(false));
        for (keywords, namespace) in [(true, false), (false, true)] {
            assert_eq!(selected(&take(0), &args, keywords, namespace), Ok(false));
        }
        assert_eq!(
            selected(
                &take(0),
                &[CallArgPositional::Positional(Expr::Leaf)],
                false,
                false
            ),
            Ok(false),
        );
        assert_eq!(
            selected(
                &take(0),
                &[CallArgPositional::Starred(*take(1))],
                false,
                false
            ),
            Ok(false),
        );
        for args in [
            vec![CallArgPositional::Positional(*take(0))],
            vec![
                CallArgPositional::Positional(*take(1)),
                CallArgPositional::Positional(Expr::Take(TakeOperand::new(name(
                    "same_owner_different_alias",
                    1,
                )))),
            ],
            vec![CallArgPositional::Positional(*take(3))],
        ] {
            assert!(selected(&take(0), &args, false, false).is_err());
        }
    }

    #[test]
    fn prepared_call_requires_distinct_explicit_moves_not_loads_or_nested_takes() {
        let layout = layout();
        let mut call = PreparedCall::new(take(0), take(1), Some(take(2)), None);
        assert!(call.validate_resolved(&layout).is_ok());
        call.keywords = Some(take(1));
        assert!(call.validate_resolved(&layout).is_err());
        call.keywords = Some(take(2));
        call.arguments = Box::new(Expr::Sequence(vec![*take(1)]));
        assert!(call.validate_resolved(&layout).is_err());
        call.arguments = take(1);
        call.func = Box::new(Expr::Leaf);
        assert!(call.validate_resolved(&layout).is_err());
        call.func = take(0);
        call.frame_namespace = Some(FrameNamespace::Mapping(take(2)));
        assert!(call.validate_resolved(&layout).is_err());
    }
}
