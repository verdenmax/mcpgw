//! Test-only: run the in-memory MockUpstream over real stdio, so the subprocess
//! connect path (`connect_stdio_upstream`) can be smoke-tested against a real child.
use rmcp::transport::stdio;
use rmcp::ServiceExt;
use upstream::testkit::MockUpstream;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = MockUpstream::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
