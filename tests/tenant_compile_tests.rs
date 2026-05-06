#![cfg(feature = "tenant")]

#[test]
fn tenant_compile_guards() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/tenant_no_key.rs");
    t.compile_fail("tests/ui/tenant_duplicate_key.rs");
}
