//! UI tests入口（当前仅最小 happy 场景）

#[test]
fn ui_handle_happy_min_ok() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/happy_min.rs");
}
