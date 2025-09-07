use crate::bus::KindId;

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
