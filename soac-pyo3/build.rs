include!("../build_support/soac_build_identity.rs");

fn main() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace crate should have a repo-root parent");
    let build_identity = compute_soac_build_identity(repo_root)
        .expect("expected to compute SOAC build identity for module cache keys");
    println!("cargo:rustc-env=SOAC_BUILD_IDENTITY={build_identity}");

    emit_vendored_python_link(repo_root).expect("expected to emit vendored CPython link flags");
}
