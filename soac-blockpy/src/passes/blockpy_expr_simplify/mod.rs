use super::ast_to_ast::string_templates::lower_string_templates_in_expr;
use crate::block_py::{
    core_call_expr_with_meta, core_runtime_name_expr_with_meta,
    core_runtime_positional_call_expr_with_meta, literal_expr, operation, Await, BytesLiteral,
    CallArgKeyword, CallArgPositional, HasMeta, InstrWithAwaitAndYield, InstrWithConstantNone,
    Meta, NumberLiteral, NumberLiteralValue, StringLiteral, WithMeta, Yield, YieldFrom,
};
use crate::passes::InstrRuff;
use crate::py_expr;
use ruff_python_ast::{self as ast, Expr};

fn core_builtin_name(id: &str) -> InstrWithAwaitAndYield {
    core_runtime_name_expr_with_meta(id, Default::default(), Default::default())
}

fn number_literal_expr_with_meta(
    value: NumberLiteralValue,
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
) -> InstrWithAwaitAndYield {
    literal_expr(NumberLiteral { value }, Meta::new(node_index, range))
}

fn tuple_from_ast_exprs_with_meta(
    values: Vec<Expr>,
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
) -> InstrWithAwaitAndYield {
    operation::Tuple::new(
        values
            .into_iter()
            .map(InstrWithAwaitAndYield::from_ast_expr)
            .collect::<Vec<_>>(),
    )
    .with_meta(Meta::new(node_index, range))
    .into()
}

fn complex_literal_expr_with_meta(
    real: f64,
    imag: f64,
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
) -> InstrWithAwaitAndYield {
    core_runtime_positional_call_expr_with_meta(
        "complex_from_parts",
        node_index.clone(),
        range,
        vec![
            number_literal_expr_with_meta(
                NumberLiteralValue::Float(real),
                node_index.clone(),
                range,
            ),
            number_literal_expr_with_meta(NumberLiteralValue::Float(imag), node_index, range),
        ],
    )
}

fn reduce_core_blockpy_dict(items: Box<[ast::DictItem]>) -> InstrWithAwaitAndYield {
    let mut segments: Vec<InstrWithAwaitAndYield> = Vec::new();
    let mut keyed_pairs = Vec::new();

    for item in items {
        match item {
            ast::DictItem {
                key: Some(key),
                value,
            } => {
                keyed_pairs.push(py_expr!(
                    "({key:expr}, {value:expr})",
                    key = key,
                    value = value,
                ));
            }
            ast::DictItem { key: None, value } => {
                if !keyed_pairs.is_empty() {
                    let tuple = tuple_from_ast_exprs_with_meta(
                        std::mem::take(&mut keyed_pairs),
                        ast::AtomicNodeIndex::default(),
                        Default::default(),
                    );
                    segments.push(core_runtime_positional_call_expr_with_meta(
                        "dict",
                        ast::AtomicNodeIndex::default(),
                        Default::default(),
                        vec![tuple],
                    ));
                }
                segments.push(InstrWithAwaitAndYield::from_ast_expr(py_expr!(
                    "__soac__.dict({mapping:expr})",
                    mapping = value
                )));
            }
        }
    }

    if !keyed_pairs.is_empty() {
        let tuple = tuple_from_ast_exprs_with_meta(
            keyed_pairs,
            ast::AtomicNodeIndex::default(),
            Default::default(),
        );
        segments.push(core_runtime_positional_call_expr_with_meta(
            "dict",
            ast::AtomicNodeIndex::default(),
            Default::default(),
            vec![tuple],
        ));
    }

    let expr = match segments.len() {
        0 => core_runtime_positional_call_expr_with_meta(
            "dict",
            ast::AtomicNodeIndex::default(),
            Default::default(),
            Vec::new(),
        ),
        _ => segments
            .into_iter()
            .reduce(|left, right| {
                core_operation_expr(operation::BinOp::new(
                    operation::BinOpKind::Or,
                    Box::new(left),
                    Box::new(right),
                ))
            })
            .expect("dict segments are non-empty"),
    };
    expr
}

