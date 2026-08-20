use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use ruff_db::system::walk_directory::{
    DirectoryWalker, IgnoreIncremental, WalkDirectoryBuilder, WalkDirectoryConfiguration,
    WalkDirectoryVisitorBuilder,
};
use ruff_db::system::{
    DirectoryEntry, DirectoryFilter, FileType, Metadata, OsSystem, System, SystemPath,
    SystemPathBuf, SystemVirtualPath, WhichError, WhichResult, WritableSystem,
};
use ruff_notebook::{Notebook, NotebookError};
use soac_contracts::{
    AnalysisDirectoryFilter, AnalysisEnvironmentVariable, AnalysisInput, AnalysisInputState,
    Fingerprint, capture_analysis_input, capture_analysis_input_with_filters,
    verify_analysis_inputs,
};

#[derive(Debug, Default)]
struct Observations {
    paths: BTreeMap<PathBuf, AnalysisInputState>,
    environment: BTreeMap<String, Option<Fingerprint>>,
    errors: Vec<String>,
}

/// A read-only ty OS database with an authenticated input boundary. Cached
/// directory listings report their actual name/prefix/suffix queries; neither
/// output files nor bytecode-cache names are globally hidden from imports.
#[derive(Debug, Clone)]
pub(crate) struct AnalysisSystem {
    inner: OsSystem,
    observations: Arc<Mutex<Observations>>,
}

impl AnalysisSystem {
    pub(crate) fn new(root: &SystemPath) -> Self {
        Self {
            inner: OsSystem::new(root),
            observations: Arc::default(),
        }
    }

    fn error(&self, message: impl ToString) -> io::Error {
        let message = message.to_string();
        self.observations
            .lock()
            .expect("analysis input lock poisoned")
            .errors
            .push(message.clone());
        io::Error::other(message)
    }

    fn record_state(&self, path: &SystemPath, mut state: AnalysisInputState) -> io::Result<()> {
        let path = path.as_std_path().to_path_buf();
        let mut observations = self
            .observations
            .lock()
            .expect("analysis input lock poisoned");
        if let Some(previous) = observations.paths.get(&path) {
            match (previous, &mut state) {
                (
                    AnalysisInputState::Directory {
                        canonical_path: old,
                        observations: old_views,
                    },
                    AnalysisInputState::Directory {
                        canonical_path: new,
                        observations: new_views,
                    },
                ) if old == new => {
                    for previous in old_views {
                        if let Some(current) =
                            new_views.iter().find(|view| view.filter == previous.filter)
                        {
                            if current != previous {
                                let message = format!(
                                    "analysis directory query changed while ty was reading {}",
                                    path.display()
                                );
                                observations.errors.push(message.clone());
                                return Err(io::Error::other(message));
                            }
                        } else {
                            new_views.push(previous.clone());
                        }
                    }
                    new_views.sort_by(|left, right| left.filter.cmp(&right.filter));
                }
                (previous, current) if previous != &*current => {
                    let message = format!(
                        "analysis input changed while ty was reading {}",
                        path.display()
                    );
                    observations.errors.push(message.clone());
                    return Err(io::Error::other(message));
                }
                _ => {}
            }
        }
        observations.paths.insert(path, state);
        Ok(())
    }

    fn record(&self, path: &SystemPath) -> io::Result<()> {
        let state =
            capture_analysis_input(path.as_std_path(), false).map_err(|error| self.error(error))?;
        self.record_state(path, state)
    }

    fn record_view(
        &self,
        path: &SystemPath,
        filter: AnalysisDirectoryFilter,
        entries: impl IntoIterator<Item = (String, FileType)>,
    ) -> io::Result<()> {
        let mut expected: Vec<_> = entries
            .into_iter()
            .map(|(name, kind)| (name, kind.is_file(), kind.is_directory(), kind.is_symlink()))
            .collect();
        expected.sort();
        let expected =
            Fingerprint::digest(serde_json::to_vec(&expected).map_err(|error| self.error(error))?);
        let current = capture_analysis_input_with_filters(path.as_std_path(), &[filter])
            .map_err(|error| self.error(error))?;
        if !matches!(&current, AnalysisInputState::Directory { observations, .. }
            if observations.len() == 1 && observations[0].entries == expected)
        {
            return Err(self.error(format!("cached directory view no longer matches {}", path)));
        }
        self.record_state(path, current)
    }

