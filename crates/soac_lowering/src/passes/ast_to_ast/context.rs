use std::cell::RefCell;
use std::collections::HashSet;

use crate::passes::ast_to_ast::scope_helpers::ScopeKind;

use crate::namegen::fresh_name;

#[derive(Clone, Debug)]
pub struct ScopeFrame {
    pub kind: ScopeKind,
    pub in_async_function: bool,
    pub globals: HashSet<String>,
    pub nonlocals: HashSet<String>,
}

impl ScopeFrame {
    pub fn module() -> Self {
        Self {
            kind: ScopeKind::Module,
            in_async_function: false,
            globals: HashSet::new(),
            nonlocals: HashSet::new(),
        }
    }

    pub fn new(kind: ScopeKind, globals: HashSet<String>, nonlocals: HashSet<String>) -> Self {
        Self {
            kind,
            in_async_function: false,
            globals,
            nonlocals,
        }
    }
}

pub struct Context {
    pub source: String,
    scope_stack: RefCell<Vec<ScopeFrame>>,
}

impl Context {
    pub fn new(source: &str) -> Self {
        Self {
            source: source.to_string(),
            scope_stack: RefCell::new(vec![ScopeFrame::module()]),
        }
    }

    pub fn line_number_at(&self, offset: usize) -> usize {
        self.source[..offset]
            .bytes()
            .filter(|&b| b == b'\n')
            .count()
            + 1
    }

    pub fn fresh(&self, name: &str) -> String {
        fresh_name(name)
    }

    pub fn push_scope(&self, frame: ScopeFrame) {
        self.scope_stack.borrow_mut().push(frame);
    }

    pub fn pop_scope(&self) {
        self.scope_stack.borrow_mut().pop();
    }

    pub fn current_scope(&self) -> ScopeFrame {
        self.scope_stack
            .borrow()
            .last()
            .cloned()
            .unwrap_or_else(ScopeFrame::module)
    }
}
