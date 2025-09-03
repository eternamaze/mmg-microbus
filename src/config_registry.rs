use crate::bus::KindId;

pub type CfgInvokeFn = for<'a> fn(
    comp: &'a mut dyn crate::component::Component,
    ctx: crate::component::ConfigContext,
    v: serde_json::Value,
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
