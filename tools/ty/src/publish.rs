use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use soac_contracts::{
    AnalysisDirectoryFilter, AnalysisInput, AnalysisInputState, ArtifactExpectations,
    ArtifactSigningKey, EncodedModuleShard, StrictArtifactDeployment, TypeArtifactManifest,
    sign_manifest, verify_complete_generation, verify_manifest,
};

#[derive(Debug, Serialize)]
pub(crate) struct Publication {
    pub(crate) generation: String,
    pub(crate) artifact_directory: PathBuf,
    pub(crate) modules: usize,
    pub(crate) reused_shards: usize,
}

fn regular_file(path: &Path) -> Result<Vec<u8>> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "artifact file must not be a symlink: {}",
        path.display()
    );
    Ok(fs::read(path)?)
}

fn authority_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.file_type().is_file(),
                "startup authority must be a regular file or absent"
            );
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("inspect startup authority destination"),
    }
}

fn verify_existing(
    destination: &Path,
    signed: &[u8],
    manifest: &TypeArtifactManifest,
    signing_key: &ArtifactSigningKey,
) -> Result<()> {
    ensure!(
        fs::symlink_metadata(destination)?.file_type().is_dir(),
        "generation must be a real directory"
    );
    ensure!(
        fs::symlink_metadata(destination.join("modules"))?
            .file_type()
            .is_dir(),
        "module shard directory must not be a symlink"
    );
    ensure!(
        regular_file(&destination.join("manifest.json"))? == signed,
        "existing immutable generation has a different manifest"
    );
    let verified = verify_manifest(
        signed,
        &signing_key.trust_anchor(),
        &ArtifactExpectations {
            generation: manifest.generation,
            environment: manifest.environment.clone(),
        },
    )?;
    verify_complete_generation(verified, |digest| {
        regular_file(
            &destination
                .join("modules")
                .join(format!("{digest}.soac-types")),
        )
        .map_err(|error| soac_contracts::ContractError::InvalidStructure(error.to_string()))
    })?;
    Ok(())
}

