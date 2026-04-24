macro_rules! py_stmt_internal {
    ($template:literal $(, $name:ident = $value:expr)* $(,)?) => {{
        use std::collections::HashMap;
        #[allow(unused_imports)]
        use crate::template::{PlaceholderValue, SyntaxTemplate};

        #[allow(unused_mut)]
        let mut values: HashMap<&str, PlaceholderValue> = HashMap::new();
        #[allow(unused_mut)]
        let mut ids: HashMap<&str, serde_json::Value> = HashMap::new();
        $(match $crate::template::IntoPlaceholder::into_placeholder($value) {
            Ok(value) => { values.insert(stringify!($name), value); }
            Err(id) => { ids.insert(stringify!($name), id); }
        });*

        static TEMPLATE: ::std::sync::LazyLock<$crate::template::SyntaxTemplate> =
            ::std::sync::LazyLock::new(|| {
                $crate::template::SyntaxTemplate::parse($template)
            });

        use ruff_python_ast::Stmt;
        let template = (*TEMPLATE).clone();
        let values = values.into_iter().map(|(name, value)| (name.to_string(), value)).collect();
        let ids = ids.into_iter().map(|(name, value)| (name.to_string(), value)).collect();
        Stmt::from(template.instantiate_one($template, file!(), line!(), column!(), values, ids))
    }};
}

macro_rules! py_stmts_internal {
    ($template:literal $(, $name:ident = $value:expr)* $(,)?) => {{
        use std::collections::HashMap;
        #[allow(unused_imports)]
        use crate::template::{PlaceholderValue, SyntaxTemplate};

        #[allow(unused_mut)]
        let mut values: HashMap<&str, PlaceholderValue> = HashMap::new();
        #[allow(unused_mut)]
        let mut ids: HashMap<&str, serde_json::Value> = HashMap::new();
        $(match $crate::template::IntoPlaceholder::into_placeholder($value) {
            Ok(value) => { values.insert(stringify!($name), value); }
            Err(id) => { ids.insert(stringify!($name), id); }
        });*

        static TEMPLATE: ::std::sync::LazyLock<$crate::template::SyntaxTemplate> =
            ::std::sync::LazyLock::new(|| {
                $crate::template::SyntaxTemplate::parse($template)
            });

        let template = (*TEMPLATE).clone();
        let values = values.into_iter().map(|(name, value)| (name.to_string(), value)).collect();
        let ids = ids.into_iter().map(|(name, value)| (name.to_string(), value)).collect();
        template.instantiate_suite(values, ids)
    }};
}

macro_rules! py_expr {
    ($template:literal $(, $name:ident = $value:expr)* $(,)?) => {{
        use ruff_python_ast::{self as ast, Stmt};
        let stmt = $crate::py_stmt_internal!($template $(, $name = $value)*);
        match stmt {
            Stmt::Expr(ast::StmtExpr { value, .. }) => *value,
            other => {
                if tracing::enabled!(tracing::Level::TRACE) {
                    tracing::trace!(
                        "py_expr expected expression statement from template `{}`; got {:?}",
                        $template,
                        other
                    );
                }
                panic!("expected expression statement");
            }
        }
    }};
}

macro_rules! py_stmt {
    ($template:literal $(, $name:ident = $value:expr)* $(,)?) => {{
            $crate::py_stmt_internal!($template $(, $name = $value)*)
    }};
}

macro_rules! py_stmts {
    ($template:literal $(, $name:ident = $value:expr)* $(,)?) => {{
            $crate::py_stmts_internal!($template $(, $name = $value)*)
    }};
}

macro_rules! py_stmt_typed {
    ($template:literal $(, $name:ident = $value:expr)* $(,)?) => {{
        let stmt = $crate::py_stmt_internal!($template $(, $name = $value)*);
        $crate::template::expect_stmt::<_>(stmt, $template)
    }};
}

pub(crate) use py_expr;
pub(crate) use py_stmt;
pub(crate) use py_stmt_internal;
pub(crate) use py_stmt_typed;
pub(crate) use py_stmts;
pub(crate) use py_stmts_internal;

