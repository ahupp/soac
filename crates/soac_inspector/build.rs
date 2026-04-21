use build_support::{compute_soac_build_identity, emit_vendored_python_link};

fn main() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace crate should live under crates/ in the repo root");
    let build_identity = compute_soac_build_identity(repo_root)
        .expect("expected to compute SOAC build identity for module cache keys");
    println!("cargo:rustc-env=SOAC_BUILD_IDENTITY={build_identity}");

    emit_vendored_python_link(repo_root).expect("expected to emit vendored CPython link flags");
}
