use crate::{bus::KindId, component::DynFactory};

/// Registration info for a component type collected via inventory.
/// - `instances`: optional declared instance names; empty => implicit singleton.
pub struct Registration {
    pub kind: fn() -> KindId,
    pub type_name: fn() -> &'static str,
    pub instances: fn() -> &'static [&'static str],
    pub new_factory: fn() -> DynFactory,
}

inventory::collect!(Registration);

/// Get the number of declared instances for a given kind; empty means 1 (implicit singleton).
pub fn instances_count(kind: KindId) -> usize {
    for reg in inventory::iter::<Registration> {
        if (reg.kind)() == kind {
            let list = (reg.instances)();
            return if list.is_empty() { 1 } else { list.len() };
        }
    }
    0
}

/// Iterate all registrations
pub fn all() -> Vec<&'static Registration> {
    inventory::iter::<Registration>.into_iter().collect()
}

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
