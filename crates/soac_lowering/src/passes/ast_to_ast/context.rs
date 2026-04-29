use std::cell::RefCell;
use std::collections::HashSet;

use crate::passes::ast_to_ast::scope_helpers::ScopeKind;

use crate::namegen::fresh_name;

#[derive(Clone, Debug)]
pub(crate) struct ScopeFrame {
    pub kind: ScopeKind,
    pub in_async_function: bool,
    pub globals: HashSet<String>,
    pub nonlocals: HashSet<String>,
}

impl ScopeFrame {
    pub(crate) fn module() -> Self {
        Self {
            kind: ScopeKind::Module,
            in_async_function: false,
            globals: HashSet::new(),
            nonlocals: HashSet::new(),
        }
    }

    pub(crate) fn new(
        kind: ScopeKind,
        globals: HashSet<String>,
        nonlocals: HashSet<String>,
    ) -> Self {
        Self {
            kind,
            in_async_function: false,
            globals,
            nonlocals,
        }
    }
}

pub(crate) struct Context {
    pub source: String,
    scope_stack: RefCell<Vec<ScopeFrame>>,
    value_forwarding_local_stack: RefCell<Vec<HashSet<String>>>,
    no_raise_local_stack: RefCell<Vec<HashSet<String>>>,
}

impl Context {
    pub(crate) fn new(source: &str) -> Self {
        Self {
            source: source.to_string(),
            scope_stack: RefCell::new(vec![ScopeFrame::module()]),
            value_forwarding_local_stack: RefCell::new(vec![HashSet::new()]),
            no_raise_local_stack: RefCell::new(vec![HashSet::new()]),
        }
    }

    pub(crate) fn line_number_at(&self, offset: usize) -> usize {
        self.source[..offset]
            .bytes()
            .filter(|&b| b == b'\n')
            .count()
            + 1
    }

    pub(crate) fn fresh(&self, name: &str) -> String {
        fresh_name(name)
    }

    pub(crate) fn push_scope(&self, frame: ScopeFrame) {
        self.scope_stack.borrow_mut().push(frame);
    }

    pub(crate) fn pop_scope(&self) {
        self.scope_stack.borrow_mut().pop();
    }

    pub(crate) fn current_scope(&self) -> ScopeFrame {
        self.scope_stack
            .borrow()
            .last()
            .cloned()
            .unwrap_or_else(ScopeFrame::module)
    }

    pub(crate) fn push_value_forwarding_locals(&self, names: HashSet<String>) {
        self.value_forwarding_local_stack.borrow_mut().push(names);
    }

    pub(crate) fn pop_value_forwarding_locals(&self) {
        self.value_forwarding_local_stack.borrow_mut().pop();
    }

    pub(crate) fn current_value_forwarding_locals(&self) -> HashSet<String> {
        self.value_forwarding_local_stack
            .borrow()
            .last()
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn push_no_raise_locals(&self, names: HashSet<String>) {
        self.no_raise_local_stack.borrow_mut().push(names);
    }

    pub(crate) fn pop_no_raise_locals(&self) {
        self.no_raise_local_stack.borrow_mut().pop();
    }

    pub(crate) fn current_no_raise_locals(&self) -> HashSet<String> {
        self.no_raise_local_stack
            .borrow()
            .last()
            .cloned()
            .unwrap_or_default()
    }
}
