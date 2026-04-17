use libc::wchar_t;
use pyo3::ffi;
use pyo3::prelude::*;
use std::ffi::CString;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct EmbeddedPythonConfig {
    program_name: String,
    python_home: PathBuf,
    search_paths: Vec<PathBuf>,
}

impl EmbeddedPythonConfig {
    pub fn vendored(repo_root: impl AsRef<Path>, program_name: impl Into<String>) -> Self {
        let repo_root = repo_root.as_ref();
        let python_home = vendored_python_home(repo_root);
        let mut search_paths = vec![python_home.join("Lib")];
        if let Some(build_lib_dir) = vendored_python_build_lib_dir(&python_home) {
            search_paths.push(build_lib_dir);
        }
        Self {
            program_name: program_name.into(),
            python_home,
            search_paths,
        }
    }

    pub fn with_search_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.search_paths.push(path.into());
        self
    }

    pub fn with_search_paths(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.search_paths.extend(paths);
        self
    }

    pub fn python_home(&self) -> &Path {
        &self.python_home
    }

    pub fn search_paths(&self) -> &[PathBuf] {
        &self.search_paths
    }
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace crate should have a repo-root parent")
        .to_path_buf()
}

pub fn vendored_python_home(repo_root: &Path) -> PathBuf {
    repo_root.join("vendor").join("cpython")
}

pub fn vendored_python_build_lib_dir(python_home: &Path) -> Option<PathBuf> {
    let pybuilddir = python_home.join("pybuilddir.txt");
    if let Ok(raw) = std::fs::read_to_string(pybuilddir) {
        let relative = raw.trim();
        if !relative.is_empty() {
            return Some(python_home.join(relative));
        }
    }

    let build_dir = python_home.join("build");
    let entries = std::fs::read_dir(build_dir).ok()?;
    for entry in entries {
        let path = entry.ok()?.path();
        if path.is_dir()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("lib."))
        {
            return Some(path);
        }
    }
    None
}

pub fn test_extension_staging_dir(repo_root: &Path) -> Result<PathBuf, String> {
    let staging_dir = repo_root.join("target").join("debug").join("test-ext");
    let source_ext = repo_root
        .join("target")
        .join("debug")
        .join("lib_soac_ext.so");
    let staged_ext = staging_dir.join("_soac_ext.so");
    std::fs::create_dir_all(&staging_dir).map_err(|err| {
        format!(
            "failed to create test extension staging dir {}: {err}",
            staging_dir.display()
        )
    })?;
    if source_ext.exists() {
        stage_extension_file(&source_ext, &staged_ext)?;
    }
    Ok(staging_dir)
}

#[cfg(unix)]
fn stage_extension_file(source_ext: &Path, staged_ext: &Path) -> Result<(), String> {
    let needs_symlink = std::fs::read_link(staged_ext)
        .map(|target| target != source_ext)
        .unwrap_or(true);
    if !needs_symlink {
        return Ok(());
    }
    if std::fs::symlink_metadata(staged_ext).is_ok() {
        std::fs::remove_file(staged_ext).map_err(|err| {
            format!(
                "failed to remove stale staged extension {}: {err}",
                staged_ext.display()
            )
        })?;
    }
    std::os::unix::fs::symlink(source_ext, staged_ext).map_err(|err| {
        format!(
            "failed to symlink staged extension {} -> {}: {err}",
            staged_ext.display(),
            source_ext.display()
        )
    })
}

#[cfg(not(unix))]
fn stage_extension_file(source_ext: &Path, staged_ext: &Path) -> Result<(), String> {
    std::fs::copy(source_ext, staged_ext)
        .map(|_| ())
        .map_err(|err| {
            format!(
                "failed to copy staged extension {} -> {}: {err}",
                source_ext.display(),
                staged_ext.display()
            )
        })
}

pub fn test_python_config(program_name: impl Into<String>) -> Result<EmbeddedPythonConfig, String> {
    let repo_root = repo_root();
    let staging_dir = test_extension_staging_dir(&repo_root)?;
    Ok(EmbeddedPythonConfig::vendored(&repo_root, program_name)
        .with_search_path(repo_root.join("soac_py").join("src"))
        .with_search_path(staging_dir))
}

pub fn initialize_vendored_python(program_name: impl Into<String>) -> Result<(), String> {
    let config = EmbeddedPythonConfig::vendored(repo_root(), program_name);
    initialize_python_and_ensure_sys_path(&config)
}

pub fn initialize_test_python(program_name: impl Into<String>) -> Result<(), String> {
    let config = test_python_config(program_name)?;
    initialize_python_and_ensure_sys_path(&config)
}

pub fn initialize_python_and_ensure_sys_path(config: &EmbeddedPythonConfig) -> Result<(), String> {
    initialize_python(config)?;
    Python::attach(|py| ensure_sys_path_entries(py, config.search_paths()))
}

pub fn initialize_python(config: &EmbeddedPythonConfig) -> Result<(), String> {
    static INIT_RESULT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT_RESULT
        .get_or_init(|| initialize_python_once(config))
        .clone()
}

fn initialize_python_once(config: &EmbeddedPythonConfig) -> Result<(), String> {
    unsafe {
        if ffi::Py_IsInitialized() != 0 {
            return Ok(());
        }

        let path_config = LeakedPythonPathConfig::from_config(config)?;
        install_pre_init_path_config(&path_config);
        PYTHON_PATH_CONFIG
            .set(path_config)
            .map_err(|_| "embedded Python path config was already installed".to_string())?;
        Python::initialize();
    }
    Ok(())
}

