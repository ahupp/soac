//! Original lexical identities and explicitly registered rewrite origins.
//!
//! Names used by generated helpers are not authority. This catalog is built
//! from the original parsed bytes, checked against the authenticated proposal,
//! and consulted by the rewrite which creates each corresponding helper.

use std::collections::HashMap;

mod source_names;

pub(crate) use source_names::SourceNameCatalog;
pub(super) use source_names::{pattern_name, statement_names, type_parameter_name};

use ruff_python_ast::token::{TokenKind, Tokens};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::{Ranged, TextRange};
use soac_contracts::{DefinitionKind, ModuleTypeFacts, SourceIdentity, SourceRange};

use super::semantic::{SemanticAstState, SemanticScope};
use crate::transformer::{walk_expr, walk_stmt, Transformer};

#[derive(Default)]
pub(crate) struct SourceCatalog {
    source_names: SourceNameCatalog,
    definitions: HashMap<(TextRange, DefinitionKind), SourceIdentity>,
    native_definitions: HashMap<(TextRange, DefinitionKind), NativeSourceDefinition>,
    generator_expressions: HashMap<TextRange, soac_core::block_py::GeneratorExpressionCode>,
}

#[derive(Clone, Debug)]
pub(crate) struct NativeSourceDefinition {
    pub name: String,
    pub qualname: String,
    pub first_offset: u32,
    /// The actual def/async/class keyword, distinct from an earlier decorator.
    /// For a module, its first source statement anchors native annotation setup.
    pub header_offset: u32,
}

impl SourceCatalog {
    pub(crate) fn record_source_names(&mut self, body: &ast::Suite) {
        self.source_names = SourceNameCatalog::from_original(&mut body.clone());
    }

    pub(crate) fn source_names(&self) -> &SourceNameCatalog {
        &self.source_names
    }

    pub(crate) fn from_original(
        facts: &ModuleTypeFacts,
        body: &ast::Suite,
        tokens: &Tokens,
    ) -> anyhow::Result<Self> {
        struct Collector<'a> {
            facts: &'a ModuleTypeFacts,
            tokens: &'a Tokens,
            path: Vec<String>,
            native_path: Vec<String>,
            scope: SemanticScope,
            catalog: SourceCatalog,
        }
        impl Collector<'_> {
            fn record(&mut self, name: &str, range: TextRange, kind: DefinitionKind) {
                let mut path = self.path.clone();
                path.push(name.to_owned());
                let identity = SourceIdentity {
                    module: self.facts.module.clone(),
                    lexical_qualname: path.join("."),
                    source_range: SourceRange::new(range.start().to_u32(), range.end().to_u32()),
                    definition_kind: kind,
                };
                assert!(self
                    .catalog
                    .definitions
                    .insert((range, kind), identity)
                    .is_none());
                let mut native_path = self.native_path.clone();
                native_path.push(name.to_owned());
                // A global def/class still belongs to its original lexical
                // owner, but CPython starts its native qualname at the module.
                // Use the same resolved scope rule as ordinary lowering, not
                // a second walk that guesses which global statement applies.
                let native_qualname = match kind {
                    DefinitionKind::Function | DefinitionKind::Class => {
                        self.scope.child_function_qualname(name)
                    }
                    _ => native_path.join("."),
                };
                assert!(self
                    .catalog
                    .native_definitions
                    .insert(
                        (range, kind),
                        NativeSourceDefinition {
                            name: name.to_owned(),
                            qualname: native_qualname,
                            first_offset: range.start().to_u32(),
                            header_offset: range.start().to_u32(),
                        },
                    )
                    .is_none());
            }

            fn visit_generator_expression(
                &mut self,
                generator: &mut ast::ExprGenerator,
                native_range: TextRange,
            ) {
                let (first, remaining) = generator
                    .generators
                    .split_first_mut()
                    .expect("parsed generator expression has an iterator");
                assert!(self
                    .catalog
                    .generator_expressions
                    .insert(
                        generator.range,
                        soac_core::block_py::GeneratorExpressionCode {
                            expression_range: SourceRange::new(
                                native_range.start().to_u32(),
                                native_range.end().to_u32(),
                            ),
                            iterable_range: SourceRange::new(
                                first.iter.range().start().to_u32(),
                                first.iter.range().end().to_u32(),
                            ),
                        },
                    )
                    .is_none());
                // Only the first iterable is evaluated in the outer
                // code object. Source lexical ancestry stays unchanged;
                // the suspended body has its own native <genexpr>.
                self.visit_expr(&mut first.iter);
                self.native_path.push("<genexpr>".into());
                self.visit_expr(&mut first.target);
                for condition in &mut first.ifs {
                    self.visit_expr(condition);
                }
                for comprehension in remaining {
                    self.visit_comprehension(comprehension);
                }
                self.visit_expr(&mut generator.elt);
                self.native_path.pop();
            }

