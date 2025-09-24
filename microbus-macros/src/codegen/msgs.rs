// Centralized compile-time diagnostic & error string constants for the macro codegen layer.
// Behavior-neutral refactor: only moves literal strings into named constants.

pub(super) const ERR_HANDLE_NO_ARGS: &str = "#[handle] does not accept any arguments in this model";
pub(super) const ERR_HANDLE_MULTI_ATTR: &str =
    "a method can only have one #[handle(...)] attribute";
pub(super) const ERR_HANDLE_CTX_DUP: &str =
    "#[handle] allows at most one &ComponentContext parameter";
pub(super) const ERR_HANDLE_NEED_ONE_T: &str =
    "#[handle] requires exactly one &T parameter (message payload)";
pub(super) const ERR_HANDLE_ONLY_ONE_T: &str =
    "#[handle] allows only one &T parameter; remove extras";

pub(super) const ERR_HANDLE_MUT_SELF: &str =
    "#[handle] method cannot take &mut self under spawned worker model; use interior mutability";

pub(super) const ERR_ACTIVE_MUT_SELF: &str =
    "#[active] method cannot take &mut self; use interior mutability if needed";
pub(super) const ERR_ACTIVE_CTX_DUP: &str =
    "#[active] allows at most one &ComponentContext parameter";
pub(super) const ERR_ACTIVE_ONLY_CTX: &str = "#[active] method can only take &ComponentContext as parameter; other &T parameters are not allowed";
pub(super) const ERR_ACTIVE_LIST_ONCE_ONLY: &str = "#[active] only supports (once)";
pub(super) const ERR_ACTIVE_NO_NV: &str = "#[active] does not take name-value arguments";

pub(super) const ERR_INIT_SIG: &str = "#[init] only allows optional &ComponentContext";

pub(super) const ERR_STOP_MUT_SELF: &str = "#[stop] cannot take &mut self; use interior mutability";
pub(super) const ERR_STOP_CTX_DUP: &str = "#[stop] allows at most one &ComponentContext parameter";
pub(super) const ERR_STOP_SIG: &str =
    "#[stop] method must take only self or optionally &self plus &ComponentContext";

pub(super) const ERR_COMPONENT_TARGET: &str = "#[component] only supports struct or impl blocks";