    pub(crate) fn read_directory_filtered(
        &self,
        path: &SystemPath,
        filter: AnalysisDirectoryFilter,
    ) -> io::Result<Vec<DirectoryEntry>> {
        let entries = self
            .inner
            .read_directory(path)?
            .collect::<io::Result<Vec<_>>>()
            .map_err(|error| self.error(error))?;
        let entries: Vec<_> = entries
            .into_iter()
            .filter(|entry| {
                let kind = entry.file_type();
                filter.includes(
                    entry.path().file_name().unwrap_or_default(),
                    kind.is_file(),
                    kind.is_directory(),
                    kind.is_symlink(),
                )
            })
            .collect();
        self.record_view(
            path,
            filter,
            entries.iter().map(|entry| {
                (
                    entry.path().file_name().unwrap_or_default().to_owned(),
                    entry.file_type(),
                )
            }),
        )?;
        Ok(entries)
    }

    pub(crate) fn observe_path(&self, path: &SystemPath) -> Result<()> {
        self.record(path)?;
        Ok(())
    }

    pub(crate) fn snapshot(
        &self,
    ) -> Result<(Vec<AnalysisInput>, Vec<AnalysisEnvironmentVariable>)> {
        let observations = self
            .observations
            .lock()
            .expect("analysis input lock poisoned");
        if !observations.errors.is_empty() {
            bail!(
                "analysis inputs were not stable: {}",
                observations.errors.join("; ")
            );
        }
        let inputs = observations
            .paths
            .iter()
            .map(|(path, state)| AnalysisInput {
                path: path.clone(),
                state: state.clone(),
            })
            .collect::<Vec<_>>();
        let environment = observations
            .environment
            .iter()
            .map(|(name, value)| AnalysisEnvironmentVariable {
                name: name.clone(),
                value: *value,
            })
            .collect::<Vec<_>>();
        verify_analysis_inputs(&inputs)?;
        for variable in &environment {
            let current = std::env::var(&variable.name).ok().map(Fingerprint::digest);
            if current != variable.value {
                bail!("analysis environment changed: {}", variable.name);
            }
        }
        Ok((inputs, environment))
    }
}

impl System for AnalysisSystem {
    fn path_metadata(&self, path: &SystemPath) -> io::Result<Metadata> {
        self.record(path)?;
        let metadata = self.inner.path_metadata(path)?;
        self.record(path)?;
        Ok(metadata)
    }

    fn canonicalize_path(&self, path: &SystemPath) -> io::Result<SystemPathBuf> {
        self.record(path)?;
        let canonical = self.inner.canonicalize_path(path)?;
        self.record(path)?;
        Ok(canonical)
    }

    fn is_same_file(&self, first: &SystemPath, second: &SystemPath) -> io::Result<bool> {
        self.record(first)?;
        self.record(second)?;
        self.inner.is_same_file(first, second)
    }

    fn which(&self, _binary_name: &str) -> WhichResult {
        let _ = self.error("offline analysis requires the explicitly selected interpreter; PATH discovery is disabled");
        Err(WhichError::CannotFindBinaryPath)
    }

    fn read_to_string(&self, path: &SystemPath) -> io::Result<String> {
        self.record(path)?;
        let contents = self.inner.read_to_string(path)?;
        let state = self
            .observations
            .lock()
            .expect("analysis input lock poisoned")
            .paths
            .get(path.as_std_path())
            .cloned();
        if let Some(AnalysisInputState::File { digest, size, .. }) = state
            && (Fingerprint::digest(contents.as_bytes()) != digest || contents.len() as u64 != size)
        {
            return Err(self.error(format!("analysis source changed while reading {path}")));
        }
        self.record(path)?;
        Ok(contents)
    }

