//! Runtime disable set: temporarily hide an upstream (namespace) or a single qualified tool from
//! the gateway's snapshot. Pure in-memory `BTreeSet`s with optional JSON persistence (atomic,
//! best-effort) so disables survive a restart when `[dashboard].disabled_state_path` is set.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

/// The disabled names, serialized form is `DisabledSnapshot`.
#[derive(Default)]
struct DisabledState {
    upstreams: BTreeSet<String>,
    tools: BTreeSet<String>,
}

impl DisabledState {
    fn to_snapshot(&self) -> DisabledSnapshot {
        DisabledSnapshot {
            upstreams: self.upstreams.iter().cloned().collect(),
            tools: self.tools.iter().cloned().collect(),
        }
    }
}

/// Ordered, owned view of the disabled set — the `GET /api/disabled` body and the on-disk form.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DisabledSnapshot {
    #[serde(default)]
    pub upstreams: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
}

/// In-memory disable set with optional JSON persistence. Cheaply shared behind an `Arc`.
pub struct DisableSet {
    inner: RwLock<DisabledState>,
    path: Option<PathBuf>,
}

impl Default for DisableSet {
    /// Empty, no persistence (in-memory only) — the default a plain `GatewayState` carries.
    fn default() -> Self {
        Self {
            inner: RwLock::new(DisabledState::default()),
            path: None,
        }
    }
}

impl DisableSet {
    /// Build from an optional state-file path. With a path that exists, load it; a missing file is
    /// an empty set (normal); a corrupt/unreadable file degrades to empty + `warn!` (self-healing,
    /// never blocks startup). With no path, an empty in-memory set.
    pub fn load_or_new(path: Option<PathBuf>) -> Self {
        let mut state = DisabledState::default();
        if let Some(p) = path.as_deref() {
            match std::fs::read_to_string(p) {
                Ok(text) => match serde_json::from_str::<DisabledSnapshot>(&text) {
                    Ok(snap) => {
                        state.upstreams = snap.upstreams.into_iter().collect();
                        state.tools = snap.tools.into_iter().collect();
                    }
                    Err(e) => {
                        tracing::warn!(path = %p.display(), error = %e,
                            "disabled state file is corrupt; starting with an empty disable set");
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {} // first run: empty
                Err(e) => {
                    tracing::warn!(path = %p.display(), error = %e,
                        "could not read disabled state file; starting with an empty disable set");
                }
            }
        }
        Self {
            inner: RwLock::new(state),
            path,
        }
    }

    pub fn is_upstream_disabled(&self, name: &str) -> bool {
        self.inner.read().unwrap().upstreams.contains(name)
    }

    pub fn is_tool_disabled(&self, qualified: &str) -> bool {
        self.inner.read().unwrap().tools.contains(qualified)
    }

    /// Ordered snapshot (the API body + the persisted form).
    pub fn snapshot(&self) -> DisabledSnapshot {
        self.inner.read().unwrap().to_snapshot()
    }

    pub fn disable_upstream(&self, name: &str) -> bool {
        self.mutate(|s| s.upstreams.insert(name.to_string()))
    }
    pub fn enable_upstream(&self, name: &str) -> bool {
        self.mutate(|s| s.upstreams.remove(name))
    }
    pub fn disable_tool(&self, qualified: &str) -> bool {
        self.mutate(|s| s.tools.insert(qualified.to_string()))
    }
    pub fn enable_tool(&self, qualified: &str) -> bool {
        self.mutate(|s| s.tools.remove(qualified))
    }

    /// Apply `f` under the write lock; if it reports a change, persist (best-effort) before
    /// releasing the lock so the on-disk form matches the in-memory set. Returns whether changed.
    fn mutate(&self, f: impl FnOnce(&mut DisabledState) -> bool) -> bool {
        let mut s = self.inner.write().unwrap();
        let changed = f(&mut s);
        if changed {
            if let Some(p) = self.path.as_deref() {
                let snap = s.to_snapshot();
                persist(p, &snap);
            }
        }
        changed
    }
}

/// Best-effort atomic write: serialize to a sibling temp file, fsync, then rename over `path`.
/// Any failure is logged and swallowed — the in-memory set stays authoritative and the next
/// successful toggle rewrites the whole file. (A separate `write_atomic` fn avoids the
/// immediately-invoked-closure that would trip `clippy::redundant_closure_call`.)
fn persist(path: &Path, snap: &DisabledSnapshot) {
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    if let Err(e) = write_atomic(&tmp, path, snap) {
        let _ = std::fs::remove_file(&tmp); // don't leak a temp on failure
        tracing::warn!(path = %path.display(), error = %e,
            "could not persist disabled state file (in-memory set is still authoritative)");
    }
}

fn write_atomic(tmp: &Path, path: &Path, snap: &DisabledSnapshot) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(snap)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let mut f = std::fs::File::create(tmp)?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mcpgw-dis-{}-{name}.json", std::process::id()))
    }

