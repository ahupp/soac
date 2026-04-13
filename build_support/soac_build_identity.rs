pub fn compute_soac_build_identity(repo_root: &std::path::Path) -> std::io::Result<String> {
    let mut hasher = StableHasher::new();
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    hasher.update(b"\0");
    for key in [
        "PROFILE",
        "CARGO_CFG_TARGET_ARCH",
        "CARGO_CFG_TARGET_OS",
        "CARGO_CFG_TARGET_ENV",
    ] {
        if let Ok(value) = std::env::var(key) {
            hasher.update(key.as_bytes());
            hasher.update(b"=");
            hasher.update(value.as_bytes());
            hasher.update(b"\0");
        }
    }

    let mut paths = Vec::new();
    for relative in [
        "Cargo.toml",
        "Cargo.lock",
        "soac-blockpy",
        "soac-jit",
        "soac-macros",
        "soac-pyo3",
        "soac-runtime",
    ] {
        collect_identity_paths(&repo_root.join(relative), &mut paths)?;
    }
    paths.sort();

    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
        let relative = path.strip_prefix(repo_root).unwrap_or(path.as_path());
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update(b"\0");
        hasher.update(std::fs::read(path)?.as_slice());
        hasher.update(b"\0");
    }

    Ok(format!("{:016x}", hasher.finish()))
}

fn collect_identity_paths(
    path: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) -> std::io::Result<()> {
    if path.is_file() {
        if path_is_build_identity_input(path) {
            out.push(path.to_path_buf());
        }
        return Ok(());
    }
    if !path.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let child = entry.path();
        if child.is_dir() {
            collect_identity_paths(&child, out)?;
        } else if path_is_build_identity_input(&child) {
            out.push(child);
        }
    }
    Ok(())
}

fn path_is_build_identity_input(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("rs") | Some("toml") | Some("lock")
    )
}

struct StableHasher {
    hash: u64,
}

impl StableHasher {
    fn new() -> Self {
        Self {
            hash: 0xcbf29ce484222325,
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.hash ^= u64::from(*byte);
            self.hash = self.hash.wrapping_mul(0x00000100000001b3);
        }
    }

    fn finish(self) -> u64 {
        self.hash
    }
}