use crate::passes::ast_to_ast::body::Suite;
use crate::transformer::{walk_expr, walk_keyword, walk_parameter, walk_stmt, Transformer};
use crate::{namegen::fresh_name, passes::ast_to_ast::simplify::flatten};
use regex::Regex;
use ruff_python_ast::{self as ast, DictItem, Expr, Stmt};
use ruff_python_parser::parse_expression;
use ruff_text_size::TextRange;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
};

pub(crate) trait StmtTryFrom: Sized {
    const EXPECTED: &'static str;
    fn try_from_stmt(stmt: Stmt) -> Result<Self, Stmt>;
}

macro_rules! impl_stmt_try_from {
    ($ty:ty, $variant:ident) => {
        impl StmtTryFrom for $ty {
            const EXPECTED: &'static str = stringify!($ty);
            fn try_from_stmt(stmt: Stmt) -> Result<Self, Stmt> {
                match stmt {
                    Stmt::$variant(val) => Ok(val),
                    other => Err(other),
                }
            }
        }
    };
}

impl StmtTryFrom for Stmt {
    const EXPECTED: &'static str = "Stmt";
    fn try_from_stmt(stmt: Stmt) -> Result<Self, Stmt> {
        Ok(stmt)
    }
}

impl_stmt_try_from!(ast::StmtFunctionDef, FunctionDef);
impl_stmt_try_from!(ast::StmtClassDef, ClassDef);
impl_stmt_try_from!(ast::StmtReturn, Return);
impl_stmt_try_from!(ast::StmtDelete, Delete);
impl_stmt_try_from!(ast::StmtTypeAlias, TypeAlias);
impl_stmt_try_from!(ast::StmtAssign, Assign);
impl_stmt_try_from!(ast::StmtAugAssign, AugAssign);
impl_stmt_try_from!(ast::StmtAnnAssign, AnnAssign);
impl_stmt_try_from!(ast::StmtFor, For);
impl_stmt_try_from!(ast::StmtWhile, While);
impl_stmt_try_from!(ast::StmtIf, If);
impl_stmt_try_from!(ast::StmtWith, With);
impl_stmt_try_from!(ast::StmtMatch, Match);
impl_stmt_try_from!(ast::StmtRaise, Raise);
impl_stmt_try_from!(ast::StmtTry, Try);
impl_stmt_try_from!(ast::StmtAssert, Assert);
impl_stmt_try_from!(ast::StmtImport, Import);
impl_stmt_try_from!(ast::StmtImportFrom, ImportFrom);
impl_stmt_try_from!(ast::StmtGlobal, Global);
impl_stmt_try_from!(ast::StmtNonlocal, Nonlocal);
impl_stmt_try_from!(ast::StmtExpr, Expr);
impl_stmt_try_from!(ast::StmtPass, Pass);
impl_stmt_try_from!(ast::StmtBreak, Break);
impl_stmt_try_from!(ast::StmtContinue, Continue);
impl_stmt_try_from!(ast::StmtIpyEscapeCommand, IpyEscapeCommand);

pub(crate) fn expect_stmt<T: StmtTryFrom>(stmt: Stmt, template: &'static str) -> T {
    match T::try_from_stmt(stmt) {
        Ok(value) => value,
        Err(other) => panic!(
            "py_stmt expected {}, got {:?} (template: {})",
            T::EXPECTED,
            other,
            template,
        ),
    }
}

fn body_from_stmts(stmts: Vec<Stmt>) -> Suite {
    stmts
}

pub(crate) fn is_simple(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Name(_)
            | Expr::NumberLiteral(_)
            | Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
            | Expr::EllipsisLiteral(_)
    )
}

pub(crate) enum PlaceholderValue {
    Expr(Box<Expr>),
    Stmt(Vec<Stmt>),
}

pub(crate) struct DictEntries<I>(I);

fn expand_body_stmt(stmt: Stmt) -> Vec<Stmt> {
    vec![stmt]
}

pub(crate) trait IntoPlaceholder {
    fn into_placeholder(self) -> Result<PlaceholderValue, Value>;
}

impl IntoPlaceholder for Expr {
    fn into_placeholder(self) -> Result<PlaceholderValue, Value> {
        Ok(PlaceholderValue::Expr(Box::new(self)))
    }
}