            fn record_header_offsets(
                &mut self,
                range: TextRange,
                kind: DefinitionKind,
                decorators: &[ast::Decorator],
                name_range: TextRange,
            ) {
                let after_decorators = decorators
                    .last()
                    .map_or(range.start(), |decorator| decorator.range().end());
                let header = self
                    .tokens
                    .iter()
                    .find(|token| {
                        token.range().start() >= after_decorators
                            && token.range().start() < name_range.start()
                            && match kind {
                                DefinitionKind::Function => {
                                    matches!(token.kind(), TokenKind::Def | TokenKind::Async)
                                }
                                DefinitionKind::Class => token.kind() == TokenKind::Class,
                                _ => false,
                            }
                    })
                    .expect("original definition header has a matching parser token");
                let native = self
                    .catalog
                    .native_definitions
                    .get_mut(&(range, kind))
                    .expect("recorded source definition");
                native.header_offset = header.range().start().to_u32();
                if let Some(decorator) = decorators.first() {
                    native.first_offset = decorator.expression.range().start().to_u32();
                }
            }
        }
        impl Transformer for Collector<'_> {
            fn visit_stmt(&mut self, stmt: &mut Stmt) {
                match stmt {
                    Stmt::FunctionDef(function) => {
                        self.record(
                            function.name.as_str(),
                            function.range,
                            DefinitionKind::Function,
                        );
                        self.record_header_offsets(
                            function.range,
                            DefinitionKind::Function,
                            &function.decorator_list,
                            function.name.range(),
                        );
                        for decorator in &mut function.decorator_list {
                            self.visit_decorator(decorator);
                        }
                        if let Some(params) = &mut function.type_params {
                            self.path.push(function.name.to_string());
                            self.visit_type_params(params);
                            self.path.pop();
                        }
                        self.visit_parameters(&mut function.parameters);
                        if let Some(returns) = &mut function.returns {
                            self.visit_annotation(returns);
                        }
                        let body_scope = self
                            .scope
                            .child_scope_for_function(function)
                            .expect("original function has a semantic child scope");
                        let native_path = std::mem::replace(
                            &mut self.native_path,
                            vec![format!("{}.<locals>", body_scope.qualname())],
                        );
                        let scope = std::mem::replace(&mut self.scope, body_scope);
                        self.path.push(format!("{}.<locals>", function.name));
                        self.visit_body(&mut function.body);
                        self.path.pop();
                        self.scope = scope;
                        self.native_path = native_path;
                    }
                    Stmt::ClassDef(class) => {
                        self.record(class.name.as_str(), class.range, DefinitionKind::Class);
                        self.record_header_offsets(
                            class.range,
                            DefinitionKind::Class,
                            &class.decorator_list,
                            class.name.range(),
                        );
                        for decorator in &mut class.decorator_list {
                            self.visit_decorator(decorator);
                        }
                        if let Some(params) = &mut class.type_params {
                            self.path.push(class.name.to_string());
                            self.visit_type_params(params);
                            self.path.pop();
                        }
                        if let Some(arguments) = &mut class.arguments {
                            self.visit_arguments(arguments);
                        }
                        let body_scope = self
                            .scope
                            .child_scope_for_class(class)
                            .expect("original class has a semantic child scope");
                        let native_path = std::mem::replace(
                            &mut self.native_path,
                            vec![body_scope.qualname().to_owned()],
                        );
                        let scope = std::mem::replace(&mut self.scope, body_scope);
                        self.path.push(class.name.to_string());
                        self.visit_body(&mut class.body);
                        self.path.pop();
                        self.scope = scope;
                        self.native_path = native_path;
                    }
                    Stmt::TypeAlias(alias) => {
                        let Expr::Name(name) = alias.name.as_ref() else {
                            unreachable!("type alias target is a source name")
                        };
                        self.record(name.id.as_str(), alias.range, DefinitionKind::TypeAlias);
                        if let Some(params) = &mut alias.type_params {
                            self.path.push(name.id.to_string());
                            self.visit_type_params(params);
                            self.path.pop();
                        }
                        self.visit_expr(&mut alias.value);
                    }
                    other => walk_stmt(self, other),
                }
            }

            fn visit_expr(&mut self, expr: &mut Expr) {
                match expr {
                    Expr::Lambda(lambda) => {
                        self.record("<lambda>", lambda.range, DefinitionKind::Lambda);
                        if let Some(parameters) = &mut lambda.parameters {
                            self.visit_parameters(parameters);
                        }
                        self.path.push("<lambda>".into());
                        self.native_path.push("<lambda>.<locals>".into());
                        self.visit_expr(&mut lambda.body);
                        self.path.pop();
                        self.native_path.pop();
                    }
                    Expr::Call(call) => {
                        self.visit_expr(&mut call.func);
                        // Ruff omits shared call parentheses from a bare
                        // generator argument's own span. CPython's genexp
                        // grammar includes them, including their opening line.
                        // Use the actual parsed Arguments span, never a name,
                        // guessed line, or widened range from matching text.
                        let native_range = call.arguments.range;
                        if call.arguments.keywords.is_empty() {
                            if let [Expr::Generator(generator)] = call.arguments.args.as_mut() {
                                if !generator.parenthesized {
                                    self.visit_generator_expression(generator, native_range);
                                    return;
                                }
                            }
                        }
                        self.visit_arguments(&mut call.arguments);
                    }
                    Expr::Generator(generator) => {
                        let native_range = generator.range;
                        self.visit_generator_expression(generator, native_range);
                    }
                    _ => walk_expr(self, expr),
                }
            }

            fn visit_type_param(&mut self, parameter: &mut ast::TypeParam) {
                let name = match parameter {
                    ast::TypeParam::TypeVar(value) => value.name.as_str(),
                    ast::TypeParam::ParamSpec(value) => value.name.as_str(),
                    ast::TypeParam::TypeVarTuple(value) => value.name.as_str(),
                }
                .to_owned();
                self.record(&name, parameter.range(), DefinitionKind::Parameter);
                crate::transformer::walk_type_param(self, parameter);
            }
        }
        // Later lowering analyzes the rewritten AST separately. This snapshot
        // belongs only to the original definitions whose native names we map.
        let mut original_body = body.clone();
        let source_names = SourceNameCatalog::from_original(&mut original_body);
        let semantic = SemanticAstState::from_ruff(&mut original_body, &source_names);
        let mut collector = Collector {
            facts,
            tokens,
            path: Vec::new(),
            native_path: Vec::new(),
            scope: semantic.module_scope(),
            catalog: Self::default(),
        };
        collector.visit_body(&mut original_body);
        let mut catalog = collector.catalog;
        // Native module annotation setup uses the first original statement's
        // location, including a docstring or future import but not comments.
        // A decorated declaration starts at its header in CPython's statement
        // AST, even though Ruff includes decorators in that statement's range.
        let first_statement_offset = match body.first() {
            Some(Stmt::FunctionDef(function)) => {
                catalog
                    .native_definition(function.range, DefinitionKind::Function)
                    .expect("the first source function is recorded")
                    .header_offset
            }
            Some(Stmt::ClassDef(class)) => {
                catalog
                    .native_definition(class.range, DefinitionKind::Class)
                    .expect("the first source class is recorded")
                    .header_offset
            }
            Some(statement) => statement.start().to_u32(),
            None => 0,
        };
        assert!(catalog
            .native_definitions
            .insert(
                (
                    TextRange::new(0.into(), facts.source_size.into()),
                    DefinitionKind::Module,
                ),
                NativeSourceDefinition {
                    name: "<module>".into(),
                    qualname: "<module>".into(),
                    first_offset: 0,
                    header_offset: first_statement_offset,
                },
            )
            .is_none());
        // A signature authenticates the exporter, not a correspondence guessed
        // from a name. Recheck the exact lexical definition in this parser.
        for identity in facts
            .classes
            .iter()
            .map(|class| &class.identity)
            .chain(facts.functions.iter().map(|function| &function.identity))
        {
            let range = TextRange::new(
                identity.source_range.start.into(),
                identity.source_range.end.into(),
            );
            anyhow::ensure!(
                catalog.definitions.get(&(range, identity.definition_kind)) == Some(identity),
                "offline identity does not match its original source definition: {}",
                identity.lexical_qualname,
            );
        }
        Ok(catalog)
    }

    pub(crate) fn definition(
        &self,
        range: TextRange,
        kind: DefinitionKind,
    ) -> Option<SourceIdentity> {
        self.definitions.get(&(range, kind)).cloned()
    }

    pub(crate) fn generator_expression(
        &self,
        range: TextRange,
    ) -> Option<soac_core::block_py::GeneratorExpressionCode> {
        self.generator_expressions.get(&range).cloned()
    }

    pub(crate) fn native_definition(
        &self,
        range: TextRange,
        kind: DefinitionKind,
    ) -> Option<&NativeSourceDefinition> {
        self.native_definitions.get(&(range, kind))
    }
}
