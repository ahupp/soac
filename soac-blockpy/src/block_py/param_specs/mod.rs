use crate::passes::ast_to_ast::expr_utils::make_dp_tuple;
use ruff_python_ast::{self as ast, Expr};

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ParamKind {
    Any,
    PosOnly,
    VarArg,
    KwOnly,
    KwArg,
}

#[derive(Debug, Clone, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Param {
    pub name: String,
    pub kind: ParamKind,
    pub has_default: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ParamDefaultSource<'a> {
    Positional(usize),
    KeywordOnly(&'a str),
}

#[derive(
    Debug, Clone, Default, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct ParamSpec {
    pub params: Vec<Param>,
}

impl ParamSpec {
    pub fn len(&self) -> usize {
        self.params.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Param> {
        self.params.iter()
    }

    pub fn iter_with_default_sources(
        &self,
    ) -> impl Iterator<Item = (&Param, Option<ParamDefaultSource<'_>>)> + '_ {
        let mut next_positional_default = 0usize;
        self.params.iter().map(move |param| {
            let default_source = if !param.has_default {
                None
            } else {
                match param.kind {
                    ParamKind::PosOnly | ParamKind::Any => {
                        let index = next_positional_default;
                        next_positional_default += 1;
                        Some(ParamDefaultSource::Positional(index))
                    }
                    ParamKind::KwOnly => Some(ParamDefaultSource::KeywordOnly(param.name.as_str())),
                    ParamKind::VarArg | ParamKind::KwArg => None,
                }
            };
            (param, default_source)
        })
    }

    pub fn names(&self) -> Vec<String> {
        self.params.iter().map(|param| param.name.clone()).collect()
    }

    pub fn default_count(&self) -> usize {
        self.params.iter().filter(|param| param.has_default).count()
    }

    pub fn positional_param_indices(&self) -> Vec<usize> {
        self.params
            .iter()
            .enumerate()
            .filter(|(_, param)| matches!(param.kind, ParamKind::PosOnly | ParamKind::Any))
            .map(|(index, _)| index)
            .collect()
    }

    pub fn param_index(&self, name: &str) -> Option<usize> {
        self.params.iter().position(|param| param.name == name)
    }

    pub fn vararg_index(&self) -> Option<usize> {
        self.params
            .iter()
            .position(|param| param.kind == ParamKind::VarArg)
    }

    pub fn kwarg_index(&self) -> Option<usize> {
        self.params
            .iter()
            .position(|param| param.kind == ParamKind::KwArg)
    }

    pub(crate) fn validate_default_count(&self, count: usize) {
        assert_eq!(
            self.default_count(),
            count,
            "ParamSpec default count does not match defaults payload",
        );
    }
}

fn push_param(
    spec: &mut ParamSpec,
    defaults: &mut Vec<Expr>,
    param: Param,
    default: Option<&Expr>,
) {
    if param.has_default {
        defaults.push(
            default
                .cloned()
                .expect("params marked with has_default should carry a default expression"),
        );
    }
    spec.params.push(param);
}

pub(crate) fn collect_param_spec_and_defaults(
    parameters: &ast::Parameters,
) -> (ParamSpec, Vec<Expr>) {
    let mut spec = ParamSpec::default();
    let mut defaults = Vec::new();

    for param in &parameters.posonlyargs {
        push_param(
            &mut spec,
            &mut defaults,
            Param {
                name: param.parameter.name.id.to_string(),
                kind: ParamKind::PosOnly,
                has_default: param.default.is_some(),
            },
            param.default.as_deref(),
        );
    }
    for param in &parameters.args {
        push_param(
            &mut spec,
            &mut defaults,
            Param {
                name: param.parameter.name.id.to_string(),
                kind: ParamKind::Any,
                has_default: param.default.is_some(),
            },
            param.default.as_deref(),
        );
    }
    if let Some(param) = &parameters.vararg {
        spec.params.push(Param {
            name: param.name.id.to_string(),
            kind: ParamKind::VarArg,
            has_default: false,
        });
    }
    for param in &parameters.kwonlyargs {
        push_param(
            &mut spec,
            &mut defaults,
            Param {
                name: param.parameter.name.id.to_string(),
                kind: ParamKind::KwOnly,
                has_default: param.default.is_some(),
            },
            param.default.as_deref(),
        );
    }
    if let Some(param) = &parameters.kwarg {
        spec.params.push(Param {
            name: param.name.id.to_string(),
            kind: ParamKind::KwArg,
            has_default: false,
        });
    }

    spec.validate_default_count(defaults.len());
    (spec, defaults)
}

pub(crate) fn param_defaults_to_expr(defaults: &[Expr]) -> Expr {
    make_dp_tuple(defaults.to_vec())
}

#[cfg(test)]
mod test;
