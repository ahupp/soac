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

    pub fn validate_default_count(&self, count: usize) {
        assert_eq!(
            self.default_count(),
            count,
            "ParamSpec default count does not match defaults payload",
        );
    }
}