fn core_operation_expr(operation: impl Into<InstrWithAwaitAndYield>) -> InstrWithAwaitAndYield {
    operation.into()
}

fn core_operation_expr_with_meta(
    detail: impl Into<InstrWithAwaitAndYield>,
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
) -> InstrWithAwaitAndYield {
    core_operation_expr(detail.into().with_meta(Meta::new(node_index, range)))
}

fn unary_op_expr_with_meta(
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
    kind: operation::UnaryOpKind,
    operand: InstrWithAwaitAndYield,
) -> InstrWithAwaitAndYield {
    core_operation_expr_with_meta(
        operation::UnaryOp::new(kind, Box::new(operand)),
        node_index,
        range,
    )
}

fn binop_expr_with_meta(
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
    kind: operation::BinOpKind,
    left: InstrWithAwaitAndYield,
    right: InstrWithAwaitAndYield,
) -> InstrWithAwaitAndYield {
    core_operation_expr_with_meta(
        operation::BinOp::new(kind, Box::new(left), Box::new(right)),
        node_index,
        range,
    )
}

fn getattr_expr_with_meta(
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
    value: InstrWithAwaitAndYield,
    attr: String,
) -> InstrWithAwaitAndYield {
    let attr_expr = literal_expr(
        StringLiteral { value: attr },
        Meta::new(node_index.clone(), range),
    );
    core_operation_expr_with_meta(
        operation::GetAttr::new(Box::new(value), Box::new(attr_expr)),
        node_index,
        range,
    )
}

fn getitem_expr_with_meta(
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
    value: InstrWithAwaitAndYield,
    index: InstrWithAwaitAndYield,
) -> InstrWithAwaitAndYield {
    core_operation_expr_with_meta(
        operation::GetItem::new(Box::new(value), Box::new(index)),
        node_index,
        range,
    )
}

fn unary_op_expr_from_ast_with_meta(
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
    op: ast::UnaryOp,
    operand: InstrWithAwaitAndYield,
) -> InstrWithAwaitAndYield {
    unary_op_expr_with_meta(
        node_index,
        range,
        operation::UnaryOpKind::from_ast_unary_op(op),
        operand,
    )
}

fn binop_expr_from_ast_with_meta(
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
    op: ast::Operator,
    left: InstrWithAwaitAndYield,
    right: InstrWithAwaitAndYield,
) -> InstrWithAwaitAndYield {
    match op {
        ast::Operator::Add => add_op_expr_with_meta(node_index, range, left, right),
        _ => binop_expr_with_meta(
            node_index,
            range,
            operation::BinOpKind::from_ast_operator(op),
            left,
            right,
        ),
    }
}

fn compare_expr_from_ast_with_meta(
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
    op: ast::CmpOp,
    left: InstrWithAwaitAndYield,
    right: InstrWithAwaitAndYield,
) -> InstrWithAwaitAndYield {
    match op {
        ast::CmpOp::Eq => {
            binop_expr_with_meta(node_index, range, operation::BinOpKind::Eq, left, right)
        }
        ast::CmpOp::NotEq => {
            binop_expr_with_meta(node_index, range, operation::BinOpKind::Ne, left, right)
        }
        ast::CmpOp::Lt => {
            binop_expr_with_meta(node_index, range, operation::BinOpKind::Lt, left, right)
        }
        ast::CmpOp::LtE => {
            binop_expr_with_meta(node_index, range, operation::BinOpKind::Le, left, right)
        }
        ast::CmpOp::Gt => {
            binop_expr_with_meta(node_index, range, operation::BinOpKind::Gt, left, right)
        }
        ast::CmpOp::GtE => {
            binop_expr_with_meta(node_index, range, operation::BinOpKind::Ge, left, right)
        }
        ast::CmpOp::Is => {
            binop_expr_with_meta(node_index, range, operation::BinOpKind::Is, left, right)
        }
        ast::CmpOp::IsNot => unary_op_expr_with_meta(
            node_index.clone(),
            range,
            operation::UnaryOpKind::Not,
            binop_expr_with_meta(node_index, range, operation::BinOpKind::Is, left, right),
        ),
        ast::CmpOp::In => binop_expr_with_meta(
            node_index,
            range,
            operation::BinOpKind::Contains,
            right,
            left,
        ),
        ast::CmpOp::NotIn => unary_op_expr_with_meta(
            node_index.clone(),
            range,
            operation::UnaryOpKind::Not,
            binop_expr_with_meta(
                node_index,
                range,
                operation::BinOpKind::Contains,
                right,
                left,
            ),
        ),
    }
}

