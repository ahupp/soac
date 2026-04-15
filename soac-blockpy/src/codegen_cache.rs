use crate::block_py::{
    walk_expr_mut, walk_module_mut, BlockPyFunction, BlockPyModule, CounterSite, FunctionId,
    FunctionNameGen, ModuleNameGen, VisitMut,
};
use crate::passes::{
    CodegenModuleShape, FactStore, InstrCodegen, LocalEnvModulePlan, LocalEnvResumeModulePlan,
    RefcountPlan,
};
use anyhow::{anyhow, Context, Result};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

const CODEGEN_MODULE_CACHE_MAGIC: &[u8] = b"SOAC_BLOCKPY_CODEGEN_CACHE\0";
const CODEGEN_MODULE_CACHE_FORMAT_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonModuleCacheSource {
    Project,
    PythonStdlib,
}

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedPreparedCodegen {
    pub value_facts: FactStore,
    pub ownership_plan: RefcountPlan,
    pub local_env_plan: LocalEnvModulePlan,
    pub local_env_resume_plan: LocalEnvResumeModulePlan,
}

impl CachedPreparedCodegen {
    pub fn remap_function_ids(&mut self, remap: impl Fn(FunctionId) -> FunctionId + Copy) {
        self.value_facts.remap_function_ids(remap);
        self.ownership_plan.remap_function_ids(remap);
        self.local_env_plan.remap_function_ids(remap);
        self.local_env_resume_plan.remap_function_ids(remap);
    }
}

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedCodegenModule {
    pub module: BlockPyModule<CodegenModuleShape>,
    pub prepared: Option<CachedPreparedCodegen>,
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

pub fn codegen_module_cache_key(source_hash: u64, build_identity: &str) -> String {
    format!("{source_hash:016x}-{:016x}", stable_hash(build_identity))
}

pub fn store_codegen_module_cache(
    path: impl AsRef<Path>,
    module: &BlockPyModule<CodegenModuleShape>,
    prepared: Option<&CachedPreparedCodegen>,
) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = non_empty_parent(path) {
        fs::create_dir_all(parent)
            .with_context(|| format!("create BlockPy cache dir {}", parent.display()))?;
    }

    let cache = CachedCodegenModule {
        module: module.clone(),
        prepared: prepared.cloned(),
    };
    let archive = rkyv::to_bytes::<rkyv::rancor::Error>(&cache)
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

pub fn load_codegen_module_cache(path: impl AsRef<Path>) -> Result<CachedCodegenModule> {
    let path = path.as_ref();
    let bytes = fs::read(path).with_context(|| format!("read BlockPy cache {}", path.display()))?;
    let archive = archive_bytes_from_cache_file(&bytes)
        .with_context(|| format!("decode BlockPy cache header {}", path.display()))?;
    let archive = aligned_archive_bytes(archive);

    let mut cache = rkyv::from_bytes::<CachedCodegenModule, rkyv::rancor::Error>(archive.as_ref())
        .map_err(|err| anyhow!("deserialize BlockPy codegen module cache: {err}"))?;

    rehydrate_codegen_module_generators(&mut cache.module);
    Ok(cache)
}

pub fn rehydrate_codegen_module_generators(module: &mut BlockPyModule<CodegenModuleShape>) {
    module.module_name_gen = recovered_module_name_gen(module);
    for function in &mut module.callable_defs {
        function.name_gen = recovered_function_name_gen(function);
    }
}

pub fn remap_codegen_module_function_ids(
    module: &mut BlockPyModule<CodegenModuleShape>,
    module_name_gen: ModuleNameGen,
) {
    remap_codegen_module_function_ids_with_remapper(
        module,
        FunctionIdRemapper {
            new_module_id: module_name_gen.module_id(),
        },
    );
}

pub fn remap_cached_codegen_module_function_ids(
    cache: &mut CachedCodegenModule,
    module_name_gen: ModuleNameGen,
) {
    let remapper = FunctionIdRemapper {
        new_module_id: module_name_gen.module_id(),
    };
    remap_codegen_module_function_ids_with_remapper(&mut cache.module, remapper);
    if let Some(prepared) = &mut cache.prepared {
        prepared.remap_function_ids(|function_id| remapper.remap(function_id));
    }
}

fn remap_codegen_module_function_ids_with_remapper(
    module: &mut BlockPyModule<CodegenModuleShape>,
    mut remapper: FunctionIdRemapper,
) {
    walk_module_mut(&mut remapper, module);

    for function in &mut module.callable_defs {
        function.function_id = remapper.remap(function.function_id);
        function.name_gen = recovered_function_name_gen(function);
    }
    for counter in &mut module.counter_defs {
        match &mut counter.site {
            CounterSite::BlockEntry { function_id, .. } => {
                *function_id = remapper.remap(*function_id);
            }
            CounterSite::DeoptEntry { function_id, .. } => {
                *function_id = remapper.remap(*function_id);
            }
            CounterSite::Runtime { function_id, .. } => {
                if let Some(function_id) = function_id {
                    *function_id = remapper.remap(*function_id);
                }
            }
        }
    }
    module.module_name_gen = recovered_module_name_gen(module);
}

#[derive(Clone, Copy)]
struct FunctionIdRemapper {
    new_module_id: u32,
}

impl FunctionIdRemapper {
    fn remap(&self, function_id: FunctionId) -> FunctionId {
        if function_id == FunctionId::global() {
            function_id
        } else {
            FunctionId::new(self.new_module_id, function_id.function_id())
        }
    }
}

impl VisitMut<InstrCodegen> for FunctionIdRemapper {
    fn visit_instr_mut(&mut self, expr: &mut InstrCodegen)
    where
        InstrCodegen: crate::block_py::ChildVisitable<InstrCodegen>,
    {
        match expr {
            InstrCodegen::CallDirect(op) => {
                op.function_id = self.remap(op.function_id);
            }
            InstrCodegen::MakeFunctionWithClosure(op) => {
                op.set_function_id(self.remap(op.function_id()));
            }
            InstrCodegen::BinOp(_)
            | InstrCodegen::UnaryOp(_)
            | InstrCodegen::Tuple(_)
            | InstrCodegen::CalleeFunctionId(_)
            | InstrCodegen::Call(_)
            | InstrCodegen::GetAttr(_)
            | InstrCodegen::SetAttr(_)
            | InstrCodegen::GetItem(_)
            | InstrCodegen::SetItem(_)
            | InstrCodegen::DelItem(_)
            | InstrCodegen::Load(_)
            | InstrCodegen::Store(_)
            | InstrCodegen::Del(_)
            | InstrCodegen::MakeCell(_)
            | InstrCodegen::IncrementCounter(_)
            | InstrCodegen::CellRef(_) => {}
        }
        walk_expr_mut(self, expr);
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

fn stable_hash(text: &str) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
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
        codegen_module_cache_key, codegen_module_cache_path, load_codegen_module_cache,
        remap_cached_codegen_module_function_ids, remap_codegen_module_function_ids,
        store_codegen_module_cache, CachedPreparedCodegen, PythonModuleCacheSource,
    };
    use crate::block_py::{
        walk_block, walk_expr, BlockPyModule, ChildVisitable, FunctionId, HasSemanticInstrId,
        InstrCodegen, ModuleNameGen, Visit,
    };
    use crate::lower_python_to_blockpy_for_testing;
    use crate::passes::{self, CodegenModuleShape};
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

    fn collect_make_function_with_closure_ids(
        module: &BlockPyModule<CodegenModuleShape>,
    ) -> Vec<FunctionId> {
        struct Collector {
            function_ids: Vec<FunctionId>,
        }

        impl Visit<InstrCodegen> for Collector {
            fn visit_instr(&mut self, expr: &InstrCodegen)
            where
                InstrCodegen: ChildVisitable<InstrCodegen>,
            {
                if let InstrCodegen::MakeFunctionWithClosure(op) = expr {
                    self.function_ids.push(op.function_id());
                }
                walk_expr(self, expr);
            }
        }

        let mut collector = Collector {
            function_ids: Vec::new(),
        };
        for function in &module.callable_defs {
            for block in &function.blocks {
                walk_block(&mut collector, block);
            }
        }
        collector.function_ids
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
        store_codegen_module_cache(&path, &module, None).expect("store codegen cache");

        let loaded = load_codegen_module_cache(&path)
            .expect("load codegen cache")
            .module;
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

    #[test]
    fn cache_key_combines_source_hash_and_build_identity_hash() {
        assert_eq!(
            codegen_module_cache_key(0x1234, "build-a"),
            "0000000000001234-09e267510d26cc71"
        );
        assert_ne!(
            codegen_module_cache_key(0x1234, "build-a"),
            codegen_module_cache_key(0x1234, "build-b")
        );
        assert_ne!(
            codegen_module_cache_key(0x1234, "build-a"),
            codegen_module_cache_key(0x5678, "build-a")
        );
    }

    #[test]
    fn remaps_cached_codegen_module_to_fresh_module_id() {
        let mut module = lower_python_to_blockpy_for_testing(
            r#"
def outer():
    def inner():
        return 1
    return inner()
"#,
        )
        .expect("transform should succeed")
        .codegen_module;

        remap_codegen_module_function_ids(&mut module, ModuleNameGen::new(99));

        assert_eq!(module.module_name_gen.module_id(), 99);
        for function in &module.callable_defs {
            assert_eq!(function.function_id.module_id(), 99);
            assert_eq!(function.name_gen.function_id(), function.function_id);
        }
        let make_function_ids = collect_make_function_with_closure_ids(&module);
        assert!(
            !make_function_ids.is_empty(),
            "test module should contain MakeFunctionWithClosure"
        );
        for function_id in make_function_ids {
            assert_eq!(
                function_id.module_id(),
                99,
                "cached MakeFunctionWithClosure ids must point at the remapped module id"
            );
        }

        let recovered_next_function = module.module_name_gen.next_function_name_gen();
        assert!(
            !module
                .callable_defs
                .iter()
                .any(|function| function.function_id == recovered_next_function.function_id()),
            "rehydrated module generator should not reissue an existing function id"
        );
    }

    #[test]
    fn round_trips_prepared_codegen_cache_without_rendering() {
        let module = lower_python_to_blockpy_for_testing(
            r#"
def outer(value):
    def inner():
        return value
    return inner()
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let prepared = prepared_codegen_for_module(&module);
        let value_fact_count = prepared.value_facts.expr_facts().count();
        let ownership_function_count = prepared.ownership_plan.functions.len();
        let local_env_function_count = prepared.local_env_plan.functions.len();
        let resume_function_count = prepared.local_env_resume_plan.functions.len();

        let path = unique_cache_path();
        let _ = std::fs::remove_file(&path);
        store_codegen_module_cache(&path, &module, Some(&prepared)).expect("store codegen cache");

        let loaded = load_codegen_module_cache(&path).expect("load codegen cache");
        let _ = std::fs::remove_file(&path);
        let loaded_prepared = loaded
            .prepared
            .as_ref()
            .expect("prepared codegen cache should be persisted");

        assert_eq!(summarize_module(&loaded.module), summarize_module(&module));
        assert_eq!(
            loaded_prepared.value_facts.expr_facts().count(),
            value_fact_count
        );
        assert_eq!(
            loaded_prepared.ownership_plan.functions.len(),
            ownership_function_count
        );
        assert_eq!(
            loaded_prepared.local_env_plan.functions.len(),
            local_env_function_count
        );
        assert_eq!(
            loaded_prepared.local_env_resume_plan.functions.len(),
            resume_function_count
        );
    }

    #[test]
    fn remaps_prepared_codegen_cache_to_fresh_module_id() {
        let module = lower_python_to_blockpy_for_testing(
            r#"
def outer(value):
    def inner():
        return value
    return inner()
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let prepared = prepared_codegen_for_module(&module);
        let mut cache = super::CachedCodegenModule {
            module,
            prepared: Some(prepared),
        };

        remap_cached_codegen_module_function_ids(&mut cache, ModuleNameGen::new(111));

        let prepared = cache
            .prepared
            .as_ref()
            .expect("prepared codegen cache should be preserved");
        for (key, _) in prepared.value_facts.expr_facts() {
            assert_eq!(key.function_id.module_id(), 111);
        }
        for ((function_id, _), _) in prepared.value_facts.block_entry_facts() {
            assert_eq!(function_id.module_id(), 111);
        }
        for function_id in prepared.ownership_plan.functions.keys() {
            assert_eq!(function_id.module_id(), 111);
        }
        for function_id in prepared.local_env_plan.functions.keys() {
            assert_eq!(function_id.module_id(), 111);
        }
        for function_id in prepared.local_env_resume_plan.functions.keys() {
            assert_eq!(function_id.module_id(), 111);
        }
        for function_plan in prepared.local_env_resume_plan.functions.values() {
            for entry in &function_plan.entries {
                assert_eq!(entry.point.function_id().module_id(), 111);
            }
        }
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

    fn prepared_codegen_for_module(
        module: &BlockPyModule<CodegenModuleShape>,
    ) -> CachedPreparedCodegen {
        let value_facts = passes::infer_module_value_facts(module);
        let ownership_plan = passes::plan_ownership_effects(module, &value_facts);
        let local_env_plan = passes::plan_local_env_module(module, &value_facts);
        let local_env_resume_plan =
            passes::plan_local_env_resume_module(module, &local_env_plan, &value_facts);
        CachedPreparedCodegen {
            value_facts,
            ownership_plan,
            local_env_plan,
            local_env_resume_plan,
        }
    }
}
