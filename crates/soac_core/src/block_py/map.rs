use super::visit::{Visit, VisitMut};
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

pub trait InstrField<In>: Sized
where
    In: Instr,
{
    type Mapped<Out: Instr>;

    fn visit_field<V>(&self, visitor: &mut V)
    where
        In: ChildVisitable<In>,
        V: Visit<In> + ?Sized;

    fn visit_field_mut<V>(&mut self, visitor: &mut V)
    where
        In: ChildVisitable<In>,
        V: VisitMut<In> + ?Sized;

    fn map_field<Out, M>(self, map: &mut M) -> Self::Mapped<Out>
    where
        Out: Instr,
        M: MapInstr<In, Out>;

    fn try_map_field<Out, Error, M>(self, map: &mut M) -> Result<Self::Mapped<Out>, Error>
    where
        Out: Instr,
        M: TryMapInstr<In, Out, Error>;
}

impl<In> InstrField<In> for Box<In>
where
    In: Instr,
{
    type Mapped<Out: Instr> = Box<Out>;

    fn visit_field<V>(&self, visitor: &mut V)
    where
        In: ChildVisitable<In>,
        V: Visit<In> + ?Sized,
    {
        visitor.visit_instr(self);
    }

    fn visit_field_mut<V>(&mut self, visitor: &mut V)
    where
        In: ChildVisitable<In>,
        V: VisitMut<In> + ?Sized,
    {
        visitor.visit_instr_mut(self);
    }

    fn map_field<Out, M>(self, map: &mut M) -> Self::Mapped<Out>
    where
        Out: Instr,
        M: MapInstr<In, Out>,
    {
        Box::new(map.map_instr(*self))
    }

    fn try_map_field<Out, Error, M>(self, map: &mut M) -> Result<Self::Mapped<Out>, Error>
    where
        Out: Instr,
        M: TryMapInstr<In, Out, Error>,
    {
        map.try_map_instr(*self).map(Box::new)
    }
}

impl<In> InstrField<In> for In
where
    In: Instr,
{
    type Mapped<Out: Instr> = Out;

    fn visit_field<V>(&self, visitor: &mut V)
    where
        In: ChildVisitable<In>,
        V: Visit<In> + ?Sized,
    {
        visitor.visit_instr(self);
    }

    fn visit_field_mut<V>(&mut self, visitor: &mut V)
    where
        In: ChildVisitable<In>,
        V: VisitMut<In> + ?Sized,
    {
        visitor.visit_instr_mut(self);
    }

    fn map_field<Out, M>(self, map: &mut M) -> Self::Mapped<Out>
    where
        Out: Instr,
        M: MapInstr<In, Out>,
    {
        map.map_instr(self)
    }

    fn try_map_field<Out, Error, M>(self, map: &mut M) -> Result<Self::Mapped<Out>, Error>
    where
        Out: Instr,
        M: TryMapInstr<In, Out, Error>,
    {
        map.try_map_instr(self)
    }
}

impl<In> InstrField<In> for Option<Box<In>>
where
    In: Instr,
{
    type Mapped<Out: Instr> = Option<Box<Out>>;

    fn visit_field<V>(&self, visitor: &mut V)
    where
        In: ChildVisitable<In>,
        V: Visit<In> + ?Sized,
    {
        if let Some(item) = self {
            visitor.visit_instr(item);
        }
    }

    fn visit_field_mut<V>(&mut self, visitor: &mut V)
    where
        In: ChildVisitable<In>,
        V: VisitMut<In> + ?Sized,
    {
        if let Some(item) = self {
            visitor.visit_instr_mut(item);
        }
    }

    fn map_field<Out, M>(self, map: &mut M) -> Self::Mapped<Out>
    where
        Out: Instr,
        M: MapInstr<In, Out>,
    {
        self.map(|value| Box::new(map.map_instr(*value)))
    }

    fn try_map_field<Out, Error, M>(self, map: &mut M) -> Result<Self::Mapped<Out>, Error>
    where
        Out: Instr,
        M: TryMapInstr<In, Out, Error>,
    {
        self.map(|value| map.try_map_instr(*value).map(Box::new))
            .transpose()
    }
}

