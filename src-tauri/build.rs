use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const SHORT_SHA_LEN: usize = 7;

fn main() {
    emit_rerun_hints();

    let commit = resolve_commit_sha().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GIT_COMMIT={commit}");

    tauri_build::build();
}

fn emit_rerun_hints() {
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/packed-refs");

    let head_path = PathBuf::from(".git/HEAD");
    let Ok(head_contents) = fs::read_to_string(head_path) else {
        return;
    };

    let Some(ref_path) = head_contents.strip_prefix("ref: ") else {
        return;
    };

    let ref_path = ref_path.trim();
    if ref_path.is_empty() {
        return;
    }

    println!("cargo:rerun-if-changed=.git/{ref_path}");
}

fn resolve_commit_sha() -> Option<String> {
    if let Ok(sha) = env::var("GITHUB_SHA") {
        if let Some(short) = normalize_sha(&sha) {
            return Some(short);
        }
    }

    let output = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let sha = String::from_utf8(output.stdout).ok()?;
    normalize_sha(&sha)
}

fn normalize_sha(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    Some(trimmed.chars().take(SHORT_SHA_LEN).collect())
}
