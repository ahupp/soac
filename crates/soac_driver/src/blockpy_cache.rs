use anyhow::{Context, Result, anyhow, bail};
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, CounterSite, FunctionNameGen, ModuleNameGen, RuntimeFunctionId,
    RuntimeModuleId, VisitMut, walk_expr_mut, walk_module_mut,
};
use soac_core::pass_tracker::PassTracker;
use soac_ir_blockpy::{BlockPyModuleShape, InstrBlockPy};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

const BLOCKPY_MODULE_CACHE_MAGIC: &[u8] = b"SOAC_BLOCKPY_CODEGEN_CACHE\0";
// Version 12 preserves source ranges and explicit annotation/class-dictionary
// capture projections. Older generations cannot supply those source-site and
// cell-object decisions to authenticated runtime consumers.
// Generation 16 also preserves expression-temporary lifetime categories.
// Generation 17 adds explicit generic parameter scopes/default-container
// projections and original local/free cell-load error provenance.
// Generation 18 adds explicit class-decorator preparation/application operands
// and the construction carrier; older caches cannot supply those boundaries.
// Generation 19 preserves each native annotation provider's original header
// line separately from its decorated source definition's starting line.
// Generation 20 preserves decorator call argument vectors and exact construction
// template identities without creating the namespace function prematurely.
// Generation 21 resolves actual class-cell captures and preserved-cell cleanup.
// Generation 22 adds source-selected original-function descriptor applications.
// Generation 23 distinguishes decorated class-provider start lines from
// function-provider header lines. Older metadata cannot identify the actual
// native class annotation code.
// Generation 24 retains a generic wrapper's exact native header/body span
// separately from the complete signed declaration, which includes decorators.
// Generation 25 carries explicit private class-construction cells, the paired
// namespace template, and the class-statement cleanup operation.
// Generation 26 adds explicit lexical binding/transport projections and private
// call slots, plus preserved enclosing-exception block context.
// Version 27 rejects potentially lossy literal outputs from prior caches, so
// the original-token surrogate guard cannot be bypassed by a cache hit. It also
// carries the completed handled-region BlockContext representation.
// Version 28 records producer-selected operand lifetimes and explicit
// pre-handler unwind blocks; older caches can retain failed assignment values.
// Version 29 carries exact generator-expression code exposure and explicit
// GeneratorReturn completion, plus resolved expression-operand unwind order.
// Version 30 records module-global versus explicit class-mapping call context.
// Older caches cannot supply the source activation's materialized namespace.
// Version 31 records pending abrupt-payload scopes and trim-only Unwind
// contexts, suspension edges consumed by resolved transport ownership, and
// explicit normalized raise propagation independent of handled-scope lifetime.
// Older caches can retain exited payloads or retire live suspended transports.
// Version 32 records explicit block-parameter transport-copy purpose and
// removes the stale duplicated suspended throw-context snapshot. Older caches
// can retain replaced/retired caught objects through compiler-only copies.
// Version 33 records exceptional managed-resume delivery and explicit native
// async-yield wrapping before suspension, plus async GeneratorReturn completion.
// Prior caches can redelegate an injected error or allocate after leaving the
// source yield's error edge, and still use the obsolete completion sentinel.
// Version 34 carries parser-owned source local/cell inventories and their
// resolved physical storage projections for traceback lifetime ownership.
// Version 35 adds explicit authenticated-source activation obligations and
// terminal SourceFrameExit operations before implicit local cleanup.
// Version 36 records semantic generator-control slots and fresh resume-ABI
// bindings; source variables with compiler-like spellings are not controls.
// Version 37 preserves resolved block-parameter control roles across transport
// copies and optimization; ordinary _dp_try_* names never select control state.
// Version 38 records native class code/slot identities, ordered cell recipes,
// explicit raw-slot transitions and the shared class lifetime projection,
// including consuming Operand handoffs and native collection insertion.
// Older helpers preallocate different cells and use the obsolete body ABI.
// Version 39 distinguishes local and suspended expression-operand ownership;
// a raw preserved source/cell/control slot cannot authorize a consuming read.
// Version 40 also retains qualified original source-error sites on committed
// inline fragments; implicit iterator exhaustion must not become an ordinary
// caller traceback event when the IteratorStep operation is eliminated.
// Version 40 gives each emitted native class-comprehension region its own
// snapshot owners and validated unwind boundary. Duplicated source finally
// bodies cannot alias one snapshot or reuse another site's acquisition rank.
// Version 41 records the native code's first line and distinguishes actual
// capture source ranges from class-annotation body-completion origins. A
// synthetic creation marker must never be treated as an ordinary source span.
// Version 42 represents a new source event for an already-normalized error
// explicitly, including its original range. Cached propagation-only raises
// cannot substitute for direct or delegated generator injection events.
// Version 43 resolves lexical forwarding in the creating frame and attributes
// implicit async waits to original native source ranges. Older lowered
// carriers and generated template offsets cannot be reused for these bodies.
// Version 44 gives class activations the original native header range, while
// their authenticated declaration identity still includes its decorators.
// Version 45 moves augmented-assignment operands into their sole consuming
// use and keeps delegated exception classification outside Python handlers.
// Older bodies can duplicate/retire a suspended operand twice or expose an
// internal StopIteration/forwarded error through sys.exception().
// Version 46 retains scalar native source-reference readiness beside the
// existing physical source-frame projection. Older archives have no such field.
// Version 49 preserves every original comprehension target and its clause,
// including attribute stores, beside the lambda/function local-slot carrier.
// Version 50 preserves the original eager collection kind and distinguishes
// isolated iteration targets from containing-function named-expression stores.
// Version 51 keeps each sibling/nested region's native lexical parent, with
// deduplicated current carriers and separate per-emission snapshot owners.
// Version 53 removes SOAC traceback/frame inventories, source error sites,
// native function-comprehension slot projections and frame-exit instructions.
// Semantic exception disposition, lexical bindings and ownership remain.
// Version 54 retains strict source builtin references as indexed global loads.
// Older archives may snapshot an initially absent name as a runtime builtin
// and miss later legal module bindings or captured-builtin mutations.
// Version 55 collects enclosing-scope names from lambda parameter defaults,
// including comprehension walrus stores. Older archives can lose those local
// or captured bindings even though lambda body assignments remain isolated.
// Version 56 removes native class-comprehension slot/snapshot operations.
// Class lexical cells remain explicit; eager regions use ordinary helper scopes.
// Version 57 initializes set-comprehension helpers with an empty set literal.
// Older archives call the shadowable source-global `set` constructor instead.
const BLOCKPY_MODULE_CACHE_FORMAT_VERSION: u32 = 57;