impl<In> InstrField<In> for CallArgPositional<In>
where
    In: Instr,
{
    type Mapped<Out: Instr> = CallArgPositional<Out>;

    fn visit_field<V>(&self, visitor: &mut V)
    where
        In: ChildVisitable<In>,
        V: Visit<In> + ?Sized,
    {
        visitor.visit_instr(self.expr());
    }

    fn visit_field_mut<V>(&mut self, visitor: &mut V)
    where
        In: ChildVisitable<In>,
        V: VisitMut<In> + ?Sized,
    {
        visitor.visit_instr_mut(self.expr_mut());
    }

    fn map_field<Out, M>(self, map: &mut M) -> Self::Mapped<Out>
    where
        Out: Instr,
        M: MapInstr<In, Out>,
    {
        self.map_instr(|expr| map.map_instr(expr))
    }

    fn try_map_field<Out, Error, M>(self, map: &mut M) -> Result<Self::Mapped<Out>, Error>
    where
        Out: Instr,
        M: TryMapInstr<In, Out, Error>,
    {
        self.try_map_instr(|expr| map.try_map_instr(expr))
    }
}

impl<In, Field> InstrField<In> for Vec<Field>
where
    In: Instr,
    Field: InstrField<In>,
{
    type Mapped<Out: Instr> = Vec<Field::Mapped<Out>>;

    fn visit_field<V>(&self, visitor: &mut V)
    where
        In: ChildVisitable<In>,
        V: Visit<In> + ?Sized,
    {
        for item in self {
            item.visit_field(visitor);
        }
    }

    fn visit_field_mut<V>(&mut self, visitor: &mut V)
    where
        In: ChildVisitable<In>,
        V: VisitMut<In> + ?Sized,
    {
        for item in self {
            item.visit_field_mut(visitor);
        }
    }

    fn map_field<Out, M>(self, map: &mut M) -> Self::Mapped<Out>
    where
        Out: Instr,
        M: MapInstr<In, Out>,
    {
        self.into_iter().map(|item| item.map_field(map)).collect()
    }

    fn try_map_field<Out, Error, M>(self, map: &mut M) -> Result<Self::Mapped<Out>, Error>
    where
        Out: Instr,
        M: TryMapInstr<In, Out, Error>,
    {
        self.into_iter()
            .map(|item| item.try_map_field(map))
            .collect()
    }
}

impl<In> InstrField<In> for CallArgKeyword<In>
where
    In: Instr,
{
    type Mapped<Out: Instr> = CallArgKeyword<Out>;

    fn visit_field<V>(&self, visitor: &mut V)
    where
        In: ChildVisitable<In>,
        V: Visit<In> + ?Sized,
    {
        visitor.visit_instr(self.expr());
    }

    fn visit_field_mut<V>(&mut self, visitor: &mut V)
    where
        In: ChildVisitable<In>,
        V: VisitMut<In> + ?Sized,
    {
        visitor.visit_instr_mut(self.expr_mut());
    }

    fn map_field<Out, M>(self, map: &mut M) -> Self::Mapped<Out>
    where
        Out: Instr,
        M: MapInstr<In, Out>,
    {
        self.map_instr(|expr| map.map_instr(expr))
    }

    fn try_map_field<Out, Error, M>(self, map: &mut M) -> Result<Self::Mapped<Out>, Error>
    where
        Out: Instr,
        M: TryMapInstr<In, Out, Error>,
    {
        self.try_map_instr(|expr| map.try_map_instr(expr))
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

pub trait MapTerm<In, Out>: MapInstr<In, Out>
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

pub trait MapBlock<In, Out, InExtra = (), OutExtra = ()>: MapTerm<In, Out>
where
    In: Instr,
    Out: Instr,
    OutExtra: Default,
{
    fn map_block(&mut self, block: Block<In, InExtra>) -> Block<Out, OutExtra> {
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
            extra: OutExtra::default(),
        }
    }
}

impl<In, Out, InExtra, OutExtra, M> MapBlock<In, Out, InExtra, OutExtra> for M
where
    In: Instr,
    Out: Instr,
    OutExtra: Default,
    M: MapTerm<In, Out>,
{
}

pub trait MapFunction<PIn, POut>:
    MapBlock<PIn::Instr, POut::Instr, PIn::BlockExtra, POut::BlockExtra>
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
            execution_mode: func.execution_mode,
            params: func.params,
            body_params: func.body_params,
            public_scope: func.public_scope,
            blocks: func
                .blocks
                .into_iter()
                .map(|block| self.map_block(block))
                .collect(),
            doc: func.doc,
            public_storage_layout: func.public_storage_layout,
            storage_layout: func.storage_layout,
            scope: func.scope,
        }
    }
}

