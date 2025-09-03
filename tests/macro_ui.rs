//! Compile-fail UI tests for #[handles]/#[handle]

#[test]
fn ui_handle_instance_without_from_fails() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/handle_instance_without_from.rs");
}

#[test]
fn ui_handle_instance_string_forbidden_fails() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/handle_instance_string_forbidden.rs");
}

#[test]
fn ui_handle_happy_min_ok() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/happy_min.rs");
}