fn add_op_expr_with_meta(
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
    left: InstrWithAwaitAndYield,
    right: InstrWithAwaitAndYield,
) -> InstrWithAwaitAndYield {
    binop_expr_with_meta(node_index, range, operation::BinOpKind::Add, left, right)
}

fn add_op_expr(
    left: InstrWithAwaitAndYield,
    right: InstrWithAwaitAndYield,
) -> InstrWithAwaitAndYield {
    add_op_expr_with_meta(
        ast::AtomicNodeIndex::default(),
        Default::default(),
        left,
        right,
    )
}

fn lower_core_call_args(args: Vec<Expr>) -> Vec<CallArgPositional<InstrWithAwaitAndYield>> {
    args.into_iter()
        .map(|arg| {
            CallArgPositional::from_ast_expr_with(arg, InstrWithAwaitAndYield::from_ast_expr)
        })
        .collect()
}

fn lower_core_call_keywords(
    keywords: Vec<ast::Keyword>,
) -> Vec<CallArgKeyword<InstrWithAwaitAndYield>> {
    keywords
        .into_iter()
        .map(|keyword| {
            CallArgKeyword::from_ast_keyword_with(keyword, InstrWithAwaitAndYield::from_ast_expr)
        })
        .collect()
}

fn make_function_kind_from_literal(expr: &Expr) -> Option<crate::block_py::FunctionKind> {
    let Expr::StringLiteral(node) = expr else {
        return None;
    };
    match node.value.to_str() {
        "function" => Some(crate::block_py::FunctionKind::Function),
        "coroutine" => Some(crate::block_py::FunctionKind::Coroutine),
        "generator" => Some(crate::block_py::FunctionKind::Generator),
        "async_generator" => Some(crate::block_py::FunctionKind::AsyncGenerator),
        _ => None,
    }
}

fn make_function_id_from_literal(expr: &Expr) -> Option<crate::block_py::FunctionId> {
    let Expr::NumberLiteral(node) = expr else {
        return None;
    };
    let ast::Number::Int(value) = &node.value else {
        return None;
    };
    value
        .to_string()
        .parse()
        .ok()
        .map(crate::block_py::FunctionId::from_packed_runtime_u64)
}

fn string_arg_from_core_expr(expr: InstrWithAwaitAndYield) -> Option<String> {
    let InstrWithAwaitAndYield::Literal(literal) = expr else {
        return None;
    };
    let crate::block_py::Literal::StringLiteral(literal) = literal.into_literal() else {
        return None;
    };
    Some(literal.value)
}

fn non_operator_operation_from_helper_call(
    name: &str,
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
    args: Vec<InstrWithAwaitAndYield>,
) -> Option<InstrWithAwaitAndYield> {
    let mut args = args.into_iter();
    let meta = Meta::new(node_index, range);
    let operation = match name {
        "store_global" => operation::Store::new(
            ast::name::Name::new({
                let _globals = args.next()?;
                string_arg_from_core_expr(args.next()?)?
            }),
            Box::new(args.next()?),
        )
        .with_meta(meta)
        .into(),
        "cell_ref" => operation::CellRefForName::new(string_arg_from_core_expr(args.next()?)?)
            .with_meta(meta)
            .into(),
        _ => return None,
    };
    if args.next().is_some() {
        return None;
    }
    Some(operation)
}

