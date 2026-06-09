//! Registry of live upstream connections, keyed by server name, plus connection state.

use std::collections::HashMap;
use std::sync::Arc;

use crate::connection::UpstreamHandle;

/// Lifecycle state of an upstream connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamState {
    Connecting,
    Ready,
    Failed,
}

/// Thread-safe registry mapping server name -> connected handle.
#[derive(Clone, Default)]
pub struct UpstreamRegistry {
    inner: Arc<std::sync::RwLock<HashMap<String, Arc<UpstreamHandle>>>>,
}

impl UpstreamRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert (or replace) the handle for its server name. Re-inserting the same
    /// server name supersedes the prior handle; if nothing else holds the old `Arc`,
    /// its rmcp service is cancelled on drop.
    pub fn insert(&self, handle: Arc<UpstreamHandle>) {
        self.inner
            .write()
            .unwrap()
            .insert(handle.server().to_string(), handle);
    }

    pub fn get(&self, server: &str) -> Option<Arc<UpstreamHandle>> {
        self.inner.read().unwrap().get(server).cloned()
    }

    /// Remove and return the handle for `server`, if present. The caller may keep the
    /// returned `Arc` to perform a graceful `shutdown().await`; dropping it cancels the
    /// rmcp service.
    pub fn remove(&self, server: &str) -> Option<Arc<UpstreamHandle>> {
        self.inner.write().unwrap().remove(server)
    }

    pub fn server_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.inner.read().unwrap().keys().cloned().collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_values_are_distinct() {
        assert_ne!(UpstreamState::Connecting, UpstreamState::Ready);
        assert_ne!(UpstreamState::Ready, UpstreamState::Failed);
        assert_ne!(UpstreamState::Connecting, UpstreamState::Failed);
    }
}
