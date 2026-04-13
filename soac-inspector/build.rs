include!("../build_support/soac_build_identity.rs");

fn main() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace crate should have a repo-root parent");
    let build_identity = compute_soac_build_identity(repo_root)
        .expect("expected to compute SOAC build identity for module cache keys");
    println!("cargo:rustc-env=SOAC_BUILD_IDENTITY={build_identity}");

    let python_lib_dir = repo_root.join("vendor/cpython");
    let python_link_name =
        find_python_shared_lib_name(&python_lib_dir).expect("expected vendored shared libpython");
    let python_lib_dir = python_lib_dir.display();
    println!("cargo:rustc-link-search=native={python_lib_dir}");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{python_lib_dir}");
    println!("cargo:rustc-link-lib=dylib={python_link_name}");
}

fn find_python_shared_lib_name(dir: &std::path::Path) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
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
        return file_name
            .strip_prefix("lib")
            .and_then(|name| name.strip_suffix(".so"))
            .map(ToOwned::to_owned);
    }
    None
}
