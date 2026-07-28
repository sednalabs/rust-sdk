#[allow(unused_imports)]
use rmcp_macros::{tool, tool_router};

struct DuplicateExplicitNames;

#[tool_router]
impl DuplicateExplicitNames {
    #[tool(name = "shared")]
    fn first(&self) {}

    #[tool(name = "shared")]
    fn second(&self) {}
}

fn main() {}
