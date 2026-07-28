use rmcp_macros::{tool, tool_router};

struct DuplicateEffectiveNames;

#[tool_router]
impl DuplicateEffectiveNames {
    #[tool(name = "shared")]
    fn explicit_name(&self) {}

    #[tool]
    fn shared(&self) {}
}

fn main() {}
