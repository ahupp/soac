use super::{
    ChildVisitable, CounterId, HasMeta, Instr, MapInstr, Mappable, Meta, TryMapInstr, WithMeta,
};
use soac_core::block_py::operation_macro::define_operation;

define_operation! {
    pub struct IncrementCounter {
        counter_id: CounterId,
    }
}
