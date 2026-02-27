use crate::agent::claude::ClaudeAdapter;
use crate::agent::codex::CodexAdapter;
use crate::agent::AgentAdapter;
use crate::types::AgentKind;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Default)]
pub struct AdapterFactory {
    adapters: BTreeMap<AgentKind, Arc<dyn AgentAdapter>>,
}

impl AdapterFactory {
    pub fn with_defaults() -> Self {
        let mut this = Self::default();
        this.register(Arc::new(ClaudeAdapter));
        this.register(Arc::new(CodexAdapter));
        this
    }

    pub fn register(&mut self, adapter: Arc<dyn AgentAdapter>) {
        self.adapters.insert(adapter.backend(), adapter);
    }

    pub fn get(&self, backend: AgentKind) -> Option<Arc<dyn AgentAdapter>> {
        self.adapters.get(&backend).cloned()
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
