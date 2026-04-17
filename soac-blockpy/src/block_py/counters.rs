use super::operation_macro::define_operation;
use super::{
    ChildVisitable, CounterId, HasMeta, Instr, MapInstr, Mappable, Meta, TryMapInstr, WithMeta,
};

define_operation! {
    pub struct IncrementCounter {
        counter_id: CounterId,
    }
}
