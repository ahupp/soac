use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_python_codegen::{Generator, Indentation};
use ruff_source_file::LineEnding;
use ruff_text_size::TextRange;

pub(crate) trait ToRuffAst {
    fn to_ruff_ast(&self) -> Vec<Stmt>;
}

impl ToRuffAst for Expr {
    fn to_ruff_ast(&self) -> Vec<Stmt> {
        vec![Stmt::Expr(ast::StmtExpr {
            value: Box::new(self.clone()),
            range: TextRange::default(),
            node_index: ast::AtomicNodeIndex::default(),
        })]
    }
}

impl ToRuffAst for Stmt {
    fn to_ruff_ast(&self) -> Vec<Stmt> {
        vec![self.clone()]
    }
}

impl ToRuffAst for &Stmt {
    fn to_ruff_ast(&self) -> Vec<Stmt> {
        vec![self.to_owned().clone()]
    }
}

impl ToRuffAst for &Vec<Stmt> {
    fn to_ruff_ast(&self) -> Vec<Stmt> {
        self.to_vec()
    }
}

impl ToRuffAst for &Expr {
    fn to_ruff_ast(&self) -> Vec<Stmt> {
        let expr = self.to_owned().clone();
        vec![Stmt::Expr(ast::StmtExpr {
            value: Box::new(expr),
            range: TextRange::default(),
            node_index: ast::AtomicNodeIndex::default(),
        })]
    }
}

impl ToRuffAst for &[Stmt] {
    fn to_ruff_ast(&self) -> Vec<Stmt> {
        self.to_vec()
    }
}

/// Convert a ruff AST ModModule to a pretty-printed string.
pub(crate) fn ruff_ast_to_string(module: impl ToRuffAst) -> String {
    let module = module.to_ruff_ast();
    // Use default stylist settings for pretty printing.
    let indent = Indentation::new("    ".to_string());
    let mut output = String::new();
    for stmt in module {
        let gen = Generator::new(&indent, LineEnding::default());
        output.push_str(&gen.stmt(&stmt));
        output.push_str(LineEnding::default().as_str());
    }
    output
}
