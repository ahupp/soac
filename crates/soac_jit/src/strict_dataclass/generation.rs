//! Deterministic generated-member semantics, separate from creation provenance.
//!
//! An authentic builder can receive trace-mutated arguments. Recording its
//! text and compiling that same text proves origin, not the selected method's
//! behavior. Each SOURCE event must match this exact role-owned fragment and
//! its actual bound operands. Final installation rechecks the same role/code
//! and closure/default witnesses. No substring allowlist or eval is involved.

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyString;
use soac_contracts::DataclassOptions;

use super::catalog::text_is;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FieldRole {
    Instance,
    InitOnly,
    ClassVariable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DefaultKind {
    Missing,
    Value,
    Factory,
}

/// Values projected from the actual canonical Field (or the default Field
/// semantics for an ordinary source default), checked against its signed
/// field name/kind/default and constructor-parameter plan. No Python roots.
/// The caller preserves the signed generator-field subsequence, including
/// init=False fields. The exporter can append ClassVars separately; they do
/// not enter generated field bodies and need no invented constructor order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GenerationField {
    pub(super) name: String,
    pub(super) role: FieldRole,
    pub(super) default: DefaultKind,
    pub(super) init: bool,
    pub(super) kw_only: bool,
    pub(super) repr: bool,
    pub(super) compare: bool,
    pub(super) hash: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GeneratedRole {
    Init,
    Repr,
    Equality,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Hash,
    FrozenSetattr,
    FrozenDelattr,
}

impl GeneratedRole {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Init => "__init__",
            Self::Repr => "__repr__",
            Self::Equality => "__eq__",
            Self::Less => "__lt__",
            Self::LessEqual => "__le__",
            Self::Greater => "__gt__",
            Self::GreaterEqual => "__ge__",
            Self::Hash => "__hash__",
            Self::FrozenSetattr => "__setattr__",
            Self::FrozenDelattr => "__delattr__",
        }
    }
}

