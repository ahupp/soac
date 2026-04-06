use super::*;

pub trait Mappable<E>: Sized
where
    E: Instr,
{
    type Mapped<T: Instr>;

    fn map_children<T, M>(self, map: &mut M) -> Self::Mapped<T>
    where
        T: Instr,
        M: MapInstr<E, T>;

    fn try_map_children<T, Error, M>(self, map: &mut M) -> Result<Self::Mapped<T>, Error>
    where
        T: Instr,
        M: TryMapInstr<E, T, Error>;

    fn map_same_children<M>(self, map: &mut M) -> Self::Mapped<E>
    where
        M: MapInstr<E, E>,
    {
        self.map_children(map)
    }

    fn try_map_same_children<Error, M>(self, map: &mut M) -> Result<Self::Mapped<E>, Error>
    where
        M: TryMapInstr<E, E, Error>,
    {
        self.try_map_children(map)
    }
}

pub trait MapInstr<In: Instr, Out: Instr> {
    fn map_instr(&mut self, instr: In) -> Out;
    fn map_name(&mut self, name: In::Name) -> Out::Name;
}

pub trait TryMapInstr<In: Instr, Out: Instr, Error> {
    fn try_map_instr(&mut self, instr: In) -> Result<Out, Error>;
    fn try_map_name(&mut self, name: In::Name) -> Result<Out::Name, Error>;
}

impl<I, F> MapInstr<I, I> for F
where
    I: Instr,
    F: FnMut(I) -> I,
{
    fn map_instr(&mut self, instr: I) -> I {
        self(instr)
    }

    fn map_name(&mut self, name: I::Name) -> I::Name {
        name
    }
}

impl<I, Error, F> TryMapInstr<I, I, Error> for F
where
    I: Instr,
    F: FnMut(I) -> Result<I, Error>,
{
    fn try_map_instr(&mut self, instr: I) -> Result<I, Error> {
        self(instr)
    }

    fn try_map_name(&mut self, name: I::Name) -> Result<I::Name, Error> {
        Ok(name)
    }
}

pub(crate) trait MapTerm<In, Out>: MapInstr<In, Out>
where
    In: Instr,
    Out: Instr,
{
    fn map_term(&mut self, term: BlockTerm<In>) -> BlockTerm<Out> {
        match term {
            BlockTerm::Jump(edge) => BlockTerm::Jump(BlockEdge {
                target: edge.target,
                args: edge.args,
            }),
            BlockTerm::IfTerm(if_term) => BlockTerm::IfTerm(TermIf {
                test: self.map_instr(if_term.test),
                then_label: if_term.then_label,
                else_label: if_term.else_label,
            }),
            BlockTerm::BranchTable(branch) => BlockTerm::BranchTable(TermBranchTable {
                index: self.map_instr(branch.index),
                targets: branch.targets,
                default_label: branch.default_label,
            }),
            BlockTerm::Raise(raise_stmt) => BlockTerm::Raise(TermRaise {
                exc: raise_stmt.exc.map(|exc| self.map_instr(exc)),
            }),
            BlockTerm::Return(value) => BlockTerm::Return(self.map_instr(value)),
        }
    }
}

impl<In, Out, M> MapTerm<In, Out> for M
where
    In: Instr,
    Out: Instr,
    M: MapInstr<In, Out>,
{
}

pub(crate) trait MapBlock<In, Out>: MapTerm<In, Out>
where
    In: Instr,
    Out: Instr,
{
    fn map_block(&mut self, block: Block<In>) -> Block<Out> {
        Block {
            label: block.label,
            body: block
                .body
                .into_iter()
                .map(|stmt| self.map_instr(stmt))
                .collect(),
            term: self.map_term(block.term),
            params: block.params,
            exc_edge: block.exc_edge,
        }
    }
}

impl<In, Out, M> MapBlock<In, Out> for M
where
    In: Instr,
    Out: Instr,
    M: MapTerm<In, Out>,
{
}

pub(crate) trait MapFunction<PIn, POut>: MapBlock<PIn::Instr, POut::Instr>
where
    PIn: ModuleShape,
    POut: ModuleShape,
{
    fn map_fn(&mut self, func: BlockPyFunction<PIn>) -> BlockPyFunction<POut> {
        BlockPyFunction {
            function_id: func.function_id,
            name_gen: func.name_gen,
            names: func.names,
            kind: func.kind,
            params: func.params,
            blocks: func
                .blocks
                .into_iter()
                .map(|block| self.map_block(block))
                .collect(),
            doc: func.doc,
            storage_layout: func.storage_layout,
            scope: func.scope,
        }
    }
}

impl<PIn, POut, M> MapFunction<PIn, POut> for M
where
    PIn: ModuleShape,
    POut: ModuleShape,
    M: MapBlock<PIn::Instr, POut::Instr>,
{
}

pub(crate) trait MapModule<PIn, POut>: MapFunction<PIn, POut>
where
    PIn: ModuleShape,
    POut: ModuleShape,
{
    fn map_module(&mut self, module: BlockPyModule<PIn>) -> BlockPyModule<POut> {
        BlockPyModule {
            module_name_gen: module.module_name_gen,
            global_names: module.global_names,
            callable_defs: module
                .callable_defs
                .into_iter()
                .map(|function| self.map_fn(function))
                .collect(),
            module_constants: module.module_constants,
            counter_defs: module.counter_defs,
        }
    }
}

impl<PIn, POut, M> MapModule<PIn, POut> for M
where
    PIn: ModuleShape,
    POut: ModuleShape,
    M: MapFunction<PIn, POut>,
{
}

