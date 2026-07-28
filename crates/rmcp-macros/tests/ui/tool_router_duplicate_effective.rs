#[allow(unused_imports)]
use rmcp_macros::{tool, tool_router};

struct DuplicateEffectiveNames;

#[tool_router]
impl DuplicateEffectiveNames {
    #[tool(name = "shared")]
    fn renamed(&self) {}

    #[tool]
    fn shared(&self) {}
}

fn main() {}