/// Exact meaning of an entry in add_fn's locals mapping. Identity is supplied
/// by the active owner/current bound operand, never reconstructed by a name
/// lookup in arbitrary globals. Field values/factories remain ordinary values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LocalOperand {
    FactoryMarker,
    ObjectType,
    FieldDefault(usize),
    FieldFactory(usize),
    RecursiveRepr,
    ActualClass,
    FrozenInstanceError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GeneratedFragment {
    pub(super) role: GeneratedRole,
    pub(super) source: String,
    pub(super) parameters: Vec<String>,
    pub(super) locals: Vec<(String, LocalOperand)>,
    pub(super) annotation_fields: Option<Vec<String>>,
    pub(super) return_none: bool,
    pub(super) unconditional: bool,
    pub(super) overwrite: Overwrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Overwrite {
    Allowed,
    Error,
    OrderingError,
}

impl GeneratedFragment {
    fn new(role: GeneratedRole, parameters: Vec<String>, body: Vec<String>) -> Self {
        let decorator = if role == GeneratedRole::Repr {
            " @__dataclasses_recursive_repr()\n"
        } else {
            ""
        };
        Self {
            role,
            source: format!(
                "{decorator} def {}({}):\n{}",
                role.name(),
                parameters.join(","),
                body.join("\n")
            ),
            parameters,
            locals: Vec::new(),
            annotation_fields: None,
            return_none: false,
            unconditional: false,
            overwrite: Overwrite::Allowed,
        }
    }

    pub(super) fn matches_source(&self, source: &Bound<'_, PyAny>) -> bool {
        unsafe { text_is(source.as_ptr(), &self.source) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HashAction {
    Unchanged,
    SetNone,
    Generate,
    Error,
}

impl HashAction {
    pub(super) fn select(options: &DataclassOptions, explicit_hash: bool) -> Self {
        if options.unsafe_hash {
            if explicit_hash {
                Self::Error
            } else {
                Self::Generate
            }
        } else if !explicit_hash && options.eq {
            if options.frozen {
                Self::Generate
            } else {
                Self::SetNone
            }
        } else {
            Self::Unchanged
        }
    }
}

/// A cold Rust-only plan. Building quoted field literals uses native repr of
/// freshly allocated exact strings (never a field's __repr__); allocations can
/// run GC, so the caller revalidates all actual operands before publishing it.
pub(super) struct GenerationPlan {
    pub(super) has_post_init: bool,
    pub(super) fields: Vec<GenerationField>,
    pub(super) fragments: Vec<GeneratedFragment>,
    pub(super) hash_action: HashAction,
}

impl GenerationPlan {
    pub(super) fn build(
        py: Python<'_>,
        options: &DataclassOptions,
        fields: Vec<GenerationField>,
        has_post_init: bool,
        explicit_hash: bool,
    ) -> PyResult<Option<Self>> {
        if (options.order && !options.eq) || (options.weakref_slot && !options.slots) {
            return Ok(None);
        }
        let mut quoted = Vec::with_capacity(fields.len());
        for (index, field) in fields.iter().enumerate() {
            if fields[..index].iter().any(|other| other.name == field.name) {
                return Ok(None);
            }
            let name = PyString::new(py, &field.name);
            if unsafe { ffi::PyUnicode_IsIdentifier(name.as_ptr()) } != 1 {
                return Ok(None);
            }
            let literal = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(py, ffi::PyObject_Repr(name.as_ptr()))?
            };
            quoted.push(literal.cast::<PyString>()?.to_str()?.to_owned());
            if field.role != FieldRole::Instance && field.default == DefaultKind::Factory {
                return Ok(None);
            }
        }
        let mut fragments = Vec::new();
        if options.init {
            let self_name = if fields.iter().any(|field| field.name == "self") {
                "__dataclass_self__"
            } else {
                "self"
            };
            let mut parameters = vec![self_name.to_owned()];
            let mut body = Vec::new();
            let mut locals = vec![
                (
                    "__dataclass_HAS_DEFAULT_FACTORY__".to_owned(),
                    LocalOperand::FactoryMarker,
                ),
                (
                    "__dataclass_builtins_object__".to_owned(),
                    LocalOperand::ObjectType,
                ),
            ];
            let mut annotations = Vec::new();
            for (index, field) in fields
                .iter()
                .enumerate()
                .filter(|(_, field)| field.role != FieldRole::ClassVariable)
            {
                if field.init {
                    annotations.push(field.name.clone());
                }
                let default_name = format!("__dataclass_dflt_{}__", field.name);
                let value = match field.default {
                    DefaultKind::Factory => {
                        locals.push((default_name.clone(), LocalOperand::FieldFactory(index)));
                        Some(if field.init {
                            format!(
                                "{default_name}() if {} is __dataclass_HAS_DEFAULT_FACTORY__ else {}",
                                field.name, field.name
                            )
                        } else {
                            format!("{default_name}()")
                        })
                    }
                    DefaultKind::Value if field.init || options.slots => {
                        locals.push((default_name.clone(), LocalOperand::FieldDefault(index)));
                        Some(if field.init {
                            field.name.clone()
                        } else {
                            default_name
                        })
                    }
                    DefaultKind::Missing if field.init => Some(field.name.clone()),
                    DefaultKind::Missing | DefaultKind::Value => None,
                };
                if field.role == FieldRole::Instance
                    && let Some(value) = value
                {
                    let (prefix, suffix) = if options.frozen {
                        (
                            format!(
                                "  __dataclass_builtins_object__.__setattr__({self_name},{},",
                                quoted[index]
                            ),
                            ")",
                        )
                    } else {
                        (format!("  {self_name}.{}=", field.name), "")
                    };
                    body.push(format!("{prefix}{value}{suffix}"));
                }
            }
            if has_post_init {
                let initvars = fields
                    .iter()
                    .filter(|field| field.role == FieldRole::InitOnly)
                    .map(|field| field.name.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                body.push(format!("  {self_name}.__post_init__({initvars})"));
            }
            if body.is_empty() {
                body.push("  pass".to_owned());
            }
            for keyword_only in [false, true] {
                let selected = fields.iter().filter(|field| {
                    field.role != FieldRole::ClassVariable
                        && field.init
                        && field.kw_only == keyword_only
                });
                let mut seen_default = false;
                let mut first = true;
                for field in selected {
                    if keyword_only && first {
                        parameters.push("*".to_owned());
                    }
                    first = false;
                    if !keyword_only && seen_default && field.default == DefaultKind::Missing {
                        return Ok(None);
                    }
                    seen_default |= field.default != DefaultKind::Missing;
                    parameters.push(match field.default {
                        DefaultKind::Missing => field.name.clone(),
                        DefaultKind::Value => {
                            format!("{}=__dataclass_dflt_{}__", field.name, field.name)
                        }
                        DefaultKind::Factory => {
                            format!("{}=__dataclass_HAS_DEFAULT_FACTORY__", field.name)
                        }
                    });
                }
            }
            let mut fragment = GeneratedFragment::new(GeneratedRole::Init, parameters, body);
            fragment.locals = locals;
            fragment.annotation_fields = Some(annotations);
            fragment.return_none = true;
            fragments.push(fragment);
        }
        let instance_fields = fields
            .iter()
            .filter(|field| field.role == FieldRole::Instance)
            .collect::<Vec<_>>();
        if options.repr {
            let names = instance_fields
                .iter()
                .filter(|field| field.repr)
                .map(|field| format!("{}={{self.{}!r}}", field.name, field.name))
                .collect::<Vec<_>>()
                .join(", ");
            let mut fragment = GeneratedFragment::new(
                GeneratedRole::Repr,
                vec!["self".to_owned()],
                vec![format!(
                    "  return f\"{{self.__class__.__qualname__}}({names})\""
                )],
            );
            fragment.locals.push((
                "__dataclasses_recursive_repr".to_owned(),
                LocalOperand::RecursiveRepr,
            ));
            fragments.push(fragment);
        }
        if options.eq {
            let mut comparison = instance_fields
                .iter()
                .filter(|field| field.compare)
                .map(|field| format!("self.{}==other.{}", field.name, field.name))
                .collect::<Vec<_>>()
                .join(" and ");
            if comparison.is_empty() {
                comparison = "True".to_owned();
            }
            fragments.push(GeneratedFragment::new(
                GeneratedRole::Equality,
                vec!["self".to_owned(), "other".to_owned()],
                vec![
                    "  if self is other:".to_owned(),
                    "   return True".to_owned(),
                    "  if other.__class__ is self.__class__:".to_owned(),
                    format!("   return {comparison}"),
                    "  return NotImplemented".to_owned(),
                ],
            ));
        }
        if options.order {
            let left = tuple_expression(
                "self",
                instance_fields
                    .iter()
                    .copied()
                    .filter(|field| field.compare),
            );
            let right = tuple_expression(
                "other",
                instance_fields
                    .iter()
                    .copied()
                    .filter(|field| field.compare),
            );
            for (role, operator) in [
                (GeneratedRole::Less, "<"),
                (GeneratedRole::LessEqual, "<="),
                (GeneratedRole::Greater, ">"),
                (GeneratedRole::GreaterEqual, ">="),
            ] {
                let mut fragment = GeneratedFragment::new(
                    role,
                    vec!["self".to_owned(), "other".to_owned()],
                    vec![
                        "  if other.__class__ is self.__class__:".to_owned(),
                        format!("   return {left}{operator}{right}"),
                        "  return NotImplemented".to_owned(),
                    ],
                );
                fragment.overwrite = Overwrite::OrderingError;
                fragments.push(fragment);
            }
        }
        if options.frozen {
            let mut condition = "type(self) is cls".to_owned();
            let names = fields
                .iter()
                .enumerate()
                .filter(|(_, field)| field.role == FieldRole::Instance)
                .map(|(index, _)| quoted[index].as_str())
                .collect::<Vec<_>>();
            if !names.is_empty() {
                condition.push_str(&format!(" or name in {{{}}}", names.join(", ")));
            }
            for (role, action, operation, parameters) in [
                (
                    GeneratedRole::FrozenSetattr,
                    "assign to",
                    "__setattr__(name, value)",
                    vec!["self".to_owned(), "name".to_owned(), "value".to_owned()],
                ),
                (
                    GeneratedRole::FrozenDelattr,
                    "delete",
                    "__delattr__(name)",
                    vec!["self".to_owned(), "name".to_owned()],
                ),
            ] {
                let mut fragment = GeneratedFragment::new(
                    role,
                    parameters,
                    vec![
                        format!("  if {condition}:"),
                        format!(
                            "   raise FrozenInstanceError(f\"cannot {action} field {{name!r}}\")"
                        ),
                        format!("  super(cls, self).{operation}"),
                    ],
                );
                fragment.locals = vec![
                    ("cls".to_owned(), LocalOperand::ActualClass),
                    (
                        "FrozenInstanceError".to_owned(),
                        LocalOperand::FrozenInstanceError,
                    ),
                ];
                fragment.overwrite = Overwrite::Error;
                fragments.push(fragment);
            }
        }
        let hash_action = HashAction::select(options, explicit_hash);
        if hash_action == HashAction::Generate {
            let tuple = tuple_expression(
                "self",
                instance_fields
                    .iter()
                    .copied()
                    .filter(|field| field.hash.unwrap_or(field.compare)),
            );
            let mut fragment = GeneratedFragment::new(
                GeneratedRole::Hash,
                vec!["self".to_owned()],
                vec![format!("  return hash({tuple})")],
            );
            fragment.unconditional = true;
            fragments.push(fragment);
        }
        Ok(Some(Self {
            has_post_init,
            fields,
            fragments,
            hash_action,
        }))
    }

    pub(super) fn fragment(&self, role: GeneratedRole) -> Option<&GeneratedFragment> {
        self.fragments.iter().find(|fragment| fragment.role == role)
    }
}

fn tuple_expression<'a>(
    receiver: &str,
    fields: impl Iterator<Item = &'a GenerationField>,
) -> String {
    let terms = fields
        .map(|field| format!("{receiver}.{}", field.name))
        .collect::<Vec<_>>();
    if terms.is_empty() {
        "()".to_owned()
    } else {
        format!("({},)", terms.join(","))
    }
}

#[cfg(test)]
mod tests {
    use pyo3::types::PyModule;

    use super::*;

    fn field(name: &str) -> GenerationField {
        GenerationField {
            name: name.to_owned(),
            role: FieldRole::Instance,
            default: DefaultKind::Missing,
            init: true,
            kw_only: false,
            repr: true,
            compare: true,
            hash: None,
        }
    }

    #[test]
    fn generated_init_plan_keeps_factory_conditional_initvars_and_keyword_order() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let mut value = field("value");
            value.default = DefaultKind::Factory;
            value.kw_only = true;
            let mut seed = field("seed");
            seed.role = FieldRole::InitOnly;
            seed.kw_only = true;
            let plan = GenerationPlan::build(
                py,
                &DataclassOptions::default(),
                vec![field("self"), value, seed],
                true,
                false,
            )
            .unwrap()
            .unwrap();
            let init = plan.fragment(GeneratedRole::Init).unwrap();
            assert_eq!(
                init.parameters,
                [
                    "__dataclass_self__",
                    "self",
                    "*",
                    "value=__dataclass_HAS_DEFAULT_FACTORY__",
                    "seed"
                ]
            );
            assert!(init.matches_source(PyString::new(py, " def __init__(__dataclass_self__,self,*,value=__dataclass_HAS_DEFAULT_FACTORY__,seed):\n  __dataclass_self__.self=self\n  __dataclass_self__.value=__dataclass_dflt_value__() if value is __dataclass_HAS_DEFAULT_FACTORY__ else value\n  __dataclass_self__.__post_init__(seed)").as_any()));
            assert_eq!(
                init.locals.last(),
                Some(&(
                    "__dataclass_dflt_value__".to_owned(),
                    LocalOperand::FieldFactory(1)
                ))
            );
            assert_eq!(
                init.annotation_fields.as_deref(),
                Some(&["self".to_owned(), "value".to_owned(), "seed".to_owned()][..])
            );
        });
    }

    #[test]
    fn generated_role_rejects_modified_body_name_and_decorator_text() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let options = DataclassOptions {
                frozen: true,
                order: true,
                ..DataclassOptions::default()
            };
            let plan = GenerationPlan::build(py, &options, vec![field("x")], false, false)
                .unwrap()
                .unwrap();
            for role in [
                GeneratedRole::Init,
                GeneratedRole::Repr,
                GeneratedRole::Equality,
                GeneratedRole::FrozenSetattr,
            ] {
                let fragment = plan.fragment(role).unwrap();
                assert!(fragment.matches_source(PyString::new(py, &fragment.source).as_any()));
                for altered in [
                    format!("{}\n  injected()", fragment.source),
                    fragment.source.replacen(role.name(), "__getattribute__", 1),
                    format!(" @foreign_decorator\n{}", fragment.source),
                ] {
                    assert!(!fragment.matches_source(PyString::new(py, &altered).as_any()));
                }
            }
            assert_eq!(plan.hash_action, HashAction::Generate);
            assert_eq!(
                plan.fragment(GeneratedRole::FrozenSetattr)
                    .unwrap()
                    .overwrite,
                Overwrite::Error
            );
            assert_eq!(
                plan.fragment(GeneratedRole::Less).unwrap().overwrite,
                Overwrite::OrderingError
            );
        });
    }

    #[test]
    fn generated_role_projection_uses_exact_field_flags_and_does_not_invent_checks() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let mut hidden = field("hidden");
            hidden.init = false;
            hidden.repr = false;
            hidden.compare = false;
            hidden.hash = Some(true);
            let mut classvar = field("shared");
            classvar.role = FieldRole::ClassVariable;
            let options = DataclassOptions {
                init: false,
                repr: false,
                eq: false,
                unsafe_hash: true,
                ..DataclassOptions::default()
            };
            let plan = GenerationPlan::build(py, &options, vec![hidden, classvar], false, false)
                .unwrap()
                .unwrap();
            assert_eq!(plan.fragments.len(), 1);
            let hash = plan.fragment(GeneratedRole::Hash).unwrap();
            assert!(hash.matches_source(
                PyString::new(py, " def __hash__(self):\n  return hash((self.hidden,))").as_any()
            ));
            assert!(hash.annotation_fields.is_none());
            assert!(!hash.return_none);
            assert!(hash.unconditional);
        });
    }

    #[test]
    fn generation_plan_matches_the_ordinary_stdlib_full_role_transcript() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            // This is an ordinary control, not an admitted helper graph. The
            // temporary subclass only captures the actual stdlib's text and
            // is restored before the test compares or asserts anything.
            let module = PyModule::from_code(
                py,
                c"
import dataclasses
from typing import ClassVar
original = dataclasses._FuncBuilder
captured = []
class Capture(original):
    def add_fns_to_class(self, cls):
        captured.extend(self.src)
        return super().add_fns_to_class(cls)
try:
    dataclasses._FuncBuilder = Capture
    @dataclasses.dataclass(frozen=True, order=True, kw_only=True)
    class Example:
        x: int = 1
        y: int = dataclasses.field(default_factory=lambda: 2, repr=False, compare=False, hash=True)
        seed: dataclasses.InitVar[int] = 0
        shared: ClassVar[int] = 9
        z: int = dataclasses.field(init=False, default=4)
        def __post_init__(self, seed):
            pass
finally:
    dataclasses._FuncBuilder = original
",
                c"<stdlib generation control>",
                c"stdlib_generation_control",
            )
            .unwrap();
            let mut x = field("x");
            x.default = DefaultKind::Value;
            x.kw_only = true;
            let mut y = field("y");
            y.default = DefaultKind::Factory;
            y.kw_only = true;
            y.repr = false;
            y.compare = false;
            y.hash = Some(true);
            let mut seed = field("seed");
            seed.role = FieldRole::InitOnly;
            seed.default = DefaultKind::Value;
            seed.kw_only = true;
            let mut z = field("z");
            z.default = DefaultKind::Value;
            z.init = false;
            z.kw_only = true;
            let mut shared = field("shared");
            shared.role = FieldRole::ClassVariable;
            shared.default = DefaultKind::Value;
            let options = DataclassOptions {
                frozen: true,
                order: true,
                kw_only: true,
                ..DataclassOptions::default()
            };
            let plan =
                GenerationPlan::build(py, &options, vec![x, y, seed, z, shared], true, false)
                    .unwrap()
                    .unwrap();
            let captured = module.getattr("captured").unwrap();
            assert_eq!(captured.len().unwrap(), plan.fragments.len());
            for (index, fragment) in plan.fragments.iter().enumerate() {
                assert!(
                    fragment.matches_source(&captured.get_item(index).unwrap()),
                    "{:?}",
                    fragment.role
                );
            }
        });
    }
}
