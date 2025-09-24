mod analyze;
mod emit_actives;
mod emit_handles;
mod emit_ret;
mod emit_run;
mod msgs;
mod parse;

use proc_macro::TokenStream;
use syn::{parse_macro_input, Item};

use analyze::{collect_actives, collect_handles, collect_inits, collect_stops};
use emit_actives::build_active_parts;
use emit_handles::build_handle_parts;
use emit_run::{build_init_stop_calls, component_for_struct, gen_component_run, RunParts};
use msgs::ERR_COMPONENT_TARGET;

pub fn entrypoint(args: TokenStream, input: TokenStream) -> TokenStream {
    let args_ts = proc_macro2::TokenStream::from(args);
    let item_any = parse_macro_input!(input as Item);
    match item_any {
        Item::Struct(item) => component_for_struct(&item, args_ts).into(),
        Item::Impl(item) => {
            let self_ty = item.self_ty.clone();
            let (methods, mut errs_h) = collect_handles(&item);
            let (actives, mut errs_a) = collect_actives(&item);
            let (inits, mut errs_i) = collect_inits(&item);
            let (stops, mut errs_s) = collect_stops(&item);
            let mut compile_errors = Vec::new();
            compile_errors.append(&mut errs_h);
            compile_errors.append(&mut errs_a);
            compile_errors.append(&mut errs_i);
            compile_errors.append(&mut errs_s);
            let (init_calls, stop_calls) = build_init_stop_calls(&inits, &stops);
            let (sub_decls, handle_spawns) = build_handle_parts(&methods);
            let (active_spawns, once_calls) = build_active_parts(&actives);
            let parts = RunParts {
                init_calls,
                stop_calls,
                sub_decls,
                handle_spawns,
                active_spawns,
                once_calls,
                compile_errors,
            };
            gen_component_run(&self_ty, &parts, &item).into()
        }
        other => syn::Error::new_spanned(other, ERR_COMPONENT_TARGET)
            .to_compile_error()
            .into(),
    }
}
// end of layered codegen module
