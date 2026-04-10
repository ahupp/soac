use crate::block_py::{BlockPyFunction, BlockPyModule, FunctionNameGen, ModuleNameGen};
use crate::passes::CodegenModuleShape;
use anyhow::{anyhow, Context, Result};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

const CODEGEN_MODULE_CACHE_MAGIC: &[u8] = b"SOAC_BLOCKPY_CODEGEN_CACHE\0";
const CODEGEN_MODULE_CACHE_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonModuleCacheSource {
    Project,
    PythonStdlib,
}

impl PythonModuleCacheSource {
    const fn subtree(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::PythonStdlib => "python-stdlib",
        }
    }
}

pub fn codegen_module_cache_path(
    cache_root: impl AsRef<Path>,
    source: PythonModuleCacheSource,
    cache_key: &str,
) -> Result<PathBuf> {
    let file_stem = cache_file_stem(cache_key)?;
    Ok(cache_root
        .as_ref()
        .join(source.subtree())
        .join("blockpy-codegen")
        .join(format!("{file_stem}.blockpy.rkyv")))
}

pub fn store_codegen_module_cache(
    path: impl AsRef<Path>,
    module: &BlockPyModule<CodegenModuleShape>,
) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = non_empty_parent(path) {
        fs::create_dir_all(parent)
            .with_context(|| format!("create BlockPy cache dir {}", parent.display()))?;
    }

    let archive = rkyv::to_bytes::<rkyv::rancor::Error>(module)
        .map_err(|err| anyhow!("serialize BlockPy codegen module cache: {err}"))?;
    let temp_path = temp_cache_path(path);

    {
        let mut temp_file = File::create(&temp_path)
            .with_context(|| format!("create temporary BlockPy cache {}", temp_path.display()))?;
        temp_file
            .write_all(CODEGEN_MODULE_CACHE_MAGIC)
            .with_context(|| format!("write BlockPy cache header {}", temp_path.display()))?;
        temp_file
            .write_all(&CODEGEN_MODULE_CACHE_FORMAT_VERSION.to_le_bytes())
            .with_context(|| format!("write BlockPy cache version {}", temp_path.display()))?;
        temp_file
            .write_all(archive.as_ref())
            .with_context(|| format!("write BlockPy cache archive {}", temp_path.display()))?;
    }

    fs::rename(&temp_path, path).with_context(|| {
        format!(
            "publish BlockPy cache {} -> {}",
            temp_path.display(),
            path.display()
        )
    })?;
    Ok(())
}

pub fn load_codegen_module_cache(
    path: impl AsRef<Path>,
) -> Result<BlockPyModule<CodegenModuleShape>> {
    let path = path.as_ref();
    let bytes = fs::read(path).with_context(|| format!("read BlockPy cache {}", path.display()))?;
    let archive = archive_bytes_from_cache_file(&bytes)
        .with_context(|| format!("decode BlockPy cache header {}", path.display()))?;
    let archive = aligned_archive_bytes(archive);

    let mut module = rkyv::from_bytes::<BlockPyModule<CodegenModuleShape>, rkyv::rancor::Error>(
        archive.as_ref(),
    )
    .map_err(|err| anyhow!("deserialize BlockPy codegen module cache: {err}"))?;

    rehydrate_codegen_module_generators(&mut module);
    Ok(module)
}

pub fn rehydrate_codegen_module_generators(module: &mut BlockPyModule<CodegenModuleShape>) {
    module.module_name_gen = recovered_module_name_gen(module);
    for function in &mut module.callable_defs {
        function.name_gen = recovered_function_name_gen(function);
    }
}

