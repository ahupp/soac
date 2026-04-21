use super::{
    ChildVisitable, CounterId, HasMeta, Instr, MapInstr, Mappable, Meta, TryMapInstr, WithMeta,
};
use soac_core::block_py::define_instr;

define_instr! {
    pub struct IncrementCounter {
        counter_id: CounterId,
    }
}