fn lower_core_call_expr_with_meta(
    func: Expr,
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
    args: Vec<Expr>,
    keywords: Vec<ast::Keyword>,
) -> InstrWithAwaitAndYield {
    if keywords.is_empty() {
        if let Expr::Attribute(attr) = &func {
            if matches!(attr.value.as_ref(), Expr::Name(base) if base.id.as_str() == "__soac__")
                && attr.attr.id.as_str() == "make_function"
                && args.len() == 5
            {
                if let (Some(function_id), Some(kind)) = (
                    make_function_id_from_literal(&args[0]),
                    make_function_kind_from_literal(&args[1]),
                ) {
                    return core_operation_expr(
                        operation::MakeFunction::new(
                            function_id,
                            kind,
                            Box::new(InstrWithAwaitAndYield::from_ast_expr(args[3].clone())),
                            Box::new(InstrWithAwaitAndYield::from_ast_expr(args[4].clone())),
                        )
                        .with_meta(Meta::new(node_index, range)),
                    );
                }
            }
        }
        if let Expr::Attribute(attr) = &func {
            if matches!(attr.value.as_ref(), Expr::Name(base) if base.id.as_str() == "__soac__") {
                let mut operation_args = Vec::with_capacity(args.len());
                let mut saw_starred = false;
                for arg in &args {
                    if matches!(arg, Expr::Starred(_)) {
                        saw_starred = true;
                        break;
                    }
                }
                if !saw_starred {
                    for arg in &args {
                        operation_args.push(InstrWithAwaitAndYield::from_ast_expr(arg.clone()));
                    }
                    if let Some(operation) = non_operator_operation_from_helper_call(
                        attr.attr.id.as_str(),
                        node_index.clone(),
                        range,
                        operation_args,
                    ) {
                        return core_operation_expr(operation);
                    }
                }
            }
        }
    }

    core_call_expr_with_meta(
        InstrWithAwaitAndYield::from_ast_expr(func),
        node_index,
        range,
        lower_core_call_args(args),
        lower_core_call_keywords(keywords),
    )
}

fn reduce_core_tuple_splat(elts: Vec<Expr>) -> InstrWithAwaitAndYield {
    let mut segments: Vec<InstrWithAwaitAndYield> = Vec::new();
    let mut values: Vec<InstrWithAwaitAndYield> = Vec::new();

    fn tuple_pack(values: Vec<InstrWithAwaitAndYield>) -> InstrWithAwaitAndYield {
        operation::Tuple::new(values)
            .with_meta(Meta::new(
                ast::AtomicNodeIndex::default(),
                Default::default(),
            ))
            .into()
    }

    for elt in elts {
        match elt {
            Expr::Starred(ast::ExprStarred {
                value,
                node_index,
                range,
                ..
            }) => {
                if !values.is_empty() {
                    segments.push(tuple_pack(std::mem::take(&mut values)));
                }
                segments.push(core_runtime_positional_call_expr_with_meta(
                    "tuple_from_iter",
                    node_index,
                    range,
                    vec![InstrWithAwaitAndYield::from_ast_expr(*value)],
                ));
            }
            other => values.push(InstrWithAwaitAndYield::from_ast_expr(other)),
        }
    }

    if !values.is_empty() {
        segments.push(tuple_pack(values));
    }

    segments
        .into_iter()
        .reduce(add_op_expr)
        .unwrap_or_else(|| tuple_pack(Vec::new()))
}