impl<PIn, POut, M> MapFunction<PIn, POut> for M
where
    PIn: ModuleShape,
    POut: ModuleShape,
    M: MapBlock<PIn::Instr, POut::Instr, PIn::BlockExtra, POut::BlockExtra>,
{
}

pub trait MapModule<PIn, POut>: MapFunction<PIn, POut>
where
    PIn: ModuleShape,
    POut: ModuleShape<ModuleConstant = PIn::ModuleConstant>,
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
    POut: ModuleShape<ModuleConstant = PIn::ModuleConstant>,
    M: MapFunction<PIn, POut>,
{
}

pub fn map_function_blocks<PIn, POut>(
    func: BlockPyFunction<PIn>,
    mut map_block: impl FnMut(
        Block<PIn::Instr, PIn::BlockExtra>,
    ) -> Block<POut::Instr, POut::BlockExtra>,
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
        execution_mode: func.execution_mode,
        params: func.params,
        body_params: func.body_params,
        public_scope: func.public_scope,
        blocks: func.blocks.into_iter().map(&mut map_block).collect(),
        doc: func.doc,
        public_storage_layout: func.public_storage_layout,
        storage_layout: func.storage_layout,
        scope: func.scope,
    }
}

pub fn map_module_functions<PIn, POut>(
    module: BlockPyModule<PIn>,
    mut map_fn: impl FnMut(BlockPyFunction<PIn>) -> BlockPyFunction<POut>,
) -> BlockPyModule<POut>
where
    PIn: ModuleShape,
    POut: ModuleShape<ModuleConstant = PIn::ModuleConstant>,
{
    BlockPyModule {
        module_name_gen: module.module_name_gen,
        global_names: module.global_names,
        callable_defs: module.callable_defs.into_iter().map(&mut map_fn).collect(),
        module_constants: module.module_constants,
        counter_defs: module.counter_defs,
    }
}

pub trait TryMapTerm<In, Out, Error>: TryMapInstr<In, Out, Error>
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

pub trait TryMapBlock<In, Out, Error, InExtra = (), OutExtra = ()>:
    TryMapTerm<In, Out, Error>
where
    In: Instr,
    Out: Instr,
    OutExtra: Default,
{
    fn try_map_block(&mut self, block: Block<In, InExtra>) -> Result<Block<Out, OutExtra>, Error> {
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
            extra: OutExtra::default(),
        })
    }
}

impl<In, Out, Error, InExtra, OutExtra, M> TryMapBlock<In, Out, Error, InExtra, OutExtra> for M
where
    In: Instr,
    Out: Instr,
    OutExtra: Default,
    M: TryMapTerm<In, Out, Error>,
{
}

pub trait TryMapFunction<PIn, POut, Error>:
    TryMapBlock<PIn::Instr, POut::Instr, Error, PIn::BlockExtra, POut::BlockExtra>
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
            execution_mode: func.execution_mode,
            params: func.params,
            body_params: func.body_params,
            public_scope: func.public_scope,
            blocks: func
                .blocks
                .into_iter()
                .map(|block| self.try_map_block(block))
                .collect::<Result<_, _>>()?,
            doc: func.doc,
            public_storage_layout: func.public_storage_layout,
            storage_layout: func.storage_layout,
            scope: func.scope,
        })
    }
}

impl<PIn, POut, Error, M> TryMapFunction<PIn, POut, Error> for M
where
    PIn: ModuleShape,
    POut: ModuleShape,
    M: TryMapBlock<PIn::Instr, POut::Instr, Error, PIn::BlockExtra, POut::BlockExtra>,
{
}

#[allow(dead_code)]
pub trait TryMapModule<PIn, POut, Error>: TryMapFunction<PIn, POut, Error>
where
    PIn: ModuleShape,
    POut: ModuleShape<ModuleConstant = PIn::ModuleConstant>,
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
    POut: ModuleShape<ModuleConstant = PIn::ModuleConstant>,
    M: TryMapFunction<PIn, POut, Error>,
{
}
