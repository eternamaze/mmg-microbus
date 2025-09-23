# microbus-macros

Procedural macros for `mmg-microbus`.

Provided attributes:
- `#[component]` — register component factory for `struct`, or generate `Component::run` for `impl`.
- `#[handle]` — message handler, signature like `(&ComponentContext? , &T)` with six supported return cases.
- `#[active]` — active loop/once methods, supports `#[active(once)]`.
- `#[init]` — called before main loop once.
- `#[stop]` — called before shutdown once.

This crate contains only the macro entry points; all logic lives in `src/gen.rs` to keep interface/implementation separated.