    fn read_to_notebook(&self, path: &SystemPath) -> Result<Notebook, NotebookError> {
        Notebook::from_source_code(&self.read_to_string(path)?)
    }

    fn read_virtual_path_to_string(&self, _path: &SystemVirtualPath) -> io::Result<String> {
        Err(self.error("offline analysis does not accept virtual source inputs"))
    }

    fn read_virtual_path_to_notebook(
        &self,
        path: &SystemVirtualPath,
    ) -> Result<Notebook, NotebookError> {
        Err(NotebookError::from(self.error(format!(
            "offline analysis does not accept virtual notebook {path}"
        ))))
    }

    fn current_directory(&self) -> &SystemPath {
        self.inner.current_directory()
    }

    fn user_config_directory(&self) -> Option<SystemPathBuf> {
        let _ = self.env_var("HOME");
        let _ = self.env_var("XDG_CONFIG_HOME");
        self.inner.user_config_directory()
    }

    fn cache_dir(&self) -> Option<SystemPathBuf> {
        None
    }

    fn read_directory<'a>(
        &'a self,
        path: &SystemPath,
    ) -> io::Result<Box<dyn Iterator<Item = io::Result<DirectoryEntry>> + 'a>> {
        Ok(Box::new(
            self.read_directory_filtered(path, AnalysisDirectoryFilter::All)?
                .into_iter()
                .map(Ok),
        ))
    }

    fn read_directory_for_import_resolution<'a>(
        &'a self,
        path: &SystemPath,
    ) -> io::Result<Box<dyn Iterator<Item = io::Result<DirectoryEntry>> + 'a>> {
        self.record(path)?;
        let entries = self
            .inner
            .read_directory(path)?
            .collect::<io::Result<Vec<_>>>()
            .map_err(|error| self.error(error))?;
        Ok(Box::new(entries.into_iter().map(Ok)))
    }

    fn observe_directory_query(
        &self,
        path: &SystemPath,
        filter: &DirectoryFilter,
        entries: &mut dyn Iterator<Item = (&str, FileType)>,
    ) {
        let filter = match filter {
            DirectoryFilter::All => AnalysisDirectoryFilter::All,
            DirectoryFilter::Name(name) => AnalysisDirectoryFilter::Name { name: name.clone() },
            DirectoryFilter::Prefix(prefix) => AnalysisDirectoryFilter::Prefix {
                prefix: prefix.clone(),
            },
            DirectoryFilter::Suffix(suffix) => AnalysisDirectoryFilter::Suffix {
                suffix: suffix.clone(),
            },
        };
        let _ = self.record_view(
            path,
            filter,
            entries.map(|(name, kind)| (name.to_owned(), kind)),
        );
    }

    fn walk_directory(&self, path: &SystemPath) -> WalkDirectoryBuilder {
        WalkDirectoryBuilder::new(
            path,
            ObservedFileWalker {
                system: self.clone(),
            },
        )
    }

    fn env_var(&self, name: &str) -> Result<String, std::env::VarError> {
        let value = self.inner.env_var(name);
        if matches!(&value, Err(std::env::VarError::NotUnicode(_))) {
            let _ = self.error(format!("analysis environment is not UTF-8: {name}"));
        }
        let digest = value.as_ref().ok().map(Fingerprint::digest);
        let mut observations = self
            .observations
            .lock()
            .expect("analysis input lock poisoned");
        if let Some(previous) = observations.environment.insert(name.into(), digest)
            && previous != digest
        {
            observations
                .errors
                .push(format!("analysis environment changed: {name}"));
        }
        value
    }

    fn as_writable(&self) -> Option<&dyn WritableSystem> {
        None
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn dyn_clone(&self) -> Box<dyn System> {
        Box::new(self.clone())
    }
}

