use rmcp_macros::{tool, tool_router};

struct UniqueNames;

#[tool_router]
impl UniqueNames {
    #[tool(name = "renamed")]
    fn explicit_name(&self) {}

    #[tool]
    fn implicit_name(&self) {}
}

fn main() {}
