//! Compile-fail UI tests for #[handle]

// removed: previous compile-fail cases for instance filtering are obsolete after syntax change

#[test]
fn ui_handle_happy_min_ok() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/happy_min.rs");
}