/// ty lazily indexes its explicit file set through the walking interface. The
/// driver already did observed source discovery, so only those exact files are
/// allowed here. OS/gitignore-driven recursive discovery is never substituted.
struct ObservedFileWalker {
    system: AnalysisSystem,
}

impl DirectoryWalker for ObservedFileWalker {
    fn walk(
        &self,
        visitor: &mut dyn WalkDirectoryVisitorBuilder,
        configuration: WalkDirectoryConfiguration,
    ) {
        if configuration.standard_filters {
            let _ = self
                .system
                .error("offline source selection must disable unobserved OS ignore filters");
            return;
        }
        for path in &configuration.paths {
            if let Err(error) = self.system.record(path) {
                let _ = self.system.error(error);
                return;
            }
            if !self.system.inner.is_file(path) {
                let _ = self.system.error(format!(
                    "offline checker indexing requires an explicit source file: {path}"
                ));
                return;
            }
        }
        let Some((first, additional)) = configuration.paths.split_first() else {
            return;
        };
        let mut walker = self
            .system
            .inner
            .walk_directory(first)
            .standard_filters(false)
            .ignore_hidden(configuration.ignore_hidden);
        for path in additional {
            walker = walker.add(path);
        }
        walker.visit(visitor);
        for path in &configuration.paths {
            let _ = self.system.record(path);
        }
    }

    fn incremental_matcher(
        &self,
        _configuration: WalkDirectoryConfiguration,
    ) -> Box<dyn IgnoreIncremental> {
        let _ = self
            .system
            .error("incremental checker walks are unsupported by offline analysis");
        struct Unavailable;
        impl IgnoreIncremental for Unavailable {
            fn is_ignored(&mut self, _path: &SystemPath, _is_directory: bool) -> bool {
                true
            }
        }
        Box::new(Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_negative_resolution_cannot_hide_a_new_matching_entry() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = SystemPath::new(directory.path().to_str().unwrap());
        let system = AnalysisSystem::new(root);
        let cached = system
            .read_directory_for_import_resolution(root)?
            .collect::<io::Result<Vec<_>>>()?;
        assert!(cached.is_empty());
        system.observe_directory_query(
            root,
            &DirectoryFilter::Name("missing.pyi".into()),
            &mut std::iter::empty(),
        );
        std::fs::create_dir(directory.path().join("__pycache__"))?;
        std::fs::write(directory.path().join("deployment.json"), b"{}")?;
        system.snapshot()?;
        let candidate = directory.path().join("missing.pyi");
        std::fs::write(&candidate, b"VALUE: int\n")?;
        // ty may reuse its old listing, but its semantic name query must be
        // compared with the current filesystem before any facts are signed.
        system.observe_directory_query(
            root,
            &DirectoryFilter::Name("missing.pyi".into()),
            &mut std::iter::empty(),
        );
        std::fs::remove_file(candidate)?;
        assert!(
            system.snapshot().is_err(),
            "a swallowed query error must remain fatal even after restoration"
        );
        Ok(())
    }

    #[test]
    fn cached_positive_resolution_cannot_hide_a_removed_entry() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = SystemPath::new(directory.path().to_str().unwrap());
        let source = directory.path().join("model.py");
        std::fs::write(&source, b"pass\n")?;
        let system = AnalysisSystem::new(root);
        let cached = system
            .read_directory_for_import_resolution(root)?
            .collect::<io::Result<Vec<_>>>()?;
        std::fs::remove_file(&source)?;
        let mut selected = cached.iter().filter_map(|entry| {
            let name = entry.path().file_name()?;
            (name == "model.py").then_some((name, entry.file_type()))
        });
        system.observe_directory_query(
            root,
            &DirectoryFilter::Name("model.py".into()),
            &mut selected,
        );
        std::fs::write(source, b"pass\n")?;
        assert!(system.snapshot().is_err());
        Ok(())
    }
}
