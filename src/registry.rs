use crate::component::DynFactory;

// Use a local newtype to satisfy orphan rules for inventory collection.
// Store a function pointer that constructs an Arc<dyn ComponentFactory>.
pub struct FactoryEntry(pub fn() -> DynFactory);
inventory::collect!(FactoryEntry);

// Manual macros removed: prefer #[component] / #[component_factory] proc-macros for inventory registration.
