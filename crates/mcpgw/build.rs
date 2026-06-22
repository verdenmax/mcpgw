use std::process::Command;

fn main() {
    // 取构建时短 commit SHA；非 git 仓库/无 git/失败 -> "unknown"（优雅降级）。
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=MCPGW_GIT_SHA={sha}");

    // 构建时间（epoch 秒，前端格式化）。不强制每次重跑，故为「最近一次重建」的近似时间。
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=MCPGW_BUILD_TIME={ts}");
}