impl InstrWithAwaitAndYield {
    pub(crate) fn from_ruff_expr(value: InstrRuff) -> Self {
        match value {
            InstrRuff::Await(node) => {
                let meta = node.meta();
                Self::Await(
                    Await::new(Self::from_ruff_expr(*node.value))
                        .with_meta(Meta::new(meta.node_index, meta.range)),
                )
            }
            InstrRuff::Yield(node) => {
                let meta = node.meta();
                Self::Yield(
                    Yield::new(Self::from_ruff_expr(*node.value))
                        .with_meta(Meta::new(meta.node_index, meta.range)),
                )
            }
            InstrRuff::YieldFrom(node) => {
                let meta = node.meta();
                Self::YieldFrom(
                    YieldFrom::new(Self::from_ruff_expr(*node.value))
                        .with_meta(Meta::new(meta.node_index, meta.range)),
                )
            }
            InstrRuff::ExprStringLiteral(node) => literal_expr(
                StringLiteral {
                    value: node.value.to_str().to_string(),
                },
                Meta::new(node.meta().node_index, node.meta().range),
            ),
            InstrRuff::ExprBytesLiteral(node) => literal_expr(
                BytesLiteral {
                    value: {
                        let value: std::borrow::Cow<[u8]> = (&node.value).into();
                        value.into_owned()
                    },
                },
                Meta::new(node.meta().node_index, node.meta().range),
            ),
            InstrRuff::ExprNumberLiteral(node) => {
                let meta = node.meta();
                match node.value {
                    ast::Number::Int(value) => number_literal_expr_with_meta(
                        NumberLiteralValue::Int(value.into()),
                        meta.node_index,
                        meta.range,
                    ),
                    ast::Number::Float(value) => number_literal_expr_with_meta(
                        NumberLiteralValue::Float(value),
                        meta.node_index,
                        meta.range,
                    ),
                    ast::Number::Complex { real, imag } => {
                        complex_literal_expr_with_meta(real, imag, meta.node_index, meta.range)
                    }
                }
            }
            InstrRuff::ExprBooleanLiteral(node) => {
                if node.value {
                    core_builtin_name("TRUE")
                } else {
                    core_builtin_name("FALSE")
                }
            }
            InstrRuff::ExprNoneLiteral(_) => core_builtin_name("NONE"),
            InstrRuff::ExprEllipsisLiteral(_) => core_builtin_name("ELLIPSIS"),
            InstrRuff::ExprAttribute(node) if matches!(node.ctx, ast::ExprContext::Load) => {
                if matches!(
                    node.value.as_ref(),
                    InstrRuff::ExprName(base) if base.id.as_str() == "__soac__"
                ) {
                    return core_runtime_name_expr_with_meta(
                        node.attr.id.as_str(),
                        node.meta().node_index,
                        node.meta().range,
                    );
                }
                let node_index = node.meta().node_index;
                let range = node.meta().range;
                let value = Self::from_ruff_expr(*node.value);
                getattr_expr_with_meta(node_index, range, value, node.attr.id.as_str().to_string())
            }
            InstrRuff::ExprSubscript(node) if matches!(node.ctx, ast::ExprContext::Load) => {
                let node_index = node.meta().node_index;
                let range = node.meta().range;
                let value = Self::from_ruff_expr(*node.value);
                let index = Self::from_ruff_expr(*node.slice);
                getitem_expr_with_meta(node_index, range, value, index)
            }
            InstrRuff::UnaryOp(node) => {
                let node_index = node.meta().node_index;
                let range = node.meta().range;
                let operand = Self::from_ruff_expr(*node.operand);
                unary_op_expr_with_meta(node_index, range, node.kind, operand)
            }
            InstrRuff::BinOp(node) => {
                let node_index = node.meta().node_index;
                let range = node.meta().range;
                let left = Self::from_ruff_expr(*node.left);
                let right = Self::from_ruff_expr(*node.right);
                binop_expr_with_meta(node_index, range, node.kind, left, right)
            }
            InstrRuff::ExprCompare(node) if node.ops.len() == 1 && node.comparators.len() == 1 => {
                let node_index = node.meta().node_index;
                let range = node.meta().range;
                let left = Self::from_ruff_expr(*node.left);
                let right = Self::from_ruff_expr(
                    node.comparators
                        .into_iter()
                        .next()
                        .expect("single compare comparator"),
                );
                let op = node.ops.into_iter().next().expect("single compare op");
                compare_expr_from_ast_with_meta(node_index, range, op, left, right)
            }
            InstrRuff::ExprDict(node) => reduce_core_blockpy_dict(node.items.into()),
            InstrRuff::ExprName(node) => {
                let meta = node.meta();
                InstrWithAwaitAndYield::Load(operation::Load::new(node.id).with_meta(meta))
            }
            InstrRuff::ExprIpyEscapeCommand(_) => {
                panic!("IpyEscapeCommand should not reach late core BlockPy boundary")
            }
            other => Self::from_ast_expr(crate::passes::ast_to_instr::into_ast_expr(other)),
        }
    }