    #[test]
    fn disable_enable_report_changed_and_are_idempotent() {
        let d = DisableSet::default();
        assert!(d.disable_upstream("a")); // newly inserted -> changed
        assert!(!d.disable_upstream("a")); // already there -> no change
        assert!(d.is_upstream_disabled("a"));
        assert!(d.enable_upstream("a")); // removed -> changed
        assert!(!d.enable_upstream("a")); // absent -> no change
        assert!(!d.is_upstream_disabled("a"));
    }

    #[test]
    fn tool_and_upstream_axes_are_independent() {
        let d = DisableSet::default();
        d.disable_tool("srv__echo");
        assert!(d.is_tool_disabled("srv__echo"));
        assert!(!d.is_upstream_disabled("srv__echo")); // tool name is not an upstream name
        assert!(!d.is_tool_disabled("srv__greet"));
    }

    #[test]
    fn snapshot_is_sorted() {
        let d = DisableSet::default();
        d.disable_upstream("b");
        d.disable_upstream("a");
        d.disable_tool("z__t");
        d.disable_tool("a__t");
        let s = d.snapshot();
        assert_eq!(s.upstreams, vec!["a", "b"]);
        assert_eq!(s.tools, vec!["a__t", "z__t"]);
    }

    #[test]
    fn persists_and_reloads_across_instances() {
        let p = tmp("roundtrip");
        let _ = std::fs::remove_file(&p);
        {
            let d = DisableSet::load_or_new(Some(p.clone()));
            d.disable_upstream("flaky");
            d.disable_tool("github__delete_repo");
        }
        // No leftover temp file beside the state file.
        assert!(!p
            .with_extension(format!("tmp.{}", std::process::id()))
            .exists());
        let reloaded = DisableSet::load_or_new(Some(p.clone()));
        assert!(reloaded.is_upstream_disabled("flaky"));
        assert!(reloaded.is_tool_disabled("github__delete_repo"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn missing_file_loads_empty() {
        let p = tmp("missing");
        let _ = std::fs::remove_file(&p);
        let d = DisableSet::load_or_new(Some(p));
        assert_eq!(d.snapshot(), DisabledSnapshot::default());
    }

    #[test]
    fn corrupt_file_loads_empty_without_panic() {
        let p = tmp("corrupt");
        std::fs::write(&p, b"{ this is not json").unwrap();
        let d = DisableSet::load_or_new(Some(p.clone()));
        assert_eq!(d.snapshot(), DisabledSnapshot::default());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn unwritable_path_is_best_effort_and_keeps_memory_state() {
        // A path under a non-existent directory: persistence fails, but the in-memory set still
        // reflects the change and no panic occurs.
        let p = std::env::temp_dir()
            .join(format!("mcpgw-dis-nodir-{}", std::process::id()))
            .join("state.json");
        let d = DisableSet::load_or_new(Some(p));
        assert!(d.disable_upstream("x")); // still reports changed
        assert!(d.is_upstream_disabled("x")); // memory is authoritative despite write failure
    }

    #[test]
    fn enable_persists_and_reloads_as_empty() {
        let p = tmp("enable-roundtrip");
        let _ = std::fs::remove_file(&p);
        {
            let d = DisableSet::load_or_new(Some(p.clone()));
            d.disable_upstream("x");
            d.enable_upstream("x"); // persisted back to empty
        }
        let reloaded = DisableSet::load_or_new(Some(p.clone()));
        assert!(!reloaded.is_upstream_disabled("x"));
        assert_eq!(reloaded.snapshot(), DisabledSnapshot::default());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn invalid_utf8_file_loads_empty_without_panic() {
        let p = tmp("badutf8");
        std::fs::write(&p, [0xff, 0xfe, 0x00, 0x9f]).unwrap(); // not valid UTF-8
        let d = DisableSet::load_or_new(Some(p.clone()));
        assert_eq!(d.snapshot(), DisabledSnapshot::default());
        let _ = std::fs::remove_file(&p);
    }
}