impl IntoPlaceholder for Box<Expr> {
    fn into_placeholder(self) -> Result<PlaceholderValue, Value> {
        Ok(PlaceholderValue::Expr(self))
    }
}

impl IntoPlaceholder for &str {
    fn into_placeholder(self) -> Result<PlaceholderValue, Value> {
        Err(Value::String(self.to_string()))
    }
}

impl IntoPlaceholder for String {
    fn into_placeholder(self) -> Result<PlaceholderValue, Value> {
        Err(Value::String(self))
    }
}

impl IntoPlaceholder for Stmt {
    fn into_placeholder(self) -> Result<PlaceholderValue, Value> {
        Ok(PlaceholderValue::Stmt(expand_body_stmt(self)))
    }
}

impl IntoPlaceholder for Box<Stmt> {
    fn into_placeholder(self) -> Result<PlaceholderValue, Value> {
        Ok(PlaceholderValue::Stmt(expand_body_stmt(*self)))
    }
}

macro_rules! impl_into_placeholder_for_stmt {
    ($($ty:ty),* $(,)?) => {
        $(impl IntoPlaceholder for $ty {
            fn into_placeholder(self) -> Result<PlaceholderValue, Value> {
                Ok(PlaceholderValue::Stmt(expand_body_stmt(self.into())))
            }
        })*
    };
}

impl_into_placeholder_for_stmt!(
    ast::StmtFunctionDef,
    ast::StmtClassDef,
    ast::StmtReturn,
    ast::StmtDelete,
    ast::StmtTypeAlias,
    ast::StmtAssign,
    ast::StmtAugAssign,
    ast::StmtAnnAssign,
    ast::StmtFor,
    ast::StmtWhile,
    ast::StmtIf,
    ast::StmtWith,
    ast::StmtMatch,
    ast::StmtRaise,
    ast::StmtTry,
    ast::StmtAssert,
    ast::StmtImport,
    ast::StmtImportFrom,
    ast::StmtGlobal,
    ast::StmtNonlocal,
    ast::StmtExpr,
    ast::StmtPass,
    ast::StmtBreak,
    ast::StmtContinue,
    ast::StmtIpyEscapeCommand,
);

impl IntoPlaceholder for &Suite {
    fn into_placeholder(self) -> Result<PlaceholderValue, Value> {
        Ok(PlaceholderValue::Stmt(self.to_vec()))
    }
}

impl IntoPlaceholder for Vec<Stmt> {
    fn into_placeholder(self) -> Result<PlaceholderValue, Value> {
        let stmts = self
            .into_iter()
            .flat_map(expand_body_stmt)
            .collect::<Vec<_>>();
        if stmts.is_empty() {
            return Ok(PlaceholderValue::Stmt(vec![Stmt::Pass(ast::StmtPass {
                node_index: Default::default(),
                range: Default::default(),
            })]));
        }
        Ok(PlaceholderValue::Stmt(stmts))
    }
}

impl IntoPlaceholder for std::vec::IntoIter<Stmt> {
    fn into_placeholder(self) -> Result<PlaceholderValue, Value> {
        let mut stmts: Vec<Stmt> = self.flat_map(expand_body_stmt).collect();
        if stmts.is_empty() {
            stmts.push(Stmt::Pass(ast::StmtPass {
                node_index: Default::default(),
                range: Default::default(),
            }));
        }
        Ok(PlaceholderValue::Stmt(stmts))
    }
}

impl<K> IntoPlaceholder for Vec<(K, Expr)>
where
    K: Into<String>,
{
    fn into_placeholder(self) -> Result<PlaceholderValue, Value> {
        DictEntries(self).into_placeholder()
    }
}

impl<K, I> IntoPlaceholder for DictEntries<I>
where
    I: IntoIterator<Item = (K, Expr)>,
    K: Into<String>,
{
    fn into_placeholder(self) -> Result<PlaceholderValue, Value> {
        Ok(PlaceholderValue::Expr(Box::new(dict_expr_from_entries(
            self.0,
        ))))
    }
}