fn archive_bytes_from_cache_file(bytes: &[u8]) -> Result<&[u8]> {
    let rest = bytes
        .strip_prefix(CODEGEN_MODULE_CACHE_MAGIC)
        .ok_or_else(|| anyhow!("invalid BlockPy codegen cache magic"))?;
    let (version_bytes, archive) = rest
        .split_at_checked(std::mem::size_of::<u32>())
        .ok_or_else(|| anyhow!("BlockPy codegen cache is truncated"))?;
    let version = u32::from_le_bytes(
        version_bytes
            .try_into()
            .expect("split_at_checked returned exactly four version bytes"),
    );
    if version != CODEGEN_MODULE_CACHE_FORMAT_VERSION {
        return Err(anyhow!(
            "unsupported BlockPy codegen cache version {version}; expected {CODEGEN_MODULE_CACHE_FORMAT_VERSION}"
        ));
    }
    Ok(archive)
}

fn aligned_archive_bytes(bytes: &[u8]) -> rkyv::util::AlignedVec {
    let mut aligned = rkyv::util::AlignedVec::with_capacity(bytes.len());
    aligned.extend_from_slice(bytes);
    aligned
}

fn cache_file_stem(cache_key: &str) -> Result<&str> {
    let valid = !cache_key.is_empty()
        && cache_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if valid {
        Ok(cache_key)
    } else {
        Err(anyhow!(
            "BlockPy cache key must be a non-empty ASCII alnum/_/- file stem: {cache_key:?}"
        ))
    }
}

fn recovered_module_name_gen(module: &BlockPyModule<CodegenModuleShape>) -> ModuleNameGen {
    let module_id = module
        .callable_defs
        .first()
        .map(|function| function.function_id.module_id())
        .unwrap_or(0);
    let next_function_id = module
        .callable_defs
        .iter()
        .filter(|function| function.function_id.module_id() == module_id)
        .map(|function| function.function_id.function_id().saturating_add(1))
        .max()
        .unwrap_or(1)
        .max(1);
    ModuleNameGen::recovered(module_id, next_function_id)
}

fn recovered_function_name_gen(function: &BlockPyFunction<CodegenModuleShape>) -> FunctionNameGen {
    let next_block_id = function
        .blocks
        .iter()
        .filter_map(|block| {
            if block.label.is_fallthrough() {
                None
            } else {
                Some(block.label.as_u32().saturating_add(1))
            }
        })
        .max()
        .unwrap_or(0);
    FunctionNameGen::recovered(function.function_id, next_block_id, 0)
}

fn non_empty_parent(path: &Path) -> Option<&Path> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
}

fn temp_cache_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "blockpy-codegen-cache".into());
    let temp_file_name = format!("{file_name}.tmp.{}", std::process::id());

    let mut temp_path = path.to_owned();
    temp_path.set_file_name(temp_file_name);
    temp_path
}

