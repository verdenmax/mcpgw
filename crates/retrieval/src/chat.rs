//! The `ChatModel` abstraction: a single-shot chat completion (system + user -> text). The
//! HTTP-backed provider lives in the separate `chat` crate; this module only defines the trait,
//! errors, and a deterministic `MockChatModel` (behind the `testkit` feature) for tests.

use async_trait::async_trait;

/// Errors from a chat completion. Provider-agnostic so `retrieval` needs no HTTP dependency.
#[derive(Debug, thiserror::Error)]
pub enum ChatError {
    #[error("chat provider error: {0}")]
    Provider(String),
    #[error("chat model returned no usable content")]
    Empty,
}

/// A single-shot chat model: a system + user prompt yields assistant text.
#[async_trait]
pub trait ChatModel: Send + Sync {
    async fn complete(&self, system: &str, user: &str) -> Result<String, ChatError>;
}

#[cfg(feature = "testkit")]
mod mock {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Deterministic test chat model: returns a scripted `reply` (or errors when `fail`), and
    /// records call count + the last (system, user) prompts for assertions.
    pub struct MockChatModel {
        reply: String,
        fail: bool,
        pub calls: Arc<AtomicUsize>,
        pub last_system: Arc<Mutex<String>>,
        pub last_user: Arc<Mutex<String>>,
    }

    impl MockChatModel {
        /// A model that always returns `reply`.
        pub fn new(reply: impl Into<String>) -> Self {
            Self {
                reply: reply.into(),
                fail: false,
                calls: Arc::new(AtomicUsize::new(0)),
                last_system: Arc::new(Mutex::new(String::new())),
                last_user: Arc::new(Mutex::new(String::new())),
            }
        }
        /// A model whose `complete` always errors (drives degradation tests).
        pub fn failing() -> Self {
            Self {
                fail: true,
                ..Self::new("")
            }
        }
    }

    #[async_trait]
    impl ChatModel for MockChatModel {
        async fn complete(&self, system: &str, user: &str) -> Result<String, ChatError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_system.lock().unwrap() = system.to_string();
            *self.last_user.lock().unwrap() = user.to_string();
            if self.fail {
                return Err(ChatError::Provider("mock failure".into()));
            }
            Ok(self.reply.clone())
        }
    }
}

#[cfg(feature = "testkit")]
pub use mock::MockChatModel;

#[cfg(all(test, feature = "testkit"))]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[tokio::test]
    async fn mock_returns_scripted_reply_and_records_prompts() {
        let m = MockChatModel::new("hello");
        let out = m.complete("sys", "usr").await.expect("ok");
        assert_eq!(out, "hello");
        assert_eq!(m.calls.load(Ordering::SeqCst), 1);
        assert_eq!(*m.last_system.lock().unwrap(), "sys");
        assert_eq!(*m.last_user.lock().unwrap(), "usr");
    }

    #[tokio::test]
    async fn failing_mock_errors() {
        let m = MockChatModel::failing();
        assert!(m.complete("s", "u").await.is_err());
    }
}
