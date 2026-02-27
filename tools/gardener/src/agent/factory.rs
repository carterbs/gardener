use crate::agent::claude::ClaudeAdapter;
use crate::agent::codex::CodexAdapter;
use crate::agent::AgentAdapter;
use crate::logging::append_run_log;
use crate::types::AgentKind;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Default)]
pub struct AdapterFactory {
    adapters: BTreeMap<AgentKind, Arc<dyn AgentAdapter>>,
}

impl AdapterFactory {
    pub fn with_defaults() -> Self {
        append_run_log(
            "debug",
            "agent.factory.init",
            json!({
                "adapters": ["claude", "codex"]
            }),
        );
        let mut this = Self::default();
        this.register(Arc::new(ClaudeAdapter));
        this.register(Arc::new(CodexAdapter));
        this
    }

    pub fn register(&mut self, adapter: Arc<dyn AgentAdapter>) {
        let backend = adapter.backend();
        append_run_log(
            "debug",
            "agent.factory.register",
            json!({
                "backend": backend.as_str()
            }),
        );
        self.adapters.insert(backend, adapter);
    }

    pub fn get(&self, backend: AgentKind) -> Option<Arc<dyn AgentAdapter>> {
        let result = self.adapters.get(&backend).cloned();
        if result.is_none() {
            append_run_log(
                "warn",
                "agent.factory.get.miss",
                json!({
                    "backend": backend.as_str(),
                    "registered": self.adapters.keys().map(|k| k.as_str()).collect::<Vec<_>>()
                }),
            );
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::AdapterFactory;
    use crate::types::AgentKind;

    #[test]
    fn factory_registers_default_plugins() {
        let factory = AdapterFactory::with_defaults();
        assert!(factory.get(AgentKind::Codex).is_some());
        assert!(factory.get(AgentKind::Claude).is_some());
    }
}