macro_rules! impl_into_placeholder_for_signed {
    ($($ty:ty),*) => {
        $(impl IntoPlaceholder for $ty {
            fn into_placeholder(self) -> Result<PlaceholderValue, Value> {
                Err(Value::Number(serde_json::Number::from(self as i64)))
            }
        })*
    };
}

macro_rules! impl_into_placeholder_for_unsigned {
    ($($ty:ty),*) => {
        $(impl IntoPlaceholder for $ty {
            fn into_placeholder(self) -> Result<PlaceholderValue, Value> {
                Err(Value::Number(serde_json::Number::from(self as u64)))
            }
        })*
    };
}

impl_into_placeholder_for_signed!(i8, i16, i32, i64, isize);
impl_into_placeholder_for_unsigned!(u8, u16, u32, u64, usize);

impl IntoPlaceholder for bool {
    fn into_placeholder(self) -> Result<PlaceholderValue, Value> {
        Err(Value::Bool(self))
    }
}

pub(crate) fn var_for_placeholder(name: &str, ty: PlaceholderType) -> String {
    match ty {
        PlaceholderType::Expr => format!("_dp_placeholder_expr_{}__", name),
        PlaceholderType::Stmt => format!("_dp_placeholder_stmt_{}__", name),
        PlaceholderType::Identifier => format!("_dp_placeholder_id_{}__", name),
        PlaceholderType::Literal => format!("_dp_placeholder_literal_{}__", name),
        PlaceholderType::TmpName => format!("_dp_placeholder_tmpname_{}__", name),
        PlaceholderType::Dict => format!("_dp_placeholder_dict_{}__", name),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlaceholderType {
    Expr,
    Stmt,
    Identifier,
    Literal,
    TmpName,
    Dict,
}

#[derive(Clone, Debug)]
pub(crate) struct SyntaxTemplate {
    stmts: Suite,
}

impl SyntaxTemplate {
    pub(crate) fn parse(template: &str) -> Self {
        let regex = placeholder_template_regex();
        let src = regex
            .replace_all(template, |caps: &regex::Captures| {
                let name = caps.get(1).unwrap().as_str();
                let ty = match caps.get(2).unwrap().as_str() {
                    "expr" => PlaceholderType::Expr,
                    "stmt" => PlaceholderType::Stmt,
                    "id" => PlaceholderType::Identifier,
                    "literal" => PlaceholderType::Literal,
                    "tmpname" => PlaceholderType::TmpName,
                    "dict" => PlaceholderType::Dict,
                    other => panic!("unknown placeholder type `{other}` for `{name}`"),
                };
                var_for_placeholder(name, ty)
            })
            .to_string();

        let mut module = match ruff_python_parser::parse_module(&src) {
            Ok(module) => module.into_syntax(),
            Err(e) => {
                println!("template parse error: {}\n{}", e, src);
                panic!("template parse error");
            }
        };

        Self {
            stmts: std::mem::take(&mut module.body),
        }
    }

    pub(crate) fn instantiate_suite(
        mut self,
        values: HashMap<String, PlaceholderValue>,
        ids: HashMap<String, Value>,
    ) -> Vec<Stmt> {
        let mut transformer = PlaceholderReplacer::new(values, ids);
        transformer.visit_body(&mut self.stmts);
        transformer.finish();
        flatten(&mut self.stmts);
        self.stmts
    }

    pub(crate) fn instantiate_one(
        self,
        template_source: &'static str,
        call_file: &'static str,
        call_line: u32,
        call_column: u32,
        values: HashMap<String, PlaceholderValue>,
        ids: HashMap<String, Value>,
    ) -> Stmt {
        let mut stmts = self.instantiate_suite(values, ids);
        match stmts.len() {
            1 => stmts.remove(0),
            len => {
                let expanded = crate::ruff_ast::ruff_ast_to_string(stmts.as_slice());
                panic!(
                    "py_stmt template at {call_file}:{call_line}:{call_column} must produce exactly one statement, got {len}\n\
                     template:\n{template_source}\n\
                     expanded statements:\n{}",
                    expanded.trim_end()
                );
            }
        }
    }
}

struct PlaceholderReplacer {
    values: HashMap<String, PlaceholderValue>,
    ids: HashMap<String, Value>,
    tmpnames: HashMap<String, String>,
    used_ids: HashSet<String>,
    errors: Vec<String>,
}

impl PlaceholderReplacer {
    fn new(values: HashMap<String, PlaceholderValue>, ids: HashMap<String, Value>) -> Self {
        Self {
            values,
            ids,
            tmpnames: HashMap::new(),
            used_ids: HashSet::new(),
            errors: Vec::new(),
        }
    }

    fn parse_placeholder<'b>(&self, symbol: &'b str) -> Option<(PlaceholderType, &'b str)> {
        placeholder_regex().captures(symbol).map(|caps| {
            let ty = match caps.name("ty").unwrap().as_str() {
                "expr" => PlaceholderType::Expr,
                "stmt" => PlaceholderType::Stmt,
                "id" => PlaceholderType::Identifier,
                "literal" => PlaceholderType::Literal,
                "tmpname" => PlaceholderType::TmpName,
                "dict" => PlaceholderType::Dict,
                other => panic!("unknown placeholder type `{other}`"),
            };
            let name = caps.name("name").unwrap().as_str();
            (ty, name)
        })
    }

    fn take_value(&mut self, name: &str) -> PlaceholderValue {
        self.values.remove(name).unwrap_or_else(|| {
            self.errors
                .push(format!("expected value for placeholder {name}"));
            PlaceholderValue::Expr(Box::new(py_expr!("None")))
        })
    }

    fn get_id(&mut self, name: &str) -> Value {
        match self.ids.get(name) {
            Some(value) => {
                self.used_ids.insert(name.to_string());
                value.clone()
            }
            None => {
                self.errors
                    .push(format!("expected id or literal for placeholder {name}"));
                Value::String(format!("_dp_missing_{name}"))
            }
        }
    }

    fn get_tmpname(&mut self, name: &str) -> String {
        if let Some(value) = self.tmpnames.get(name) {
            return value.clone();
        }
        let value = fresh_name("tmp");
        self.tmpnames.insert(name.to_string(), value.clone());
        value
    }

    fn replace_identifier(&mut self, identifier: &mut ast::Identifier) {
        if let Some((ty, name)) = self.parse_placeholder(identifier.id.as_str()) {
            match ty {
                PlaceholderType::Identifier => {
                    let value = self.get_id(name);
                    identifier.id = identifier_string(name, value).into();
                    return;
                }
                PlaceholderType::TmpName => {
                    identifier.id = self.get_tmpname(name).into();
                    return;
                }
                PlaceholderType::Literal
                | PlaceholderType::Expr
                | PlaceholderType::Stmt
                | PlaceholderType::Dict => {
                    panic!("unexpected placeholder `{name}` in identifier context");
                }
            }
        }

        let original = identifier.id.as_str();
        let regex = placeholder_text_regex();
        if regex.is_match(original) {
            let mut result = String::with_capacity(original.len());
            let mut last_end = 0;
            for caps in regex.captures_iter(original) {
                let mat = caps.get(0).unwrap();
                result.push_str(&original[last_end..mat.start()]);
                let ty = caps.name("ty").unwrap().as_str();
                let name = caps.name("name").unwrap().as_str();
                match ty {
                    "id" => {
                        let value = self.get_id(name);
                        result.push_str(&identifier_string(name, value));
                    }
                    "tmpname" => {
                        let value = self.get_tmpname(name);
                        result.push_str(&value);
                    }
                    other => panic!("unsupported placeholder type `{other}` in identifier"),
                }
                last_end = mat.end();
            }
            result.push_str(&original[last_end..]);
            identifier.id = result.into();
        }
    }

    fn replace_optional_identifier(&mut self, identifier: &mut Option<ast::Identifier>) {
        if let Some(identifier) = identifier {
            self.replace_identifier(identifier);
        }
    }

    fn replace_name(&mut self, name: &mut ruff_python_ast::name::Name) {
        let original = name.as_str();
        let regex = placeholder_text_regex();
        if regex.is_match(original) {
            let mut result = String::with_capacity(original.len());
            let mut last_end = 0;
            for caps in regex.captures_iter(original) {
                let mat = caps.get(0).unwrap();
                result.push_str(&original[last_end..mat.start()]);
                let ty = caps.name("ty").unwrap().as_str();
                let placeholder = caps.name("name").unwrap().as_str();
                match ty {
                    "id" => {
                        let value = self.get_id(placeholder);
                        result.push_str(&identifier_string(placeholder, value));
                    }
                    "tmpname" => {
                        let value = self.get_tmpname(placeholder);
                        result.push_str(&value);
                    }
                    other => panic!("unsupported placeholder type `{other}` in name"),
                }
                last_end = mat.end();
            }
            result.push_str(&original[last_end..]);
            *name = result.into();
        }
    }

    fn finish(mut self) {
        if !self.values.is_empty() {
            let mut keys: Vec<_> = self.values.keys().cloned().collect();
            keys.sort();
            self.errors
                .push(format!("unused placeholders: {}", keys.join(", ")));
        }

        let mut unused_ids: Vec<_> = self
            .ids
            .keys()
            .filter(|name| !self.used_ids.contains(*name))
            .cloned()
            .collect();
        if !unused_ids.is_empty() {
            unused_ids.sort();
            self.errors
                .push(format!("unused ids: {}", unused_ids.join(", ")));
        }

        if !self.errors.is_empty() {
            panic!("template errors:\n{}", self.errors.join("\n"));
        }
    }
}

impl Transformer for PlaceholderReplacer {
    fn visit_expr(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Name(ast::ExprName { id, .. }) => {
                if let Some((placeholder_type, name)) = self.parse_placeholder(id.as_str()) {
                    match placeholder_type {
                        PlaceholderType::Expr => match self.take_value(name) {
                            PlaceholderValue::Expr(value) => {
                                *expr = *value;
                                return;
                            }
                            PlaceholderValue::Stmt(_) => {
                                panic!("expected expr for placeholder {name}");
                            }
                        },
                        PlaceholderType::Identifier => {
                            let value = self.get_id(name);
                            *expr = identifier_expr(name, value);
                            return;
                        }
                        PlaceholderType::TmpName => {
                            let value = self.get_tmpname(name);
                            *expr = identifier_expr(name, Value::String(value));
                            return;
                        }
                        PlaceholderType::Literal => {
                            let value = self.get_id(name);
                            *expr = literal_expr(name, value);
                            return;
                        }
                        PlaceholderType::Dict => match self.take_value(name) {
                            PlaceholderValue::Expr(value) => {
                                *expr = *value;
                                return;
                            }
                            PlaceholderValue::Stmt(_) => {
                                panic!("expected dict expression for placeholder {name}");
                            }
                        },
                        PlaceholderType::Stmt => {
                            panic!("expected stmt placeholder {name}");
                        }
                    }
                }
                self.replace_name(id);
            }
            Expr::Attribute(ast::ExprAttribute { attr, .. }) => {
                self.replace_identifier(attr);
            }
            _ => {}
        }
        walk_expr(self, expr);
    }

    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::FunctionDef(func) => {
                self.replace_identifier(&mut func.name);
                walk_stmt(self, stmt);
                return;
            }
            Stmt::ClassDef(class_def) => {
                self.replace_identifier(&mut class_def.name);
            }
            Stmt::Global(ast::StmtGlobal { names, .. })
            | Stmt::Nonlocal(ast::StmtNonlocal { names, .. }) => {
                for name in names {
                    self.replace_identifier(name);
                }
                return;
            }
            _ => {}
        }

        match stmt {
            Stmt::Expr(ast::StmtExpr { value, .. }) => {
                if let Expr::Name(ast::ExprName { id, .. }) = value.as_ref() {
                    if let Some((placeholder_type, name)) = self.parse_placeholder(id.as_str()) {
                        if matches!(placeholder_type, PlaceholderType::Stmt) {
                            match self.take_value(name) {
                                PlaceholderValue::Stmt(value) => {
                                    *stmt = Stmt::If(ast::StmtIf {
                                        node_index: ast::AtomicNodeIndex::default(),
                                        range: TextRange::default(),
                                        test: Box::new(py_expr!("True")),
                                        body: body_from_stmts(value),
                                        elif_else_clauses: Vec::new(),
                                    });
                                }
                                PlaceholderValue::Expr(value) => {
                                    *stmt = Stmt::If(ast::StmtIf {
                                        node_index: ast::AtomicNodeIndex::default(),
                                        range: TextRange::default(),
                                        test: Box::new(crate::py_expr!("True")),
                                        body: body_from_stmts(vec![Stmt::Expr(ast::StmtExpr {
                                            node_index: ast::AtomicNodeIndex::default(),
                                            range: TextRange::default(),
                                            value,
                                        })]),
                                        elif_else_clauses: Vec::new(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        walk_stmt(self, stmt);
    }

    fn visit_parameter(&mut self, parameter: &mut ast::Parameter) {
        self.replace_identifier(&mut parameter.name);
        walk_parameter(self, parameter);
    }

    fn visit_keyword(&mut self, keyword: &mut ast::Keyword) {
        self.replace_optional_identifier(&mut keyword.arg);
        walk_keyword(self, keyword);
    }

    fn visit_alias(&mut self, alias: &mut ast::Alias) {
        self.replace_identifier(&mut alias.name);
        self.replace_optional_identifier(&mut alias.asname);
    }
}

fn placeholder_regex() -> &'static Regex {
    static REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^_dp_placeholder_(?P<ty>expr|stmt|id|literal|tmpname|dict)_(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)__$",
        )
        .unwrap()
    });
    &REGEX
}

fn placeholder_template_regex() -> &'static Regex {
    static REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\{([a-zA-Z_][a-zA-Z0-9_]*)\:([a-zA-Z_][a-zA-Z0-9_]*)\}").unwrap()
    });
    &REGEX
}

fn placeholder_text_regex() -> &'static Regex {
    static REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"_dp_placeholder_(?P<ty>expr|stmt|id|literal|tmpname|dict)_(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)__",
        )
        .unwrap()
    });
    &REGEX
}