#[derive(Debug, Clone, Copy, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum PythonModuleCacheSource {
    Project,
    PythonStdlib,
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedBlockPyModuleMetadata {
    pub source: PythonModuleCacheSource,
    pub module_name: String,
    pub source_hash: u64,
    pub cache_identity: String,
}

pub(crate) struct PreOptimizationCacheTarget {
    pub(crate) path: PathBuf,
    pub(crate) metadata: CachedBlockPyModuleMetadata,
}

pub fn hash_module_source(source: &str) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in source.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

pub fn pre_optimization_module_cache_identity(
    build_identity: &str,
    runtime_names_as_globals: bool,
) -> String {
    format!("{build_identity};runtime_names_as_globals={runtime_names_as_globals}")
}

pub fn pre_optimization_module_cache_metadata(
    source: PythonModuleCacheSource,
    module_name: &str,
    source_hash: u64,
    build_identity: &str,
    runtime_names_as_globals: bool,
) -> CachedBlockPyModuleMetadata {
    CachedBlockPyModuleMetadata {
        source,
        module_name: module_name.to_string(),
        source_hash,
        cache_identity: pre_optimization_module_cache_identity(
            build_identity,
            runtime_names_as_globals,
        ),
    }
}

pub fn pre_optimization_module_cache_path(
    cache_root: &Path,
    source: PythonModuleCacheSource,
    module_name: &str,
    _source_hash: u64,
    _build_identity: &str,
    _runtime_names_as_globals: bool,
) -> std::result::Result<PathBuf, String> {
    blockpy_module_cache_path(cache_root, source, module_name).map_err(|err| err.to_string())
}

#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CachedBlockPyModule {
    pub metadata: CachedBlockPyModuleMetadata,
    pub module: BlockPyModule<BlockPyModuleShape>,
}

impl PythonModuleCacheSource {
    const fn subtree(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::PythonStdlib => "python-stdlib",
        }
    }
}

pub fn blockpy_module_cache_path(
    cache_root: impl AsRef<Path>,
    source: PythonModuleCacheSource,
    module_name: &str,
) -> Result<PathBuf> {
    let mut path = cache_root.as_ref().join(source.subtree());
    for component in module_cache_path_components(module_name)? {
        path.push(component);
    }
    path.push("mod.blockpy");
    Ok(path)
}

