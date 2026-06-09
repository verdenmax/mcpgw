# L4 — `crates/upstream/src/registry.rs` API

源文件：`crates/upstream/src/registry.rs`。活上游连接的注册表 + 连接状态枚举。

## `enum UpstreamState`
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamState {
    Connecting,
    Ready,
    Failed,
}
```
上游连接的生命周期状态。目前仅为定义（M1-B 网关接入时使用），尚未与注册表联动。

## `struct UpstreamRegistry`
```rust
#[derive(Clone, Default)]
pub struct UpstreamRegistry {
    /* inner: Arc<RwLock<HashMap<String, Arc<UpstreamHandle>>>>（私有） */
}
```
线程安全注册表，`server name -> Arc<UpstreamHandle>`。`Clone` 共享同一份内层状态。

| 方法 | 签名 | 返回 / 说明 |
|------|------|-------------|
| `new` | `pub fn new() -> Self` | 空注册表（= `Default`） |
| `insert` | `pub fn insert(&self, handle: Arc<UpstreamHandle>)` | 按 `handle.server()` 插入/替换；同名覆盖时旧 `Arc` 若无人持有则 drop 即取消其 rmcp 服务 |
| `get` | `pub fn get(&self, server: &str) -> Option<Arc<UpstreamHandle>>` | 命中则返回 `Arc` 克隆 |
| `remove` | `pub fn remove(&self, server: &str) -> Option<Arc<UpstreamHandle>>` | 摘除并返回 `Arc`；调用方可据此 graceful `shutdown().await`，丢弃则取消服务 |
| `server_names` | `pub fn server_names(&self) -> Vec<String>` | 已注册 server 名，**升序排序** |

- 锁仅在 map 操作期间持有，**不跨 await**；`unwrap()` 锁——遵守"不在持锁期间 panic"的约束。

> 详见 L3：[upstream](../L3-details/upstream.md)