#[cfg(test)]
mod test {
    use super::{
        codegen_module_cache_path, load_codegen_module_cache, store_codegen_module_cache,
        PythonModuleCacheSource,
    };
    use crate::block_py::{
        walk_block, walk_expr, BlockPyModule, ChildVisitable, FunctionId, HasSemanticInstrId,
        InstrCodegen, Visit,
    };
    use crate::lower_python_to_blockpy_for_testing;
    use crate::passes::CodegenModuleShape;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Debug, PartialEq)]
    struct ModuleSummary {
        global_names: Vec<String>,
        module_constants_len: usize,
        counter_defs_len: usize,
        functions: Vec<FunctionSummary>,
    }

    #[derive(Debug, PartialEq)]
    struct FunctionSummary {
        function_id: FunctionId,
        name_gen_function_id: FunctionId,
        qualname: String,
        block_labels: Vec<u32>,
        block_body_lens: Vec<usize>,
        instr_ids: Vec<(u32, u32)>,
    }

    struct InstrIdCollector {
        instr_ids: Vec<(u32, u32)>,
    }

    impl Visit<InstrCodegen> for InstrIdCollector {
        fn visit_instr(&mut self, expr: &InstrCodegen)
        where
            InstrCodegen: ChildVisitable<InstrCodegen>,
        {
            let instr_id = expr.semantic_instr_id();
            self.instr_ids.push((
                instr_id.block_label().as_u32(),
                instr_id.instr_index_in_block(),
            ));
            walk_expr(self, expr);
        }
    }

    #[test]
    fn round_trips_codegen_module_cache_without_rendering() {
        let module = lower_python_to_blockpy_for_testing(
            r#"
def f(x):
    if x:
        return g(x + 1)
    return g(0)

def g(y):
    return y
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let before = summarize_module(&module);

        let path = unique_cache_path();
        let _ = std::fs::remove_file(&path);
        store_codegen_module_cache(&path, &module).expect("store codegen cache");

        let loaded = load_codegen_module_cache(&path).expect("load codegen cache");
        let _ = std::fs::remove_file(&path);

        assert_eq!(summarize_module(&loaded), before);

        let max_function_id = loaded
            .callable_defs
            .iter()
            .map(|function| function.function_id.function_id())
            .max()
            .expect("test module should have callable defs");
        let recovered_next_function = loaded.module_name_gen.next_function_name_gen();
        assert_eq!(
            recovered_next_function.function_id().function_id(),
            max_function_id + 1
        );

        for function in &loaded.callable_defs {
            assert_eq!(function.name_gen.function_id(), function.function_id);
        }
        let first_function = &loaded.callable_defs[0];
        let next_block_label = first_function.name_gen.next_block_name();
        assert!(
            !first_function
                .blocks
                .iter()
                .any(|block| block.label == next_block_label),
            "rehydrated function generator should not reissue an existing block label"
        );
    }

    #[test]
    fn cache_path_keeps_python_stdlib_in_a_separate_subtree() {
        let root = PathBuf::from("/cache/root");

        assert_eq!(
            codegen_module_cache_path(&root, PythonModuleCacheSource::Project, "abc_123-def")
                .expect("project cache path"),
            PathBuf::from("/cache/root/project/blockpy-codegen/abc_123-def.blockpy.rkyv")
        );
        assert_eq!(
            codegen_module_cache_path(&root, PythonModuleCacheSource::PythonStdlib, "abc_123-def")
                .expect("stdlib cache path"),
            PathBuf::from("/cache/root/python-stdlib/blockpy-codegen/abc_123-def.blockpy.rkyv")
        );

        assert!(codegen_module_cache_path(
            &root,
            PythonModuleCacheSource::PythonStdlib,
            "../escape"
        )
        .is_err());
    }

    fn summarize_module(module: &BlockPyModule<CodegenModuleShape>) -> ModuleSummary {
        ModuleSummary {
            global_names: module.global_names.clone(),
            module_constants_len: module.module_constants.len(),
            counter_defs_len: module.counter_defs.len(),
            functions: module
                .callable_defs
                .iter()
                .map(summarize_function)
                .collect(),
        }
    }

    fn summarize_function(
        function: &crate::block_py::BlockPyFunction<CodegenModuleShape>,
    ) -> FunctionSummary {
        FunctionSummary {
            function_id: function.function_id,
            name_gen_function_id: function.name_gen.function_id(),
            qualname: function.names.qualname.clone(),
            block_labels: function
                .blocks
                .iter()
                .map(|block| block.label.as_u32())
                .collect(),
            block_body_lens: function
                .blocks
                .iter()
                .map(|block| block.body.len())
                .collect(),
            instr_ids: instr_ids(function),
        }
    }

    fn instr_ids(
        function: &crate::block_py::BlockPyFunction<CodegenModuleShape>,
    ) -> Vec<(u32, u32)> {
        let mut collector = InstrIdCollector {
            instr_ids: Vec::new(),
        };
        for block in &function.blocks {
            walk_block(&mut collector, block);
        }
        collector.instr_ids
    }

    fn unique_cache_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "soac-blockpy-codegen-cache-test-{}-{unique}.rkyv",
            std::process::id()
        ))
    }
}
