use crate::bus::KindId;
use std::any::Any;
use std::sync::Arc;

pub type CfgInvokeFn = for<'a> fn(
    comp: &'a mut dyn crate::component::Component,
    ctx: crate::component::ConfigContext,
    v: Arc<dyn Any + Send + Sync>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>,
>;

pub struct DesiredCfgSpec {
    pub component_kind: fn() -> KindId,
    pub cfg_type: fn() -> std::any::TypeId,
    pub invoke: CfgInvokeFn,
}
pub struct DesiredCfgEntry(pub DesiredCfgSpec);
inventory::collect!(DesiredCfgEntry);
