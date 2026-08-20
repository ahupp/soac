use std::path::{Path, PathBuf};

fn main() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace crate should live under crates/ in the repo root");
    build_support::emit_vendored_python_link(repo_root)
        .expect("expected to emit selected CPython link flags");

    // Cargo supplies this exact profile directory, including configured target
    // roots and target triples. Runtime environment changes must not redirect a
    // test binary to an extension from a different build.
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"));
    let artifacts = out
        .ancestors()
        .nth(3)
        .expect("OUT_DIR is <artifacts>/build/<package>/out");
    assert_eq!(
        out.parent()
            .and_then(Path::parent)
            .and_then(Path::file_name),
        Some("build".as_ref())
    );
    println!(
        "cargo:rustc-env=SOAC_TEST_ARTIFACT_DIR={}",
        artifacts.display()
    );
    println!("cargo:rerun-if-changed=build.rs");
}