pub(crate) fn map_function_blocks<PIn, POut>(
    func: BlockPyFunction<PIn>,
    mut map_block: impl FnMut(Block<PIn::Instr>) -> Block<POut::Instr>,
) -> BlockPyFunction<POut>
where
    PIn: ModuleShape,
    POut: ModuleShape,
{
    BlockPyFunction {
        function_id: func.function_id,
        name_gen: func.name_gen,
        names: func.names,
        kind: func.kind,
        params: func.params,
        blocks: func.blocks.into_iter().map(&mut map_block).collect(),
        doc: func.doc,
        storage_layout: func.storage_layout,
        scope: func.scope,
    }
}

pub(crate) fn map_module_functions<PIn, POut>(
    module: BlockPyModule<PIn>,
    mut map_fn: impl FnMut(BlockPyFunction<PIn>) -> BlockPyFunction<POut>,
) -> BlockPyModule<POut>
where
    PIn: ModuleShape,
    POut: ModuleShape,
{
    BlockPyModule {
        module_name_gen: module.module_name_gen,
        global_names: module.global_names,
        callable_defs: module.callable_defs.into_iter().map(&mut map_fn).collect(),
        module_constants: module.module_constants,
        counter_defs: module.counter_defs,
    }
}

pub(crate) trait TryMapTerm<In, Out, Error>: TryMapInstr<In, Out, Error>
where
    In: Instr,
    Out: Instr,
{
    fn try_map_term(&mut self, term: BlockTerm<In>) -> Result<BlockTerm<Out>, Error> {
        match term {
            BlockTerm::Jump(edge) => Ok(BlockTerm::Jump(BlockEdge {
                target: edge.target,
                args: edge.args,
            })),
            BlockTerm::IfTerm(if_term) => Ok(BlockTerm::IfTerm(TermIf {
                test: self.try_map_instr(if_term.test)?,
                then_label: if_term.then_label,
                else_label: if_term.else_label,
            })),
            BlockTerm::BranchTable(branch) => Ok(BlockTerm::BranchTable(TermBranchTable {
                index: self.try_map_instr(branch.index)?,
                targets: branch.targets,
                default_label: branch.default_label,
            })),
            BlockTerm::Raise(raise_stmt) => Ok(BlockTerm::Raise(TermRaise {
                exc: raise_stmt
                    .exc
                    .map(|exc| self.try_map_instr(exc))
                    .transpose()?,
            })),
            BlockTerm::Return(value) => Ok(BlockTerm::Return(self.try_map_instr(value)?)),
        }
    }
}

impl<In, Out, Error, M> TryMapTerm<In, Out, Error> for M
where
    In: Instr,
    Out: Instr,
    M: TryMapInstr<In, Out, Error>,
{
}

pub(crate) trait TryMapBlock<In, Out, Error>: TryMapTerm<In, Out, Error>
where
    In: Instr,
    Out: Instr,
{
    fn try_map_block(&mut self, block: Block<In>) -> Result<Block<Out>, Error> {
        Ok(Block {
            label: block.label,
            body: block
                .body
                .into_iter()
                .map(|stmt| self.try_map_instr(stmt))
                .collect::<Result<_, _>>()?,
            term: self.try_map_term(block.term)?,
            params: block.params,
            exc_edge: block.exc_edge,
        })
    }
}

impl<In, Out, Error, M> TryMapBlock<In, Out, Error> for M
where
    In: Instr,
    Out: Instr,
    M: TryMapTerm<In, Out, Error>,
{
}

pub(crate) trait TryMapFunction<PIn, POut, Error>:
    TryMapBlock<PIn::Instr, POut::Instr, Error>
where
    PIn: ModuleShape,
    POut: ModuleShape,
{
    fn try_map_fn(&mut self, func: BlockPyFunction<PIn>) -> Result<BlockPyFunction<POut>, Error> {
        Ok(BlockPyFunction {
            function_id: func.function_id,
            name_gen: func.name_gen,
            names: func.names,
            kind: func.kind,
            params: func.params,
            blocks: func
                .blocks
                .into_iter()
                .map(|block| self.try_map_block(block))
                .collect::<Result<_, _>>()?,
            doc: func.doc,
            storage_layout: func.storage_layout,
            scope: func.scope,
        })
    }
}

impl<PIn, POut, Error, M> TryMapFunction<PIn, POut, Error> for M
where
    PIn: ModuleShape,
    POut: ModuleShape,
    M: TryMapBlock<PIn::Instr, POut::Instr, Error>,
{
}

#[allow(dead_code)]
pub(crate) trait TryMapModule<PIn, POut, Error>: TryMapFunction<PIn, POut, Error>
where
    PIn: ModuleShape,
    POut: ModuleShape,
{
    fn try_map_module(&mut self, module: BlockPyModule<PIn>) -> Result<BlockPyModule<POut>, Error> {
        Ok(BlockPyModule {
            module_name_gen: module.module_name_gen,
            global_names: module.global_names,
            callable_defs: module
                .callable_defs
                .into_iter()
                .map(|function| self.try_map_fn(function))
                .collect::<Result<_, _>>()?,
            module_constants: module.module_constants,
            counter_defs: module.counter_defs,
        })
    }
}

impl<PIn, POut, Error, M> TryMapModule<PIn, POut, Error> for M
where
    PIn: ModuleShape,
    POut: ModuleShape,
    M: TryMapFunction<PIn, POut, Error>,
{
}