/// Refuse authority/output locations that would overwrite an analyzed input or
/// change a directory view actually consumed by the checker. No placeholder
/// startup authority is ever written to reserve a name.
pub(crate) fn check_output_boundary(
    inputs: &[AnalysisInput],
    output: &Path,
    deployment: &Path,
) -> Result<()> {
    let parent = deployment.parent().context("deployment parent")?;
    let name = deployment
        .file_name()
        .and_then(|name| name.to_str())
        .context("deployment filename")?;
    let existing_regular = authority_exists(deployment)?;
    for input in inputs {
        ensure!(
            input.path != deployment,
            "startup authority would overwrite an analysis input"
        );
        if input.path.starts_with(output) {
            ensure!(
                matches!(&input.state, AnalysisInputState::Directory { observations, .. } if observations.is_empty()),
                "artifact output overlaps a consumed checker input: {}",
                input.path.display()
            );
        }
        if input.path == parent
            && let AnalysisInputState::Directory { observations, .. } = &input.state
        {
            ensure!(
                observations
                    .iter()
                    .all(|view| view.filter != AnalysisDirectoryFilter::All
                        && (existing_regular || !view.filter.includes(name, true, false, false))),
                "startup authority changes a consumed directory view; use a dedicated deployment directory"
            );
        }
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn publish_object(path: &Path, bytes: &[u8]) -> Result<()> {
    // A crashed writer must not leave a truncated content-addressed object
    // that poisons all later attempts to publish the same valid generation.
    let parent = path
        .parent()
        .context("object requires a parent directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(path)
        .context("publish complete content-addressed shard")?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub(crate) fn publish(
    root: &Path,
    manifest: &TypeArtifactManifest,
    shards: &[EncodedModuleShard],
    signing_key: &ArtifactSigningKey,
) -> Result<Publication> {
    fs::create_dir_all(root)?;
    let root = root.canonicalize()?;
    let generation = manifest.generation.fingerprint().to_hex();
    let destination = root.join(&generation);
    let signed = sign_manifest(manifest, signing_key)?;
    let verified = verify_manifest(
        &signed,
        &signing_key.trust_anchor(),
        &ArtifactExpectations {
            generation: manifest.generation,
            environment: manifest.environment.clone(),
        },
    )?;
    if destination.try_exists()? {
        verify_existing(&destination, &signed, manifest, signing_key)?;
        return Ok(Publication {
            generation,
            artifact_directory: destination,
            modules: shards.len(),
            reused_shards: shards.len(),
        });
    }
    let objects = root.join("objects");
    fs::create_dir_all(&objects)?;
    ensure!(
        fs::symlink_metadata(&objects)?.file_type().is_dir(),
        "content-addressed object store must not be a symlink"
    );
    let stage = tempfile::Builder::new()
        .prefix(".generation-")
        .tempdir_in(&root)?;
    let module_dir = stage.path().join("modules");
    fs::create_dir(&module_dir)?;
    let mut reused_shards = 0;
    for shard in shards {
        let object = objects.join(shard.file_name());
        if object.exists() {
            if regular_file(&object)? != shard.bytes() {
                bail!("changed content-addressed shard: {}", object.display());
            }
            reused_shards += 1;
        } else {
            // The object is not published as a generation until all shards and
            // the signed manifest have passed complete-generation verification.
            match publish_object(&object, shard.bytes()) {
                Ok(()) => {}
                Err(error) if object.exists() => {
                    if regular_file(&object)? != shard.bytes() {
                        return Err(error);
                    }
                    reused_shards += 1;
                }
                Err(error) => return Err(error),
            }
        }
        let target = module_dir.join(shard.file_name());
        if fs::hard_link(&object, &target).is_err() {
            write_new(&target, shard.bytes())?;
        }
    }
    write_new(&stage.path().join("manifest.json"), &signed)?;
    verify_complete_generation(verified, |digest| {
        regular_file(&module_dir.join(format!("{digest}.soac-types")))
            .map_err(|error| soac_contracts::ContractError::InvalidStructure(error.to_string()))
    })?;
    File::open(&module_dir)?.sync_all()?;
    File::open(stage.path())?.sync_all()?;
    // rename(2) alone replaces an existing empty directory. Use no-replace
    // semantics so a partial generation is never silently repaired by a race.
    #[cfg(target_os = "linux")]
    let renamed = rustix::fs::renameat_with(
        rustix::fs::CWD,
        stage.path(),
        rustix::fs::CWD,
        &destination,
        rustix::fs::RenameFlags::NOREPLACE,
    );
    #[cfg(not(target_os = "linux"))]
    let renamed: Result<(), std::io::Error> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic generation publication currently requires Linux",
    ));
    if let Err(error) = renamed {
        if !destination.try_exists()? {
            return Err(error).context("publish complete immutable type generation");
        }
        verify_existing(&destination, &signed, manifest, signing_key)?;
        reused_shards = shards.len();
    }
    File::open(&root)?.sync_all()?;
    Ok(Publication {
        generation,
        artifact_directory: destination,
        modules: shards.len(),
        reused_shards,
    })
}

fn write_deployment_json(writer: &mut impl Write, deployment: &impl Serialize) -> Result<()> {
    // Serde emits small fragments. Buffer them before crossing a filesystem
    // boundary, and never let BufWriter::drop hide a delayed write failure.
    let mut buffered = BufWriter::new(writer);
    serde_json::to_writer(&mut buffered, deployment)
        .context("serialize trusted startup descriptor")?;
    buffered
        .write_all(b"\n")
        .context("finish trusted startup descriptor")?;
    buffered.flush().context("flush trusted startup descriptor")
}

pub(crate) fn write_deployment(path: &Path, deployment: &StrictArtifactDeployment) -> Result<()> {
    deployment.validate()?;
    let parent = path
        .parent()
        .context("deployment file requires a parent directory")?;
    fs::create_dir_all(parent)?;
    let parent = parent.canonicalize()?;
    let path = parent.join(path.file_name().context("deployment filename")?);
    authority_exists(&path)?;
    let artifact_root = deployment
        .artifact_directory
        .parent()
        .context("artifact generation requires its content store root")?;
    if parent.starts_with(artifact_root) {
        bail!("startup authority must be outside the writable artifact root");
    }
    let mut temporary = tempfile::NamedTempFile::new_in(&parent)?;
    write_deployment_json(&mut temporary, deployment)?;
    temporary.as_file().sync_all()?;
    soac_contracts::verify_analysis_inputs(&deployment.analysis_inputs)?;
    temporary
        .persist(&path)
        .context("publish trusted startup descriptor")?;
    File::open(&parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_contracts::{
        ArtifactEnvironment, ConservativeAnalysis, Fingerprint, ModuleArtifactIndex,
        ModuleTypeFacts, PythonVersion, ResolvedStrictPolicy, SourceDialect, encode_module_shard,
    };

    fn fixture(
        source: &[u8],
    ) -> Result<(
        TypeArtifactManifest,
        Vec<EncodedModuleShard>,
        ArtifactSigningKey,
    )> {
        let fingerprint = Fingerprint::digest(b"publication fixture");
        let environment = ArtifactEnvironment {
            ty_revision: "fixture".into(),
            checker_source_fingerprint: fingerprint,
            exporter_revision: "fixture".into(),
            python_version: PythonVersion {
                major: 3,
                minor: 15,
            },
            python_platform: "linux".into(),
            cpython_abi_fingerprint: fingerprint,
            normalized_project_policy: fingerprint,
            resolved_typechecker_configuration: fingerprint,
            import_search_path: fingerprint,
            typeshed_fingerprint: fingerprint,
            installed_stub_fingerprint: fingerprint,
            installed_dependency_fingerprint: fingerprint,
            analysis: ConservativeAnalysis::default(),
        };
        let facts = ModuleTypeFacts::new(
            "example",
            source,
            SourceDialect::SoacStrict,
            ResolvedStrictPolicy::default(),
        )?;
        let shard = encode_module_shard(&facts)?;
        let manifest =
            TypeArtifactManifest::new(environment, vec![ModuleArtifactIndex::from_shard(&shard)?])?;
        Ok((
            manifest,
            vec![shard],
            ArtifactSigningKey::from_bytes(&[17; 32]),
        ))
    }

    #[test]
    fn buffered_deployment_serialization_preserves_exact_bytes() -> Result<()> {
        #[derive(Default)]
        struct ObservedWriter {
            bytes: Vec<u8>,
            writes: usize,
            flushes: usize,
        }
        impl Write for ObservedWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.writes += 1;
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.flushes += 1;
                Ok(())
            }
        }

        let payload = vec!["dependency configuration"; 10_000];
        let mut expected = serde_json::to_vec(&payload)?;
        expected.push(b'\n');
        let mut writer = ObservedWriter::default();
        write_deployment_json(&mut writer, &payload)?;
        assert_eq!(writer.bytes, expected);
        assert!(
            writer.writes < payload.len(),
            "{} serializer fragments became individual file writes for {} values",
            writer.writes,
            payload.len()
        );
        assert_eq!(
            writer.flushes, 1,
            "flush must finish before file sync/persist"
        );
        eprintln!(
            "deployment serialization: {} identical bytes, {} underlying writes",
            writer.bytes.len(),
            writer.writes
        );
        Ok(())
    }

    #[test]
    fn buffered_deployment_flush_failure_prevents_publication() -> Result<()> {
        struct FailingFlush<'a>(&'a mut tempfile::NamedTempFile);
        impl Write for FailingFlush<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.write(bytes)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("injected descriptor flush failure"))
            }
        }

        for previous in [None, Some(b"previous trusted descriptor".as_slice())] {
            let directory = tempfile::tempdir()?;
            let destination = directory.path().join("deployment.json");
            if let Some(previous) = previous {
                fs::write(&destination, previous)?;
            }
            let mut temporary = tempfile::NamedTempFile::new_in(directory.path())?;
            let temporary_path = temporary.path().to_owned();
            // The same write/flush-before-sync/persist ordering as write_deployment.
            let result = write_deployment_json(&mut FailingFlush(&mut temporary), &vec![1; 100])
                .and_then(|()| {
                    temporary.as_file().sync_all()?;
                    temporary.persist(&destination)?;
                    Ok(())
                });
            let error = result.unwrap_err();
            assert_eq!(
                error.root_cause().to_string(),
                "injected descriptor flush failure"
            );
            assert_eq!(fs::read(&destination).ok().as_deref(), previous);
            assert!(
                !temporary_path.exists(),
                "failed private staging file is removed"
            );
        }
        Ok(())
    }

    #[test]
    fn complete_generation_is_idempotent_and_reuses_unchanged_shards() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let (manifest, shards, key) = fixture(b"from __future__ import strict\nvalue = 1\n")?;
        let first = publish(directory.path(), &manifest, &shards, &key)?;
        assert_eq!(first.reused_shards, 0);
        let again = publish(directory.path(), &manifest, &shards, &key)?;
        assert_eq!(again.generation, first.generation);
        assert_eq!(again.reused_shards, 1);
        let mut updated_environment = manifest.environment.clone();
        updated_environment.installed_dependency_fingerprint =
            Fingerprint::digest(b"changed dependency");
        let updated_manifest =
            TypeArtifactManifest::new(updated_environment, manifest.modules.clone())?;
        let changed = publish(directory.path(), &updated_manifest, &shards, &key)?;
        assert_ne!(changed.generation, first.generation);
        assert_eq!(changed.reused_shards, 1);
        Ok(())
    }

    #[test]
    fn missing_or_tampered_published_shards_are_never_silently_repaired() -> Result<()> {
        for replacement in [None, Some(b"tampered".as_slice())] {
            let directory = tempfile::tempdir()?;
            let (manifest, shards, key) = fixture(b"from __future__ import strict\n")?;
            let publication = publish(directory.path(), &manifest, &shards, &key)?;
            let module = publication
                .artifact_directory
                .join("modules")
                .join(shards[0].file_name());
            if let Some(bytes) = replacement {
                fs::write(module, bytes)?;
            } else {
                fs::remove_file(module)?;
            }
            assert!(publish(directory.path(), &manifest, &shards, &key).is_err());
        }
        Ok(())
    }

    #[test]
    fn changed_object_and_partial_generation_fail_before_publication() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let (manifest, shards, key) = fixture(b"from __future__ import strict\n")?;
        let objects = directory.path().join("objects");
        fs::create_dir(&objects)?;
        fs::write(objects.join(shards[0].file_name()), b"incomplete object")?;
        let destination = directory
            .path()
            .join(manifest.generation.fingerprint().to_hex());
        assert!(publish(directory.path(), &manifest, &shards, &key).is_err());
        assert!(!destination.exists());
        fs::create_dir(&destination)?;
        assert!(publish(directory.path(), &manifest, &shards, &key).is_err());
        Ok(())
    }

    #[test]
    fn concurrent_publishers_only_expose_one_complete_generation() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let (manifest, shards, key) = fixture(b"from __future__ import strict\n")?;
        let barrier = std::sync::Barrier::new(4);
        let results = std::thread::scope(|scope| {
            let workers = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        publish(directory.path(), &manifest, &shards, &key)
                    })
                })
                .collect::<Vec<_>>();
            workers
                .into_iter()
                .map(|worker| worker.join().expect("publisher panicked"))
                .collect::<Vec<_>>()
        });
        let mut generation = None;
        for result in results {
            let published = result?;
            if let Some(expected) = &generation {
                assert_eq!(&published.generation, expected);
            }
            generation = Some(published.generation);
            verify_existing(
                &published.artifact_directory,
                &sign_manifest(&manifest, &key)?,
                &manifest,
                &key,
            )?;
        }
        assert_eq!(
            fs::read_dir(directory.path())?.count(),
            2,
            "no incomplete staging directories remain"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_objects_and_published_shards_are_rejected() -> Result<()> {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let (manifest, shards, key) = fixture(b"from __future__ import strict\n")?;
        symlink(outside.path(), directory.path().join("objects"))?;
        assert!(publish(directory.path(), &manifest, &shards, &key).is_err());
        assert_eq!(fs::read_dir(outside.path())?.count(), 0);
        fs::remove_file(directory.path().join("objects"))?;
        let published = publish(directory.path(), &manifest, &shards, &key)?;
        let shard_path = published
            .artifact_directory
            .join("modules")
            .join(shards[0].file_name());
        fs::remove_file(&shard_path)?;
        let target = outside.path().join("same-bytes");
        fs::write(&target, shards[0].bytes())?;
        symlink(target, shard_path)?;
        assert!(publish(directory.path(), &manifest, &shards, &key).is_err());
        Ok(())
    }

    #[test]
    fn authority_boundary_checks_exact_consumed_directory_views() -> Result<()> {
        use soac_contracts::capture_analysis_input_with_filters;

        let directory = tempfile::tempdir()?;
        let deployment = directory.path().join("deployment.json");
        let output = directory.path().join("artifacts");
        fs::create_dir(&output)?;
        for (filter, permitted) in [
            (AnalysisDirectoryFilter::All, false),
            (
                AnalysisDirectoryFilter::Name {
                    name: "deployment.json".into(),
                },
                false,
            ),
            (
                AnalysisDirectoryFilter::Prefix {
                    prefix: "model".into(),
                },
                true,
            ),
            (
                AnalysisDirectoryFilter::SourceSelection {
                    excluded_names: vec!["artifacts".into()],
                },
                true,
            ),
        ] {
            let input = AnalysisInput {
                path: directory.path().to_owned(),
                state: capture_analysis_input_with_filters(directory.path(), &[filter])?,
            };
            assert_eq!(
                check_output_boundary(&[input], &output, &deployment).is_ok(),
                permitted
            );
            assert!(!deployment.exists(), "no placeholder authority was created");
        }
        #[cfg(unix)]
        {
            // Source-selection views include symlinks but ignore ordinary JSON
            // files. Replacing this symlink with a regular descriptor would
            // otherwise immediately invalidate the just-published snapshot.
            fs::write(directory.path().join("previous.json"), b"{}")?;
            std::os::unix::fs::symlink("previous.json", &deployment)?;
            let input = AnalysisInput {
                path: directory.path().to_owned(),
                state: capture_analysis_input_with_filters(
                    directory.path(),
                    &[AnalysisDirectoryFilter::SourceSelection {
                        excluded_names: vec!["artifacts".into()],
                    }],
                )?,
            };
            assert!(check_output_boundary(&[input], &output, &deployment).is_err());
            assert!(fs::symlink_metadata(&deployment)?.file_type().is_symlink());
        }
        Ok(())
    }
}
