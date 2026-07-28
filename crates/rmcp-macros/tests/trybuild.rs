#[test]
fn tool_router_rejects_duplicate_effective_names() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/tool_router_duplicate_*.rs");
    cases.pass("tests/ui/tool_router_unique_names.rs");
}