pub fn cached_module_paths_under_root(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    collect_cached_module_paths(root, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect_cached_module_paths(path: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("read module cache path metadata {}", path.display()))?;
    if metadata.is_file() {
        if path.file_name().and_then(|name| name.to_str()) == Some("mod.blockpy") {
            out.push(path.to_path_buf());
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    let entries = fs::read_dir(path)
        .with_context(|| format!("read module cache directory {}", path.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("read entry in {}", path.display()))?;
        collect_cached_module_paths(entry.path().as_path(), out)?;
    }
    Ok(())
}

pub fn blockpy_module_cache_key(source_hash: u64, build_identity: &str) -> String {
    format!("{source_hash:016x}-{:016x}", stable_hash(build_identity))
}

pub fn store_blockpy_module_cache(
    path: impl AsRef<Path>,
    metadata: &CachedBlockPyModuleMetadata,
    module: &BlockPyModule<BlockPyModuleShape>,
) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = non_empty_parent(path) {
        fs::create_dir_all(parent)
            .with_context(|| format!("create BlockPy cache dir {}", parent.display()))?;
    }

    let cache = CachedBlockPyModule {
        metadata: metadata.clone(),
        module: module.clone(),
    };
    let archive = rkyv::to_bytes::<rkyv::rancor::Error>(&cache)
        .map_err(|err| anyhow!("serialize BlockPy module cache: {err}"))?;
    let temp_path = temp_cache_path(path);

    {
        let mut temp_file = File::create(&temp_path)
            .with_context(|| format!("create temporary BlockPy cache {}", temp_path.display()))?;
        temp_file
            .write_all(BLOCKPY_MODULE_CACHE_MAGIC)
            .with_context(|| format!("write BlockPy cache header {}", temp_path.display()))?;
        temp_file
            .write_all(&BLOCKPY_MODULE_CACHE_FORMAT_VERSION.to_le_bytes())
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

pub fn load_blockpy_module_cache(path: impl AsRef<Path>) -> Result<CachedBlockPyModule> {
    let path = path.as_ref();
    let bytes = fs::read(path).with_context(|| format!("read BlockPy cache {}", path.display()))?;
    let archive = archive_bytes_from_cache_file(&bytes)
        .with_context(|| format!("decode BlockPy cache header {}", path.display()))?;
    let archive = aligned_archive_bytes(archive);

    let mut cache = rkyv::from_bytes::<CachedBlockPyModule, rkyv::rancor::Error>(archive.as_ref())
        .map_err(|err| anyhow!("deserialize BlockPy module cache: {err}"))?;

    rehydrate_blockpy_module_generators(&mut cache.module);
    Ok(cache)
}

pub(crate) fn try_load_pre_optimization_cache(
    cache_target: &PreOptimizationCacheTarget,
    module_name_gen: ModuleNameGen,
    pass_tracker: &mut impl PassTracker,
) -> Option<BlockPyModule<BlockPyModuleShape>> {
    let cache_path = &cache_target.path;
    let cache_exists = pass_tracker.record_timing("blockpy_cache_lookup", || cache_path.is_file());
    if !cache_exists {
        info!(
            target: "soac_blockpy_module_cache",
            event = "soac.blockpy_module_cache",
            cache_hit = false,
            path = %cache_path.display(),
            "blockpy_module_cache_miss",
        );
        return None;
    }

    let loaded = pass_tracker.record_timing("blockpy_cache_load", || {
        load_blockpy_module_cache(cache_path)
    });
    match loaded {
        Ok(mut cache) => {
            let metadata_mismatch = match validate_blockpy_module_cache_metadata(
                &cache.metadata,
                &cache_target.metadata,
            ) {
                Ok(()) => None,
                Err(err) => Some(err),
            };
            if let Some(err) = metadata_mismatch {
                warn!(
                    target: "soac_blockpy_module_cache",
                    event = "soac.blockpy_module_cache",
                    cache_hit = false,
                    path = %cache_path.display(),
                    error = %err,
                    "blockpy_module_cache_metadata_mismatch",
                );
                return None;
            }

            remap_cached_blockpy_module_function_ids(&mut cache, module_name_gen);
            info!(
                target: "soac_blockpy_module_cache",
                event = "soac.blockpy_module_cache",
                cache_hit = true,
                path = %cache_path.display(),
                "blockpy_module_cache_hit",
            );
            let CachedBlockPyModule {
                metadata: _,
                module,
            } = cache;
            Some(pass_tracker.run_pass("blockpy", || module))
        }
        Err(err) => {
            warn!(
                target: "soac_blockpy_module_cache",
                event = "soac.blockpy_module_cache",
                cache_hit = false,
                path = %cache_path.display(),
                error = %err,
                "blockpy_module_cache_load_failed",
            );
            None
        }
    }
}

pub(crate) fn store_pre_optimization_cache(
    cache_target: &PreOptimizationCacheTarget,
    module: &BlockPyModule<BlockPyModuleShape>,
    pass_tracker: &mut impl PassTracker,
) {
    let cache_path = &cache_target.path;
    let stored = pass_tracker.record_timing("blockpy_cache_store", || {
        store_blockpy_module_cache(cache_path, &cache_target.metadata, module)
    });
    match stored {
        Ok(()) => {
            info!(
                target: "soac_blockpy_module_cache",
                event = "soac.blockpy_module_cache_store",
                path = %cache_path.display(),
                "blockpy_module_cache_store",
            );
        }
        Err(err) => {
            warn!(
                target: "soac_blockpy_module_cache",
                event = "soac.blockpy_module_cache_store",
                path = %cache_path.display(),
                error = %err,
                "blockpy_module_cache_store_failed",
            );
        }
    }
}

pub fn validate_blockpy_module_cache_metadata(
    loaded: &CachedBlockPyModuleMetadata,
    expected: &CachedBlockPyModuleMetadata,
) -> Result<()> {
    if loaded == expected {
        return Ok(());
    }
    bail!(
        "BlockPy cache metadata mismatch: loaded source={:?} module={} source_hash=0x{:016x} cache_identity={:?}; expected source={:?} module={} source_hash=0x{:016x} cache_identity={:?}",
        loaded.source,
        loaded.module_name,
        loaded.source_hash,
        loaded.cache_identity,
        expected.source,
        expected.module_name,
        expected.source_hash,
        expected.cache_identity,
    )
}

pub fn rehydrate_blockpy_module_generators(module: &mut BlockPyModule<BlockPyModuleShape>) {
    module.module_name_gen = recovered_module_name_gen(module);
    for function in &mut module.callable_defs {
        function.name_gen = recovered_function_name_gen(function);
    }
}

pub fn remap_blockpy_module_function_ids(
    module: &mut BlockPyModule<BlockPyModuleShape>,
    module_name_gen: ModuleNameGen,
) {
    remap_blockpy_module_function_ids_with_remapper(
        module,
        FunctionIdRemapper {
            new_module_id: module_name_gen.runtime_module_id(),
        },
    );
}

pub fn remap_cached_blockpy_module_function_ids(
    cache: &mut CachedBlockPyModule,
    module_name_gen: ModuleNameGen,
) {
    let remapper = FunctionIdRemapper {
        new_module_id: module_name_gen.runtime_module_id(),
    };
    remap_blockpy_module_function_ids_with_remapper(&mut cache.module, remapper);
}

fn remap_blockpy_module_function_ids_with_remapper(
    module: &mut BlockPyModule<BlockPyModuleShape>,
    mut remapper: FunctionIdRemapper,
) {
    walk_module_mut(&mut remapper, module);

    for function in &mut module.callable_defs {
        function.function_id = remapper.remap(function.function_id);
        for scope in std::iter::once(&mut function.scope).chain(function.public_scope.as_mut()) {
            if let Some(construction) = &mut scope.class_construction {
                construction.namespace_function = remapper.remap(construction.namespace_function);
            }
        }
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
    new_module_id: RuntimeModuleId,
}

impl FunctionIdRemapper {
    fn remap(&self, function_id: RuntimeFunctionId) -> RuntimeFunctionId {
        if function_id == RuntimeFunctionId::global() {
            function_id
        } else {
            RuntimeFunctionId::new(self.new_module_id, function_id.local_function_id())
        }
    }
}

impl VisitMut<InstrBlockPy> for FunctionIdRemapper {
    fn visit_instr_mut(&mut self, expr: &mut InstrBlockPy)
    where
        InstrBlockPy: soac_core::block_py::ChildVisitable<InstrBlockPy>,
    {
        match expr {
            InstrBlockPy::MakeFunctionWithClosure(op) => {
                op.set_function_id(self.remap(op.function_id()));
            }
            InstrBlockPy::ConstructClass(op) => {
                op.construction_function = self.remap(op.construction_function);
            }
            InstrBlockPy::PrepareClassDecorator(op) => {
                op.construction_function = self.remap(op.construction_function);
            }
            InstrBlockPy::ApplyClassDecorator(op) => {
                op.construction_function = self.remap(op.construction_function);
            }
            InstrBlockPy::CompleteFunctionDefinition(op) => {
                op.function_id = self.remap(op.function_id);
            }
            InstrBlockPy::ApplyFunctionDescriptor(op) => {
                op.function_id = self.remap(op.function_id);
            }
            InstrBlockPy::BinOp(_)
            | InstrBlockPy::TakeOperand(_)
            | InstrBlockPy::ComprehensionInsert(_)
            | InstrBlockPy::BuildCollection(_)
            | InstrBlockPy::CallArgumentOp(_)
            | InstrBlockPy::PreparedCall(_)
            | InstrBlockPy::IteratorStep(_)
            | InstrBlockPy::DiscardClassDecorator(_)
            | InstrBlockPy::DiscardClassConstructionCaptures(_)
            | InstrBlockPy::NewAnnotationSet(_)
            | InstrBlockPy::SetupAnnotations(_)
            | InstrBlockPy::ConstructTypeParameterScope(_)
            | InstrBlockPy::SubscriptGeneric(_)
            | InstrBlockPy::SetFunctionTypeParameters(_)
            | InstrBlockPy::CreateTypeAlias(_)
            | InstrBlockPy::CreateTypeParameter(_)
            | InstrBlockPy::SetTypeParameterDefault(_)
            | InstrBlockPy::CheckAnnotationFormat(_)
            | InstrBlockPy::RecordAnnotation(_)
            | InstrBlockPy::UnaryOp(_)
            | InstrBlockPy::Tuple(_)
            | InstrBlockPy::Call(_)
            | InstrBlockPy::GetAttr(_)
            | InstrBlockPy::SetAttr(_)
            | InstrBlockPy::GetItem(_)
            | InstrBlockPy::SetItem(_)
            | InstrBlockPy::DelItem(_)
            | InstrBlockPy::Load(_)
            | InstrBlockPy::Store(_)
            | InstrBlockPy::Del(_)
            | InstrBlockPy::MakeCell(_)
            | InstrBlockPy::IncrementCounter(_)
            | InstrBlockPy::CellRef(_) => {}
        }
        walk_expr_mut(self, expr);
    }
}

fn archive_bytes_from_cache_file(bytes: &[u8]) -> Result<&[u8]> {
    let rest = bytes
        .strip_prefix(BLOCKPY_MODULE_CACHE_MAGIC)
        .ok_or_else(|| anyhow!("invalid BlockPy cache magic"))?;
    let (version_bytes, archive) = rest
        .split_at_checked(std::mem::size_of::<u32>())
        .ok_or_else(|| anyhow!("BlockPy cache is truncated"))?;
    let version = u32::from_le_bytes(
        version_bytes
            .try_into()
            .expect("split_at_checked returned exactly four version bytes"),
    );
    if version != BLOCKPY_MODULE_CACHE_FORMAT_VERSION {
        return Err(anyhow!(
            "unsupported BlockPy cache version {version}; expected {BLOCKPY_MODULE_CACHE_FORMAT_VERSION}"
        ));
    }
    Ok(archive)
}

fn aligned_archive_bytes(bytes: &[u8]) -> rkyv::util::AlignedVec {
    let mut aligned = rkyv::util::AlignedVec::with_capacity(bytes.len());
    aligned.extend_from_slice(bytes);
    aligned
}

fn module_cache_path_components(module_name: &str) -> Result<Vec<&str>> {
    if module_name.is_empty() {
        bail!("module cache path requires a non-empty module name");
    }
    let mut components = Vec::new();
    for component in module_name.split('.') {
        let valid = !component.is_empty()
            && component != "."
            && component != ".."
            && !component
                .bytes()
                .any(|byte| matches!(byte, b'/' | b'\\' | 0));
        if !valid {
            bail!("module cache path component is invalid: {component:?} in {module_name:?}");
        }
        components.push(component);
    }
    Ok(components)
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

fn recovered_module_name_gen(module: &BlockPyModule<BlockPyModuleShape>) -> ModuleNameGen {
    let module_id = module
        .callable_defs
        .first()
        .map(|function| function.function_id.runtime_module_id().as_u32())
        .unwrap_or(0);
    let next_function_id = module
        .callable_defs
        .iter()
        .filter(|function| function.function_id.runtime_module_id().as_u32() == module_id)
        .map(|function| {
            function
                .function_id
                .local_function_id()
                .as_u32()
                .saturating_add(1)
        })
        .max()
        .unwrap_or(1)
        .max(1);
    ModuleNameGen::recovered(module_id, next_function_id)
}

fn recovered_function_name_gen(function: &BlockPyFunction<BlockPyModuleShape>) -> FunctionNameGen {
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
        .unwrap_or_else(|| "blockpy-cache".into());
    let temp_file_name = format!("{file_name}.tmp.{}", std::process::id());

    let mut temp_path = path.to_owned();
    temp_path.set_file_name(temp_file_name);
    temp_path
}

#[cfg(test)]
mod test {
    use super::{
        CachedBlockPyModuleMetadata, PythonModuleCacheSource, blockpy_module_cache_key,
        blockpy_module_cache_path, load_blockpy_module_cache, remap_blockpy_module_function_ids,
        store_blockpy_module_cache, validate_blockpy_module_cache_metadata,
    };
    use soac_core::block_py::{
        BlockPyModule, ChildVisitable, HasSemanticInstrId, ModuleNameGen, RuntimeFunctionId, Visit,
        walk_block, walk_expr,
    };
    use soac_ir_blockpy::{BlockPyModuleShape, InstrBlockPy};
    use soac_lowering::lower_python_to_blockpy_for_testing;
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
        function_id: RuntimeFunctionId,
        name_gen_function_id: RuntimeFunctionId,
        qualname: String,
        block_labels: Vec<u32>,
        block_body_lens: Vec<usize>,
        instr_ids: Vec<u32>,
    }

    struct InstrIdCollector {
        instr_ids: Vec<u32>,
    }

    impl Visit<InstrBlockPy> for InstrIdCollector {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            let instr_id = expr.semantic_instr_id();
            self.instr_ids.push(instr_id.index());
            walk_expr(self, expr);
        }
    }

    fn collect_make_function_with_closure_ids(
        module: &BlockPyModule<BlockPyModuleShape>,
    ) -> Vec<RuntimeFunctionId> {
        struct Collector {
            function_ids: Vec<RuntimeFunctionId>,
        }

        impl Visit<InstrBlockPy> for Collector {
            fn visit_instr(&mut self, expr: &InstrBlockPy)
            where
                InstrBlockPy: ChildVisitable<InstrBlockPy>,
            {
                if let InstrBlockPy::MakeFunctionWithClosure(op) = expr {
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
    fn round_trips_blockpy_module_cache_without_rendering() {
        let module = lower_python_to_blockpy_for_testing(
            r#"
def f(x):
    if x:
        return g(x + 1)
    return g(0)

def g(y):
    return y

async def values(awaitable):
    try:
        result = await awaitable
        yield result
    finally:
        finish()
"#,
        )
        .expect("transform should succeed")
        .blockpy_module;
        let before = summarize_module(&module);
        let before_layouts = module
            .callable_defs
            .iter()
            .map(|function| function.storage_layout.clone())
            .collect::<Vec<_>>();
        assert!(before_layouts.iter().flatten().any(|layout| {
            layout
                .block_parameter_roles
                .iter()
                .any(|binding| binding.role == soac_core::block_py::BlockParamRole::AbruptKind)
        }));
        let runtime_names = |module: &BlockPyModule<BlockPyModuleShape>| {
            module
                .module_constants
                .iter()
                .filter_map(|value| match value {
                    soac_core::block_py::ConstantExpr::RuntimeName(name) => Some(*name),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let before_runtime_names = runtime_names(&module);
        for name in [
            soac_core::block_py::RuntimeName::GeneratorResumeDelivery,
            soac_core::block_py::RuntimeName::InjectGeneratorResumeException,
            soac_core::block_py::RuntimeName::AsyncGenWrapYield,
        ] {
            assert!(before_runtime_names.contains(&name));
        }

        let path = unique_cache_path();
        let _ = std::fs::remove_file(&path);
        let metadata = test_metadata("pkg.mod", 0x1234, "build-a");
        store_blockpy_module_cache(&path, &metadata, &module).expect("store blockpy cache");

        let loaded_cache = load_blockpy_module_cache(&path).expect("load blockpy cache");
        validate_blockpy_module_cache_metadata(&loaded_cache.metadata, &metadata)
            .expect("metadata should round-trip");
        let loaded = loaded_cache.module;
        let _ = std::fs::remove_file(&path);

        assert_eq!(summarize_module(&loaded), before);
        assert_eq!(runtime_names(&loaded), before_runtime_names);
        assert_eq!(
            loaded
                .callable_defs
                .iter()
                .map(|function| function.storage_layout.clone())
                .collect::<Vec<_>>(),
            before_layouts,
            "resolved control roles and their physical slots must survive the real cache format"
        );
        for function in &loaded.callable_defs {
            if let Some(layout) = &function.storage_layout {
                layout.validate_block_parameter_roles().unwrap();
            }
        }

        let max_function_id = loaded
            .callable_defs
            .iter()
            .map(|function| function.function_id.local_function_id().as_u32())
            .max()
            .expect("test module should have callable defs");
        let recovered_next_function = loaded.module_name_gen.next_function_name_gen();
        assert_eq!(
            recovered_next_function
                .function_id()
                .local_function_id()
                .as_u32(),
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
    fn generator_code_exposure_round_trips_and_survives_runtime_id_remapping() {
        use soac_contracts::SourceRange;
        use soac_core::block_py::{BlockTerm, FunctionKind, GeneratorExpressionCode};

        let source = "def make(values):\n    return (value for value in values)\n";
        let mut module = lower_python_to_blockpy_for_testing(source)
            .expect("lower ordinary serialization fixture")
            .blockpy_module;
        let expression = "(value for value in values)";
        let start = source.find(expression).unwrap();
        let iterable = start + expression.rfind("values").unwrap();
        let projection = GeneratorExpressionCode {
            expression_range: SourceRange::new(start as u32, (start + expression.len()) as u32),
            iterable_range: SourceRange::new(iterable as u32, (iterable + "values".len()) as u32),
        };
        let function = module
            .callable_defs
            .iter_mut()
            .find(|function| matches!(function.lowered_kind(), FunctionKind::Generator))
            .expect("lowered generator helper");
        let local_id = function.function_id.local_function_id();
        let original_params = function.params.clone();
        let completion_blocks = function
            .blocks
            .iter()
            .filter(|block| matches!(block.term, BlockTerm::GeneratorReturn(_)))
            .count();
        assert!(
            completion_blocks > 0,
            "generator completion must be explicit before caching"
        );
        // This is a serialization/remapping kernel, not runtime admission.
        // The lowerer test independently checks the actual parser projection;
        // this ordinary module has neither signed facts nor a source capability.
        assert!(function.scope.source_origin.is_none());
        function.scope.generator_expression_code = Some(projection.clone());
        function
            .public_scope
            .as_mut()
            .expect("generator public scope")
            .generator_expression_code = Some(projection.clone());

        let path = unique_cache_path();
        let metadata = test_metadata("pkg.generators", 42, "genexpr-code-v29");
        store_blockpy_module_cache(&path, &metadata, &module).expect("store code exposure");
        let mut loaded = load_blockpy_module_cache(&path)
            .expect("load code exposure")
            .module;
        std::fs::remove_file(&path).expect("remove code exposure cache");
        remap_blockpy_module_function_ids(&mut loaded, ModuleNameGen::new(99));
        assert!(loaded.strict_source.is_none());
        let function = loaded
            .callable_defs
            .iter()
            .find(|function| function.function_id.local_function_id() == local_id)
            .expect("same helper after runtime-id remapping");
        assert_eq!(function.function_id.runtime_module_id().as_u32(), 99);
        assert_eq!(function.params, original_params);
        assert_eq!(function.lowered_kind(), &FunctionKind::Generator);
        assert_eq!(
            function
                .blocks
                .iter()
                .filter(|block| matches!(block.term, BlockTerm::GeneratorReturn(_)))
                .count(),
            completion_blocks
        );
        for scope in [&function.scope, function.public_scope.as_ref().unwrap()] {
            assert_eq!(scope.generator_expression_code.as_ref(), Some(&projection));
            assert!(scope.source_origin.is_none());
        }
    }

    #[test]
    fn cache_path_keeps_python_stdlib_in_a_separate_subtree() {
        let root = PathBuf::from("/cache/root");

        assert_eq!(
            blockpy_module_cache_path(&root, PythonModuleCacheSource::Project, "pkg.submod")
                .expect("project cache path"),
            PathBuf::from("/cache/root/project/pkg/submod/mod.blockpy")
        );
        assert_eq!(
            blockpy_module_cache_path(&root, PythonModuleCacheSource::PythonStdlib, "typing")
                .expect("stdlib cache path"),
            PathBuf::from("/cache/root/python-stdlib/typing/mod.blockpy")
        );
        assert!(
            blockpy_module_cache_path(&root, PythonModuleCacheSource::PythonStdlib, "../escape")
                .is_err()
        );
    }

    #[test]
    fn cache_key_combines_source_hash_and_build_identity_hash() {
        assert_eq!(
            blockpy_module_cache_key(0x1234, "build-a"),
            "0000000000001234-09e267510d26cc71"
        );
        assert_ne!(
            blockpy_module_cache_key(0x1234, "build-a"),
            blockpy_module_cache_key(0x1234, "build-b")
        );
        assert_ne!(
            blockpy_module_cache_key(0x1234, "build-a"),
            blockpy_module_cache_key(0x5678, "build-a")
        );
    }

    #[test]
    fn remaps_cached_blockpy_module_to_fresh_module_id() {
        use soac_contracts::{DefinitionKind, ModuleContentId, SourceIdentity, SourceRange};
        use soac_core::block_py::{
            CallableSourceOrigin, CallableSourceRole, ClassConstructionScope,
            LexicalCaptureProjection, LexicalCellBinding, LexicalCellCapture, PrivateLexicalScope,
        };

        let mut module = lower_python_to_blockpy_for_testing(
            r#"
def outer():
    def inner():
        return 1
    return inner()
"#,
        )
        .expect("transform should succeed")
        .blockpy_module;

        // Cache identity remapping changes runtime addresses, never signed
        // lexical identities or their exact nominal-leaf projection.
        let namespace = module.callable_defs[0].function_id;
        let creator = CallableSourceOrigin {
            definition: SourceIdentity {
                module: ModuleContentId::new("pkg.capture", 42),
                lexical_qualname: "outer".into(),
                source_range: SourceRange::new(1, 90),
                definition_kind: DefinitionKind::Function,
            },
            role: CallableSourceRole::SourceFunction,
        };
        let capture = LexicalCellCapture {
            binding: LexicalCellBinding {
                scope: creator.definition.clone(),
                name: "Target".into(),
            },
            nominal_binding_indices: vec![2, 5],
        };
        let construction = ClassConstructionScope {
            producer: creator.clone(),
            namespace_function: namespace,
            captures: vec![capture.clone()],
        };
        let private = PrivateLexicalScope {
            creator,
            captures: vec![LexicalCaptureProjection {
                cell: capture,
                native_closure: None,
            }],
        };
        let function = &mut module.callable_defs[1];
        function.scope.class_construction = Some(construction.clone());
        function.scope.private_lexical = Some(private.clone());
        function.public_scope = Some(function.scope.clone());

        // Exercise the serialized DTO before the runtime-id rewrite as well.
        let path = unique_cache_path();
        let metadata = test_metadata("pkg.capture", 42, "private-lexical-v26");
        store_blockpy_module_cache(&path, &metadata, &module).expect("store capture metadata");
        let cached = load_blockpy_module_cache(&path).expect("load capture metadata");
        std::fs::remove_file(&path).expect("remove capture cache");
        module = cached.module;

        remap_blockpy_module_function_ids(&mut module, ModuleNameGen::new(99));

        assert_eq!(module.module_name_gen.module_id(), 99);
        for function in &module.callable_defs {
            assert_eq!(function.function_id.runtime_module_id().as_u32(), 99);
            assert_eq!(function.name_gen.function_id(), function.function_id);
        }
        let make_function_ids = collect_make_function_with_closure_ids(&module);
        assert!(
            !make_function_ids.is_empty(),
            "test module should contain MakeFunctionWithClosure"
        );
        for function_id in make_function_ids {
            assert_eq!(
                function_id.runtime_module_id().as_u32(),
                99,
                "cached MakeFunctionWithClosure ids must point at the remapped module id"
            );
        }

        let function = &module.callable_defs[1];
        for scope in [&function.scope, function.public_scope.as_ref().unwrap()] {
            let actual = scope.class_construction.as_ref().unwrap();
            assert_eq!(actual.namespace_function.runtime_module_id().as_u32(), 99);
            assert_eq!(
                actual.namespace_function.local_function_id(),
                namespace.local_function_id()
            );
            assert_eq!(actual.producer, construction.producer);
            assert_eq!(actual.captures, construction.captures);
            assert_eq!(scope.private_lexical.as_ref(), Some(&private));
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

    fn summarize_module(module: &BlockPyModule<BlockPyModuleShape>) -> ModuleSummary {
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
        function: &soac_core::block_py::BlockPyFunction<BlockPyModuleShape>,
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

    fn instr_ids(function: &soac_core::block_py::BlockPyFunction<BlockPyModuleShape>) -> Vec<u32> {
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
            "soac-blockpy-cache-test-{}-{unique}.rkyv",
            std::process::id()
        ))
    }

    fn test_metadata(
        module_name: &str,
        source_hash: u64,
        cache_identity: &str,
    ) -> CachedBlockPyModuleMetadata {
        CachedBlockPyModuleMetadata {
            source: PythonModuleCacheSource::Project,
            module_name: module_name.to_string(),
            source_hash,
            cache_identity: cache_identity.to_string(),
        }
    }
}
