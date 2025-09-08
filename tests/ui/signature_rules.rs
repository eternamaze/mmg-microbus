use mmg_microbus::prelude::*;

#[test]
fn ui_signature_rules() {
  let t = trybuild::TestCases::new();
  t.pass("tests/ui/happy_min.rs");
}
