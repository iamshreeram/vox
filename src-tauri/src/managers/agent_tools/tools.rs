// M2 stub: bodies arrive in M2-1..M2-3, this allow is removed in M2-3
#![allow(dead_code)]

//! Tool dispatcher (DESIGN D4): specs, the ReadOnly risk gate, and the four read tools
//! plus `open_settings` and `recent_conversations`.

use serde_json::Value;

use crate::managers::agent_tools::project::ProjectResolver;
use crate::managers::agent_tools::ui_port::UiPort;
use crate::managers::converse::BoxFuture;
use crate::managers::llm_tools::ToolSpec;
use crate::managers::tool_loop::{ToolExecutor, ToolOutput};

/// Default executor. Holds the resolved roots and the UI port; no mutable shared state.
pub struct DefaultToolExecutor {
    resolver: ProjectResolver,
    ui: Box<dyn UiPort>,
    recent_conversations_enabled: bool,
}

impl DefaultToolExecutor {
    pub fn new(
        resolver: ProjectResolver,
        ui: Box<dyn UiPort>,
        recent_conversations_enabled: bool,
    ) -> Self {
        let _ = (resolver, ui, recent_conversations_enabled);
        todo!()
    }
}

impl ToolExecutor for DefaultToolExecutor {
    fn specs(&self) -> Vec<ToolSpec> {
        todo!()
    }

    fn call<'a>(&'a self, name: &'a str, input: &'a Value) -> BoxFuture<'a, ToolOutput> {
        let _ = (name, input);
        todo!()
    }
}
