#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum ScopeKind {
    Function,
    Class,
    Module,
}

pub(crate) fn is_internal_symbol(name: &str) -> bool {
    name.starts_with("_dp_") || name == "__soac__"
}
