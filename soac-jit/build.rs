use std::env;
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use pyo3_build_config::{BuildFlag, PythonImplementation};

const RUNTIME_CRATE_NAME: &str = "soac_runtime";

fn main() -> Result<(), Box<dyn Error>> {
    pyo3_build_config::use_pyo3_cfgs();
    ensure_supported_python_runtime_layout()?;

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let repo_root = manifest_dir
        .parent()
        .ok_or("soac-jit should live under the repo root")?;
    let runtime_dir = repo_root.join("soac-runtime");
    let runtime_src = runtime_dir.join("src").join("lib.rs");
    emit_vendored_python_link(repo_root)?;
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let clif_out_dir = out_dir.join("soac-runtime-clif");

    emit_rerun_if_changed(&runtime_dir)?;
    fs::create_dir_all(&clif_out_dir)?;

    let build_output = build_runtime_clif(&runtime_src, &clif_out_dir)?;
    let runtime_clif = find_runtime_clif_files(&clif_out_dir)?;
    if runtime_clif.is_empty() {
        return Err(format!(
            "failed to emit runtime CLIF; rustc output was:\nstdout:\n{}\nstderr:\n{}",
            build_output.stdout, build_output.stderr
        )
        .into());
    }

    write_runtime_clif_constant(&runtime_clif)?;
    Ok(())
}

fn emit_vendored_python_link(repo_root: &Path) -> Result<(), Box<dyn Error>> {
    let python_lib_dir = repo_root.join("vendor").join("cpython");
    let python_link_name = find_python_shared_lib_name(&python_lib_dir)
        .ok_or("expected vendored shared libpython under vendor/cpython")?;
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

fn find_python_shared_lib_name(dir: &Path) -> Option<String> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries {
        let path = entry.ok()?.path();
        let file_name = path.file_name()?.to_str()?;
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

fn ensure_supported_python_runtime_layout() -> Result<(), Box<dyn Error>> {
    let config = pyo3_build_config::get();
    if !matches!(config.implementation, PythonImplementation::CPython) {
        return Err(format!(
            "runtime CLIF refcount support only supports CPython; found {:?}",
            config.implementation
        )
        .into());
    }

    let mut unsupported_modes = Vec::new();
    if config.is_free_threaded() {
        unsupported_modes.push("free-threaded");
    }
    if config.build_flags.0.contains(&BuildFlag::Py_REF_DEBUG) {
        unsupported_modes.push("Py_REF_DEBUG");
    }
    if config.build_flags.0.contains(&BuildFlag::Py_TRACE_REFS) {
        unsupported_modes.push("Py_TRACE_REFS");
    }

    if unsupported_modes.is_empty() {
        return Ok(());
    }

    Err(format!(
        "runtime CLIF refcount support does not support this CPython build: {}",
        unsupported_modes.join(", ")
    )
    .into())
}

fn emit_rerun_if_changed(runtime_dir: &Path) -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed={}", runtime_dir.display());
    for path in walk_files(runtime_dir)? {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    Ok(())
}

struct BuildOutput {
    stdout: String,
    stderr: String,
}

fn build_runtime_clif(
    runtime_src: &Path,
    clif_out_dir: &Path,
) -> Result<BuildOutput, Box<dyn Error>> {
    let clif_dir = clif_output_dir(clif_out_dir);
    if clif_dir.exists() {
        fs::remove_dir_all(&clif_dir)?;
    }
    let output = Command::new("rustup")
        .arg("run")
        .arg("nightly")
        .arg("rustc")
        .arg("-Z")
        .arg("codegen-backend=cranelift")
        .arg(runtime_src)
        .arg("--crate-name")
        .arg(RUNTIME_CRATE_NAME)
        .arg("--crate-type")
        .arg("rlib")
        .arg("--edition=2024")
        .arg("--emit=llvm-ir")
        .arg("--out-dir")
        .arg(clif_out_dir)
        .arg("-Ccodegen-units=1")
        .arg("-Copt-level=3")
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if output.status.success() || clif_output_dir(clif_out_dir).exists() {
        return Ok(BuildOutput { stdout, stderr });
    }

    Err(format!(
        "failed to build soac-runtime to CLIF with rustc-codegen-cranelift\nstdout:\n{stdout}\nstderr:\n{stderr}"
    )
    .into())
}

fn clif_output_dir(clif_out_dir: &Path) -> PathBuf {
    clif_out_dir.join(format!("{RUNTIME_CRATE_NAME}.clif"))
}

fn find_runtime_clif_files(clif_out_dir: &Path) -> Result<Vec<(String, String)>, Box<dyn Error>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(clif_output_dir(clif_out_dir))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension() != Some(OsStr::new("clif")) {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        let Some(symbol) = file_name.strip_suffix(".opt.clif") else {
            continue;
        };
        if !symbol.starts_with("soac_runtime_") {
            continue;
        }
        entries.push((symbol.to_string(), fs::read_to_string(path)?));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries)
}

fn write_runtime_clif_constant(runtime_clif: &[(String, String)]) -> Result<(), Box<dyn Error>> {
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let out_path = out_dir.join("soac_runtime_clif.rs");
    let mut contents = String::from("pub const SOAC_RUNTIME_CLIF: &[(&str, &str)] = &[\n");
    for (symbol, clif) in runtime_clif {
        contents.push_str("    (");
        contents.push_str(&format!("{symbol:?}, "));
        contents.push_str(&raw_string_literal(clif));
        contents.push_str("),\n");
    }
    contents.push_str("];\n");
    fs::write(out_path, contents)?;
    Ok(())
}

fn raw_string_literal(text: &str) -> String {
    let mut max_run = 0usize;
    let mut current = 0usize;
    for ch in text.chars() {
        if ch == '#' {
            current += 1;
            max_run = max_run.max(current);
        } else {
            current = 0;
        }
    }
    let hashes = "#".repeat(max_run + 1);
    format!("r{hashes}\"{text}\"{hashes}")
}

fn walk_files(dir: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut files = Vec::new();
    walk_files_inner(dir, &mut files)?;
    Ok(files)
}

fn walk_files_inner(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            walk_files_inner(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}