    pub(crate) fn from_ast_expr(value: Expr) -> Self {
        let mut value = value;
        lower_string_templates_in_expr(&mut value);
        match value {
            Expr::Call(node) => lower_core_call_expr_with_meta(
                *node.func,
                node.node_index,
                node.range,
                node.arguments.args.into_vec(),
                node.arguments.keywords.into_vec(),
            ),
            Expr::Await(node) => Self::Await(
                Await::new(Self::from_ast_expr(*node.value))
                    .with_meta(Meta::new(node.node_index, node.range)),
            ),
            Expr::Yield(node) => Self::Yield(
                Yield::new(
                    node.value
                        .map(|value| Self::from_ast_expr(*value))
                        .unwrap_or_else(InstrWithAwaitAndYield::constant_none),
                )
                .with_meta(Meta::new(node.node_index, node.range)),
            ),
            Expr::YieldFrom(node) => Self::YieldFrom(
                YieldFrom::new(Self::from_ast_expr(*node.value))
                    .with_meta(Meta::new(node.node_index, node.range)),
            ),
            Expr::StringLiteral(node) => literal_expr(
                StringLiteral {
                    value: node.value.to_str().to_string(),
                },
                Meta::new(node.node_index, node.range),
            ),
            Expr::BytesLiteral(node) => literal_expr(
                BytesLiteral {
                    value: {
                        let value: std::borrow::Cow<[u8]> = (&node.value).into();
                        value.into_owned()
                    },
                },
                Meta::new(node.node_index, node.range),
            ),
            Expr::NumberLiteral(node) => match node.value {
                ast::Number::Int(value) => number_literal_expr_with_meta(
                    NumberLiteralValue::Int(value.into()),
                    node.node_index,
                    node.range,
                ),
                ast::Number::Float(value) => number_literal_expr_with_meta(
                    NumberLiteralValue::Float(value),
                    node.node_index,
                    node.range,
                ),
                ast::Number::Complex { real, imag } => {
                    complex_literal_expr_with_meta(real, imag, node.node_index, node.range)
                }
            },
            Expr::BooleanLiteral(node) => {
                if node.value {
                    core_builtin_name("TRUE")
                } else {
                    core_builtin_name("FALSE")
                }
            }
            Expr::NoneLiteral(_) => core_builtin_name("NONE"),
            Expr::EllipsisLiteral(_) => core_builtin_name("ELLIPSIS"),
            Expr::Attribute(node) if matches!(node.ctx, ast::ExprContext::Load) => {
                if matches!(
                    node.value.as_ref(),
                    Expr::Name(base) if base.id.as_str() == "__soac__"
                ) {
                    return core_runtime_name_expr_with_meta(
                        node.attr.id.as_str(),
                        node.node_index,
                        node.range,
                    );
                }
                let value = Self::from_ast_expr(*node.value);
                getattr_expr_with_meta(
                    node.node_index,
                    node.range,
                    value,
                    node.attr.id.as_str().to_string(),
                )
            }
            Expr::Subscript(node) if matches!(node.ctx, ast::ExprContext::Load) => {
                let value = Self::from_ast_expr(*node.value);
                let index = Self::from_ast_expr(*node.slice);
                getitem_expr_with_meta(node.node_index, node.range, value, index)
            }
            Expr::UnaryOp(node) => {
                let operand = Self::from_ast_expr(*node.operand);
                unary_op_expr_from_ast_with_meta(node.node_index, node.range, node.op, operand)
            }
            Expr::BinOp(node) => {
                let left = Self::from_ast_expr(*node.left);
                let right = Self::from_ast_expr(*node.right);
                binop_expr_from_ast_with_meta(node.node_index, node.range, node.op, left, right)
            }
            Expr::Compare(node) if node.ops.len() == 1 && node.comparators.len() == 1 => {
                let node_index = node.node_index;
                let range = node.range;
                let left = *node.left;
                let right = node
                    .comparators
                    .into_vec()
                    .into_iter()
                    .next()
                    .expect("single compare comparator");
                let op = node
                    .ops
                    .into_vec()
                    .into_iter()
                    .next()
                    .expect("single compare op");
                compare_expr_from_ast_with_meta(
                    node_index,
                    range,
                    op,
                    Self::from_ast_expr(left),
                    Self::from_ast_expr(right),
                )
            }
            Expr::Tuple(node) if matches!(node.ctx, ast::ExprContext::Load) => {
                if node.elts.iter().any(Expr::is_starred_expr) {
                    reduce_core_tuple_splat(node.elts)
                } else {
                    tuple_from_ast_exprs_with_meta(node.elts, node.node_index, node.range)
                }
            }
            Expr::List(node) if matches!(node.ctx, ast::ExprContext::Load) => {
                let tuple = if node.elts.iter().any(Expr::is_starred_expr) {
                    reduce_core_tuple_splat(node.elts)
                } else {
                    tuple_from_ast_exprs_with_meta(node.elts, node.node_index.clone(), node.range)
                };
                core_runtime_positional_call_expr_with_meta(
                    "list",
                    node.node_index,
                    node.range,
                    vec![tuple],
                )
            }
            Expr::Set(node) => {
                let tuple = if node.elts.iter().any(Expr::is_starred_expr) {
                    reduce_core_tuple_splat(node.elts)
                } else {
                    tuple_from_ast_exprs_with_meta(node.elts, node.node_index.clone(), node.range)
                };
                core_runtime_positional_call_expr_with_meta(
                    "set",
                    node.node_index,
                    node.range,
                    vec![tuple],
                )
            }
            Expr::Slice(node) => Self::from_ast_expr(py_expr!(
                "__soac__.slice({lower:expr}, {upper:expr}, {step:expr})",
                lower = node
                    .lower
                    .map(|expr| *expr)
                    .unwrap_or_else(|| py_expr!("None")),
                upper = node
                    .upper
                    .map(|expr| *expr)
                    .unwrap_or_else(|| py_expr!("None")),
                step = node
                    .step
                    .map(|expr| *expr)
                    .unwrap_or_else(|| py_expr!("None")),
            )),
            Expr::Dict(node) => reduce_core_blockpy_dict(node.items.into()),
            Expr::Name(node) => {
                let meta = node.meta();
                InstrWithAwaitAndYield::Load(operation::Load::new(node.id).with_meta(meta))
            }
            Expr::IpyEscapeCommand(_) => {
                panic!("IpyEscapeCommand should not reach late core BlockPy boundary")
            }
            other => panic!(
                "unexpected expr reached late core BlockPy boundary: {}",
                crate::ruff_ast_to_string(&other)
            ),
        }
    }
}

#[cfg(test)]
mod test;
