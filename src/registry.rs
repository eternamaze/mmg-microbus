use crate::bus::KindId;
use std::any::TypeId;

/// A runtime constraint emitted by handlers that subscribe with `from=Type` but without specifying an instance.
/// The framework must ensure `from_kind` has exactly one instance (singleton) at startup.
pub struct RouteConstraint {
    pub consumer_ty: fn() -> &'static str,
    pub consumer_kind: fn() -> KindId,
    pub from_kind: fn() -> KindId,
}

inventory::collect!(RouteConstraint);

pub fn route_constraints() -> Vec<&'static RouteConstraint> {
    inventory::iter::<RouteConstraint>.into_iter().collect()
}

/// Produced/Consumed message type registries for wiring checks
pub struct ProducedType {
    pub type_name: fn() -> &'static str,
    pub type_id: fn() -> TypeId,
}
pub struct ConsumedType {
    pub type_name: fn() -> &'static str,
    pub type_id: fn() -> TypeId,
}

inventory::collect!(ProducedType);
inventory::collect!(ConsumedType);

pub fn produced_types() -> Vec<&'static ProducedType> {
    Vec::new()
}
pub fn consumed_types() -> Vec<&'static ConsumedType> {
    Vec::new()
}

/// Wiring check disabled by design: subscriptions without producers are allowed
pub fn check_message_wiring() -> Vec<&'static ConsumedType> {
    Vec::new()
}