#[allow(deprecated)]
unsafe fn install_pre_init_path_config(path_config: &LeakedPythonPathConfig) {
    // PyConfig would be the modern API, but the public struct layout exposed
    // through PyO3 can lag vendored CPython. These pre-init APIs avoid
    // process-environment mutation without depending on PyConfig field offsets.
    unsafe {
        ffi::Py_SetProgramName(path_config.program_name.as_ptr());
        ffi::Py_SetPythonHome(path_config.python_home.as_ptr());
        Py_SetPath(path_config.python_path.as_ptr());
    }
}

unsafe extern "C" {
    fn Py_SetPath(arg1: *const wchar_t);
}

pub fn ensure_sys_path_entries(py: Python<'_>, paths: &[PathBuf]) -> Result<(), String> {
    let sys_path = py
        .import("sys")
        .map_err(|err| err.to_string())?
        .getattr("path")
        .map_err(|err| err.to_string())?;
    for path in paths.iter().rev() {
        let path = path.to_string_lossy().into_owned();
        let present = sys_path
            .call_method1("__contains__", (path.as_str(),))
            .map_err(|err| err.to_string())?
            .extract::<bool>()
            .map_err(|err| err.to_string())?;
        if !present {
            sys_path
                .call_method1("insert", (0, path.as_str()))
                .map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

static PYTHON_PATH_CONFIG: OnceLock<LeakedPythonPathConfig> = OnceLock::new();

#[derive(Debug)]
struct LeakedPythonPathConfig {
    program_name: DecodedWideString,
    python_home: DecodedWideString,
    python_path: DecodedWideString,
}

impl LeakedPythonPathConfig {
    fn from_config(config: &EmbeddedPythonConfig) -> Result<Self, String> {
        let python_path = std::env::join_paths(config.search_paths())
            .map_err(|err| format!("failed to join Python search paths: {err}"))?;
        Ok(Self {
            program_name: DecodedWideString::from_str(config.program_name.as_str())?,
            python_home: DecodedWideString::from_path(config.python_home())?,
            python_path: DecodedWideString::from_os_str(&python_path)?,
        })
    }
}

unsafe impl Send for LeakedPythonPathConfig {}
unsafe impl Sync for LeakedPythonPathConfig {}

#[derive(Debug)]
struct DecodedWideString {
    ptr: *mut wchar_t,
}

impl DecodedWideString {
    fn from_str(value: &str) -> Result<Self, String> {
        let raw = CString::new(value)
            .map_err(|_| "Python config string contains an interior NUL byte".to_string())?;
        Self::from_cstring(&raw, value)
    }

    fn from_path(path: &Path) -> Result<Self, String> {
        let raw = path_to_cstring(path)?;
        Self::from_cstring(&raw, path.display().to_string().as_str())
    }

    fn from_os_str(value: &std::ffi::OsStr) -> Result<Self, String> {
        let raw = os_str_to_cstring(value)
            .map_err(|_| "Python path contains an interior NUL byte".to_string())?;
        Self::from_cstring(&raw, value.to_string_lossy().as_ref())
    }

    fn from_cstring(raw: &CString, display: &str) -> Result<Self, String> {
        let ptr = unsafe { ffi::Py_DecodeLocale(raw.as_ptr(), ptr::null_mut()) };
        if ptr.is_null() {
            return Err(format!("failed to decode Python config string {display}"));
        }
        Ok(Self { ptr })
    }

    fn as_ptr(&self) -> *const wchar_t {
        self.ptr
    }
}

impl Drop for DecodedWideString {
    fn drop(&mut self) {
        unsafe {
            ffi::PyMem_RawFree(self.ptr.cast());
        }
    }
}

#[cfg(unix)]
fn path_to_cstring(path: &Path) -> Result<CString, String> {
    os_str_to_cstring(path.as_os_str()).map_err(|_| {
        format!(
            "Python path contains an interior NUL byte: {}",
            path.display()
        )
    })
}

#[cfg(not(unix))]
fn path_to_cstring(path: &Path) -> Result<CString, String> {
    os_str_to_cstring(path.as_os_str()).map_err(|_| {
        format!(
            "Python path contains an interior NUL byte: {}",
            path.display()
        )
    })
}

#[cfg(unix)]
fn os_str_to_cstring(value: &std::ffi::OsStr) -> Result<CString, std::ffi::NulError> {
    CString::new(value.as_bytes())
}

#[cfg(not(unix))]
fn os_str_to_cstring(value: &std::ffi::OsStr) -> Result<CString, std::ffi::NulError> {
    CString::new(value.to_string_lossy().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendored_config_contains_stdlib_path() {
        let root = repo_root();
        let config = EmbeddedPythonConfig::vendored(&root, "soac-test");

        assert_eq!(config.python_home(), vendored_python_home(&root));
        assert!(
            config
                .search_paths()
                .contains(&config.python_home.join("Lib"))
        );
    }

    #[test]
    fn test_config_contains_soac_sources_and_staging_dir() {
        let root = repo_root();
        let config = test_python_config("soac-test").unwrap();

        assert!(config.search_paths().contains(&root.join("soac_py/src")));
        assert!(
            config
                .search_paths()
                .contains(&root.join("target/debug/test-ext"))
        );
    }
}
