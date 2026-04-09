
I'd like to merge the CfgModule struct into BlockPyModule.  To simplify the typing, lets use a trait with associated types.

trait BlockPyPass {
    type Expr;
    type Stmt;
    type Term;
    type BlockMeta;
    type Block;
    type Function;
}

// What you've been calling "semantic"
struct RuffBlockPyPass;

impl BlockPyPass for RuffBlockPyPass {
    type Expr = ruff_python_ast::Expr;
    type Stmt = BlockPyStmt<Expr>;
    type Term = BlockPyTerm<Expr>;
    type BlockMeta = ();
    type Block = CfgBlock<...>;
    type Function = BlockPyFunction<Self::Expr, ...>
}

Then BlockPyModule becomes:

struct BlockPyModule<P: BlockPyPass> {
    callable_defs: Vec<BlockPyFunction<P>>
}


There are many places we do a transform manually over BlockPyModule.  Define a pair of traits:

trait BlockPyModuleVisitor<P: BlockPyPass> {
    fn visit_module(&self, module: &BlockPyModule<P>);
    fn visit_stmt(&self, module: &P::Stmt);
    fn visit_term(&self, module: &P::Term);
    fn visit_expr(&self, module: &P::Expr);
}

and

trait BlockPyModuleMap<PIn: BlockPyPass, POut: BlockPyPass> {
    fn map_module(&self, module: BlockPyModule<PIn>) -> BlockPyModule<POut>;
    fn map_fn(&self, func: PIn::Function) -> POut::Function;
    fn map_stmt(&self, stmt: PIn::Stmt) -> POut::Stmt;
    fn map_term(&self, term: PIn::Term) -> POut::Term;
    fn map_expr(&self, expr: PIn::Expr) -> POut::Expr;
}

Then implement a generic "map_module" function on BlockPyModule that takes a BlockPyModuleMap and returns the new type.  map_module should consume `self` (by value).


Implement Into / From traits to go "up" the pass heirarchy, converting e.g a CoreBlockPyExprWithoutAwait into a CoreBlockPyExpr.  This should be infallible.
Then implement rendering functions for the inspector on CoreBlockPyExpr.