fn dict_expr_from_entries<K, I>(entries: I) -> Expr
where
    I: IntoIterator<Item = (K, Expr)>,
    K: Into<String>,
{
    let items = entries
        .into_iter()
        .map(|(key, value)| DictItem {
            key: Some(literal_expr("dict_key", Value::String(key.into()))),
            value,
        })
        .collect();
    Expr::Dict(ast::ExprDict {
        node_index: ast::AtomicNodeIndex::default(),
        range: TextRange::default(),
        items,
    })
}

fn identifier_string(name: &str, value: Value) -> String {
    match value {
        Value::String(value) => value,
        other => panic!("expected string identifier for placeholder `{name}`, got {other:?}"),
    }
}

fn identifier_expr(name: &str, value: Value) -> Expr {
    let identifier = identifier_string(name, value);
    parse_expression(&identifier)
        .map(|expr| *expr.into_syntax().body)
        .unwrap_or_else(|err| {
            panic!("failed to parse identifier `{identifier}` for placeholder `{name}`: {err}")
        })
}

fn literal_expr(name: &str, value: Value) -> Expr {
    match value {
        Value::Bool(true) => parse_constant_expr("True", name),
        Value::Bool(false) => parse_constant_expr("False", name),
        Value::Null => parse_constant_expr("None", name),
        Value::String(value) => {
            let src = serde_json::to_string(&value).expect("failed to serialize literal");
            parse_dynamic_expr(&src, name)
        }
        Value::Number(value) => parse_dynamic_expr(&value.to_string(), name),
        other => panic!("unsupported literal for placeholder `{name}`: {other:?}"),
    }
}

fn parse_constant_expr(src: &str, name: &str) -> Expr {
    parse_dynamic_expr(src, name)
}

fn parse_dynamic_expr(src: &str, name: &str) -> Expr {
    parse_expression(src)
        .map(|expr| *expr.into_syntax().body)
        .unwrap_or_else(|err| {
            panic!("failed to parse literal `{src}` for placeholder `{name}`: {err}")
        })
}

#[cfg(test)]
mod test;
