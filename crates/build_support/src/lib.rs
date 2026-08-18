pub fn compute_soac_build_identity(repo_root: &std::path::Path) -> std::io::Result<String> {
    let mut hasher = StableHasher::new();
    let package_version =
        std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").into());
    hasher.update(package_version.as_bytes());
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
        "crates/soac_lowering",
        "crates/soac_jit",
        "crates/soac_macros",
        "crates/soac_pyo3",
        "crates/soac_jit_runtime",
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

pub fn emit_vendored_python_link(repo_root: &std::path::Path) -> std::io::Result<()> {
    let python_lib_dir = vendored_python_lib_dir(repo_root);
    let python_link_name = find_vendored_python_shared_lib_name(repo_root)?;
    println!("cargo:rerun-if-changed={}", python_lib_dir.display());
    println!(
        "cargo:rustc-link-search=native={}",
        python_lib_dir.display()
    );
    println!(
        "cargo:rustc-link-arg=-Wl,-rpath,{}",
        python_lib_dir.display()
    );
    println!("cargo:rustc-link-lib=dylib={python_link_name}");
    Ok(())
}

pub fn find_vendored_python_shared_lib_name(
    repo_root: &std::path::Path,
) -> std::io::Result<String> {
    let python_lib_dir = vendored_python_lib_dir(repo_root);
    find_python_shared_lib_name(&python_lib_dir).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "expected vendored shared libpython under {}",
                python_lib_dir.display()
            ),
        )
    })
}

pub fn vendored_python_lib_dir(repo_root: &std::path::Path) -> std::path::PathBuf {
    repo_root.join("vendor").join("cpython")
}

fn find_python_shared_lib_name(dir: &std::path::Path) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut candidates = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !file_name.starts_with("libpython") || !file_name.ends_with(".so") {
            continue;
        }
        let Some(link_name) = file_name
            .strip_prefix("lib")
            .and_then(|name| name.strip_suffix(".so"))
        else {
            continue;
        };
        if !link_name
            .strip_prefix("python")
            .is_some_and(|version| version.contains('.'))
        {
            continue;
        }
        candidates.push(link_name.to_owned());
    }
    candidates.sort_unstable();
    candidates.pop()
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

#[cfg(test)]
mod tests {
    use super::find_python_shared_lib_name;

    #[test]
    fn selects_versioned_libpython_instead_of_abi_compatibility_stub() {
        let dir = std::env::temp_dir().join(format!(
            "soac-build-support-versioned-libpython-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("libpython3.so"), b"abi compatibility stub").unwrap();
        std::fs::write(dir.join("libpython3.15.so"), b"real shared interpreter").unwrap();

        assert_eq!(
            find_python_shared_lib_name(&dir).as_deref(),
            Some("python3.15")
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_unversioned_libpython_abi_compatibility_stub() {
        let dir = std::env::temp_dir().join(format!(
            "soac-build-support-compatibility-stub-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("libpython3.so"), b"abi compatibility stub").unwrap();

        assert_eq!(find_python_shared_lib_name(&dir), None);

        std::fs::remove_dir_all(dir).unwrap();
    }
}
