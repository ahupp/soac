use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyModule};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use soac_blockpy::block_py::{BlockPyFunction, BlockPyModule, FunctionId, ModuleNameGen};
use soac_blockpy::passes::{
    CodegenModuleShape, infer_module_value_facts, plan_local_env_module,
    render_local_env_function_plan, render_local_env_module_plan,
};
use soac_jit::module_constants::ModuleCodegenConstants;
use soac_jit::module_type::build_shared_state_for_inspection;
use soac_jit::{
    plan_jit_module_locals, render_cranelift_run_bb_specialized_with_runtime_state_and_cfg,
    render_jit_function_locals, render_jit_module_locals,
};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};
use tower_http::services::ServeDir;

pub use soac_jit::counter_dump::{
    CollectedKeyLayout, CounterDumpFile, CounterDumpKeyLayoutView, CounterDumpRecordView,
    CounterDumpRowView, CounterDumpTypeKeyLayoutView, collect_module_key_layouts,
    collect_type_key_layouts, parse_counter_dump_records,
};

static NEXT_WEB_MODULE_ID: AtomicU64 = AtomicU64::new(1);
static PYTHON_INIT: Once = Once::new();

#[derive(Clone)]
pub struct AppState {
    repo_root: PathBuf,
    web_dir: PathBuf,
}

#[derive(Deserialize)]
struct InspectPipelineRequest {
    source: String,
}

#[derive(Deserialize)]
struct JitClifRequest {
    source: String,
    #[serde(rename = "functionId")]
    function_id: String,
    qualname: Option<String>,
    #[serde(rename = "entryLabel")]
    entry_label: Option<String>,
}

#[derive(Deserialize)]
struct SpeedscopeProfileRequest {
    path: String,
}

#[derive(Serialize)]
pub struct JitClifResponse {
    pub clif: String,
    #[serde(rename = "cfgDot")]
    pub cfg_dot: String,
    #[serde(rename = "vcodeDisasm")]
    pub vcode_disasm: String,
    pub resolved_entry: String,
}

#[derive(Clone, Debug, Default)]
pub struct JitClifRenderOptions {
    pub load_runtime_specializations: bool,
    pub runtime_source_path: Option<PathBuf>,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    error: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.error }))).into_response()
    }
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace crate should have a repo-root parent")
        .to_path_buf()
}

pub fn web_dir() -> PathBuf {
    repo_root().join("web")
}

pub fn app() -> Router {
    let state = AppState {
        repo_root: repo_root(),
        web_dir: web_dir(),
    };
    app_with_state(state)
}

pub fn app_with_state(state: AppState) -> Router {
    Router::new()
        .route("/api/inspect_pipeline", post(handle_inspect_pipeline))
        .route("/api/jit_clif", post(handle_jit_clif))
        .route("/api/speedscope_profile", get(handle_speedscope_profile))
        .fallback_service(ServeDir::new(state.web_dir.clone()))
        .with_state(state)
}

pub fn prepare_python() {
    PYTHON_INIT.call_once(|| {
        configure_embedded_python_env();
        Python::initialize();
    });
}

fn configure_embedded_python_env() {
    let repo_root = repo_root();
    let python_home = repo_root.join("vendor/cpython");
    let mut python_path_entries = vec![python_home.join("Lib")];
    if let Some(build_lib_dir) = find_python_build_lib_dir(&python_home) {
        python_path_entries.push(build_lib_dir);
    }
    let python_path =
        std::env::join_paths(python_path_entries).expect("vendored CPython paths should be valid");
    // Configure the embedded interpreter to use the vendored CPython tree
    // before the first interpreter initialization.
    unsafe {
        std::env::set_var("PYTHONHOME", &python_home);
        std::env::set_var("PYTHONPATH", &python_path);
    }
}

fn find_python_build_lib_dir(python_home: &Path) -> Option<PathBuf> {
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

fn find_venv_site_packages(repo_root: &Path) -> Option<PathBuf> {
    let lib_dir = repo_root.join(".venv").join("lib");
    let entries = std::fs::read_dir(lib_dir).ok()?;
    for entry in entries {
        let path = entry.ok()?.path();
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name().and_then(|name| name.to_str())?;
        if !name.starts_with("python") {
            continue;
        }
        let site_packages = path.join("site-packages");
        if site_packages.is_dir() {
            return Some(site_packages);
        }
    }
    None
}

fn ensure_python_support_paths(py: Python<'_>, repo_root: &Path) -> Result<(), ApiError> {
    let sys = PyModule::import(py, "sys").map_err(|err| ApiError::internal(err.to_string()))?;
    let path = sys
        .getattr("path")
        .map_err(|err| ApiError::internal(err.to_string()))?;
    let path = path
        .cast::<PyList>()
        .map_err(|err| ApiError::internal(err.to_string()))?;
    let support_paths = [
        repo_root.to_path_buf(),
        repo_root.join("soac_py").join("src"),
        find_venv_site_packages(repo_root)
            .ok_or_else(|| ApiError::internal("repo venv site-packages not found".to_string()))?,
    ];
    for support_path in support_paths.iter().rev() {
        let support_path = support_path.to_string_lossy();
        let already_present = path.iter().any(|item| {
            item.extract::<String>()
                .map(|value| value == support_path)
                .unwrap_or(false)
        });
        if !already_present {
            path.insert(0, support_path.as_ref())
                .map_err(|err| ApiError::internal(err.to_string()))?;
        }
    }
    Ok(())
}

fn lower_source_recorded(source: &str) -> Result<soac_blockpy::LoweringResult, ApiError> {
    soac_blockpy::lower_python_to_blockpy_for_testing(source)
        .map_err(|err| ApiError::internal(err.to_string()))
}

fn inspector_function_payload(function: &BlockPyFunction<CodegenModuleShape>) -> Value {
    json!({
        "functionId": function.function_id.packed().to_string(),
        "qualname": function.names.qualname,
        "displayName": function.names.display_name,
        "bindName": function.names.bind_name,
        "kind": format!("{:?}", function.kind).to_lowercase(),
        "entryLabel": function.entry_block().label_str(),
    })
}

fn render_inspector_payload(source: &str, output: &soac_blockpy::LoweringResult) -> Value {
    let mut steps = vec![json!({
        "key": "input_source",
        "label": "input source",
        "text": source,
    })];
    for name in output.pass_tracker.pass_names() {
        let text = output
            .pass_tracker
            .render_pass_debug_text(name)
            .unwrap_or_else(|| format!("; no text renderer for pass {name}"));
        steps.push(json!({
            "key": name,
            "label": name,
            "text": text,
        }));
    }
    let facts = infer_module_value_facts(&output.codegen_module);
    let local_env_plan_text = (|| {
        let plan = plan_local_env_module(&output.codegen_module, &facts);
        render_local_env_module_plan(&output.codegen_module, &facts, &plan)
    })()
    .unwrap_or_else(|err| format!("; failed to render local_env_plan: {err}"));
    steps.push(json!({
        "key": "local_env_plan",
        "label": "local env plan",
        "text": local_env_plan_text,
    }));
    let jit_local_plan_text = (|| {
        let plan = plan_jit_module_locals(&output.codegen_module, &facts)?;
        render_jit_module_locals(&output.codegen_module, &plan)
    })()
    .unwrap_or_else(|err| format!("; failed to render jit_local_plan: {err}"));
    steps.push(json!({
        "key": "jit_local_plan",
        "label": "jit local plan",
        "text": jit_local_plan_text,
    }));
    json!({
        "steps": steps,
        "functions": output
            .codegen_module
            .callable_defs
            .iter()
            .map(inspector_function_payload)
            .collect::<Vec<_>>(),
    })
}

pub fn lower_source_to_codegen_module(
    source: &str,
) -> Result<BlockPyModule<CodegenModuleShape>, String> {
    let output = lower_source_recorded(source).map_err(|err| err.error)?;
    Ok(output.codegen_module)
}

pub fn lower_source_to_codegen_module_with_module_id(
    source: &str,
    module_id: u32,
) -> Result<BlockPyModule<CodegenModuleShape>, String> {
    let output =
        soac_blockpy::lower_python_to_blockpy_recorded(source, ModuleNameGen::new(module_id))
            .map_err(|err| err.to_string())?;
    Ok(output.codegen_module)
}

fn counter_dump_input_path_from_env_for_render() -> Option<PathBuf> {
    let mode = std::env::var("SOAC_OPT_MODE").ok()?;
    match mode.trim() {
        "verify" | "apply" => std::env::var_os("SOAC_WORK_DIR")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .map(|dir| dir.join("profile.bin")),
        _ => None,
    }
}

pub fn profile_module_id_from_env(module_name: &str) -> Result<Option<u32>, String> {
    let Some(path) = counter_dump_input_path_from_env_for_render() else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    let dump = CounterDumpFile::open(path.as_path())?;
    let records = dump.records()?;
    for record in records {
        if record.module_name()? != module_name {
            continue;
        }
        for row_index in 0..record.row_count() {
            let row = record.row(row_index)?;
            let Some(function_id) = row.function_id else {
                continue;
            };
            if function_id == FunctionId::global() {
                continue;
            }
            return Ok(Some(function_id.module_id()));
        }
    }
    Ok(None)
}

fn next_web_module_name() -> String {
    format!(
        "_dp_web_{:016x}",
        NEXT_WEB_MODULE_ID.fetch_add(1, Ordering::Relaxed)
    )
}

fn inspect_pipeline_payload(source: &str) -> Result<Value, ApiError> {
    let output = lower_source_recorded(source)?;
    Ok(render_inspector_payload(source, &output))
}

pub fn jit_debug_plan(
    module_name: &str,
    module: &BlockPyModule<CodegenModuleShape>,
    function_id: FunctionId,
) -> Result<String, String> {
    let facts = infer_module_value_facts(module);
    let Some(function) = module
        .callable_defs
        .iter()
        .find(|function| function.function_id == function_id)
    else {
        return Err(format!(
            "no specialized JIT plan for {module_name}.fn#{function_id}"
        ));
    };
    let local_env_plan = plan_local_env_module(module, &facts);
    let local_env_plan_text = render_local_env_function_plan(
        function,
        local_env_plan
            .function(function.function_id)
            .ok_or_else(|| format!("missing LocalEnv plan for {module_name}.fn#{function_id}"))?,
    )?;
    let jit_module_local_plan = plan_jit_module_locals(module, &facts)?;
    let jit_local_plan = jit_module_local_plan
        .function(function.function_id)
        .ok_or_else(|| format!("missing JIT local plan for {module_name}.fn#{function_id}"))?;
    let jit_local_plan_text = render_jit_function_locals(function, jit_local_plan)?;
    Ok(format!(
        "function:\n{function:#?}\n\nlocal_env_plan:\n{local_env_plan_text}\n\njit_local_plan:\n{jit_local_plan_text}"
    ))
}

pub fn render_jit_clif_for_module(
    repo_root: &Path,
    module_name: &str,
    module: &BlockPyModule<CodegenModuleShape>,
    function_id: FunctionId,
) -> Result<JitClifResponse, String> {
    render_jit_clif_for_module_with_options(
        repo_root,
        module_name,
        module,
        function_id,
        JitClifRenderOptions::default(),
    )
}

fn module_package_name(module_name: &str) -> &str {
    module_name
        .rsplit_once('.')
        .map(|(package, _)| package)
        .unwrap_or("")
}

fn execute_module_for_runtime_render_state(
    py: Python<'_>,
    source_path: &Path,
    module_name: &str,
    indexed_module_keys: &[String],
) -> Result<(), String> {
    let source_path = source_path
        .to_str()
        .ok_or_else(|| format!("source path is not valid utf-8: {}", source_path.display()))?;
    let ext = PyModule::import(py, "_soac_ext").map_err(|err| err.to_string())?;
    let importlib = PyModule::import(py, "importlib.machinery").map_err(|err| err.to_string())?;
    let module_spec = importlib
        .getattr("ModuleSpec")
        .map_err(|err| err.to_string())?;
    let spec = module_spec
        .call1((module_name, py.None()))
        .map_err(|err| err.to_string())?;
    let module = ext
        .getattr("create_module")
        .map_err(|err| err.to_string())?
        .call1((source_path, &spec))
        .map_err(|err| err.to_string())?;
    let globals = module.getattr("__dict__").map_err(|err| err.to_string())?;
    let globals = globals.cast::<PyDict>().map_err(|err| err.to_string())?;
    globals
        .set_item("__package__", module_package_name(module_name))
        .map_err(|err| err.to_string())?;
    globals
        .set_item("__file__", source_path)
        .map_err(|err| err.to_string())?;
    ext.getattr("exec_module")
        .map_err(|err| err.to_string())?
        .call1((&module,))
        .map_err(|err| err.to_string())?;
    unsafe {
        soac_jit::register_function_owner_types_for_module_keys(
            module.as_ptr(),
            indexed_module_keys,
        )
    }
    .map_err(|_| {
        if unsafe { pyo3::ffi::PyErr_Occurred() }.is_null() {
            "failed to register owner types for rendered runtime module".to_string()
        } else {
            PyErr::fetch(py).to_string()
        }
    })
}

fn corresponding_runtime_function<'a>(
    module: &'a BlockPyModule<CodegenModuleShape>,
    requested_function: &BlockPyFunction<CodegenModuleShape>,
) -> Option<&'a BlockPyFunction<CodegenModuleShape>> {
    module
        .callable_defs
        .iter()
        .find(|function| {
            function.function_id.function_id() == requested_function.function_id.function_id()
        })
        .or_else(|| {
            module
                .callable_defs
                .iter()
                .find(|function| function.names.qualname == requested_function.names.qualname)
        })
}

pub fn render_jit_clif_for_module_with_options(
    repo_root: &Path,
    module_name: &str,
    module: &BlockPyModule<CodegenModuleShape>,
    function_id: FunctionId,
    options: JitClifRenderOptions,
) -> Result<JitClifResponse, String> {
    let function = module
        .callable_defs
        .iter()
        .find(|function| function.function_id == function_id)
        .cloned()
        .ok_or_else(|| format!("no specialized JIT plan for {module_name}.fn#{function_id}"))?;
    let module_constants = ModuleCodegenConstants::collect_from_module(module);
    let compile_session = soac_jit::CompileSession::new();
    prepare_python();
    let (rendered, resolved_qualname, resolved_function_id, entry_label) = Python::attach(|py| {
        ensure_python_support_paths(py, repo_root).map_err(|err| err.error)?;
        PyModule::import(py, "soac.runtime").map_err(|err| err.to_string())?;
        let runtime_state = if options.load_runtime_specializations {
            if let Some(source_path) = options.runtime_source_path.as_deref() {
                execute_module_for_runtime_render_state(
                    py,
                    source_path,
                    module_name,
                    &module.global_names,
                )?;
            }
            Some(
                build_shared_state_for_inspection(py, module.clone(), module_name, "")
                    .map_err(|err| err.to_string())?,
            )
        } else {
            None
        };

        let (rendered, resolved_qualname, resolved_function_id, entry_label) =
            if let Some(shared_state) = runtime_state.as_deref() {
                let render_function =
                    corresponding_runtime_function(&shared_state.lowered_module, &function)
                        .cloned()
                        .ok_or_else(|| {
                            format!(
                                "runtime module for {module_name} did not contain function {} ({})",
                                function.function_id, function.names.qualname
                            )
                        })?;
                let rendered = unsafe {
                    render_cranelift_run_bb_specialized_with_runtime_state_and_cfg(
                        &compile_session,
                        &vec![std::ptr::null_mut::<c_void>(); render_function.blocks.len()],
                        &shared_state.lowered_module,
                        &render_function,
                        &shared_state.codegen_constants,
                        Some(shared_state),
                    )
                }?;
                let entry_label = render_function
                    .blocks
                    .first()
                    .map(|block| block.label.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string());
                (
                    rendered,
                    render_function.names.qualname.to_string(),
                    render_function.function_id,
                    entry_label,
                )
            } else {
                let rendered = unsafe {
                    render_cranelift_run_bb_specialized_with_runtime_state_and_cfg(
                        &compile_session,
                        &vec![std::ptr::null_mut::<c_void>(); function.blocks.len()],
                        module,
                        &function,
                        &module_constants,
                        None,
                    )
                }?;
                let entry_label = function
                    .blocks
                    .first()
                    .map(|block| block.label.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string());
                (
                    rendered,
                    function.names.qualname.to_string(),
                    function.function_id,
                    entry_label,
                )
            };

        Ok::<_, String>((
            rendered,
            resolved_qualname,
            resolved_function_id,
            entry_label,
        ))
    })?;
    Ok(JitClifResponse {
        clif: rendered.clif,
        cfg_dot: rendered.cfg_dot,
        vcode_disasm: rendered.vcode_disasm,
        resolved_entry: format!(
            "{}::__dp_fn_{}::{}",
            resolved_qualname,
            resolved_function_id.packed(),
            entry_label
        ),
    })
}

fn render_jit_clif(
    repo_root: &Path,
    source: &str,
    function_id: FunctionId,
    qualname: Option<&str>,
    entry_label: &str,
) -> Result<JitClifResponse, ApiError> {
    let module_name = next_web_module_name();
    let module = lower_source_to_codegen_module(source).map_err(ApiError::internal)?;
    let mut rendered =
        render_jit_clif_for_module(repo_root, module_name.as_str(), &module, function_id)
            .map_err(ApiError::internal)?;
    rendered.resolved_entry = format!(
        "{}::__dp_fn_{}::{}",
        qualname.unwrap_or("<unknown>"),
        function_id.packed(),
        entry_label
    );
    Ok(rendered)
}

async fn handle_inspect_pipeline(
    Json(request): Json<InspectPipelineRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(inspect_pipeline_payload(request.source.as_str())?))
}

async fn handle_jit_clif(
    State(state): State<AppState>,
    Json(request): Json<JitClifRequest>,
) -> Result<Json<JitClifResponse>, ApiError> {
    let function_id = parse_packed_function_id(request.function_id.as_str())?;
    let entry_label = request
        .entry_label
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("entryLabel must be provided"))?;
    Ok(Json(render_jit_clif(
        &state.repo_root,
        request.source.as_str(),
        function_id,
        request.qualname.as_deref(),
        entry_label,
    )?))
}

async fn handle_speedscope_profile(
    State(state): State<AppState>,
    Query(request): Query<SpeedscopeProfileRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let profile_path = resolve_speedscope_profile_path(&state.repo_root, request.path.as_str())?;
    let body = std::fs::read_to_string(&profile_path)
        .map_err(|err| ApiError::bad_request(format!("failed to read profile JSON: {err}")))?;
    Ok((
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        body,
    ))
}

fn parse_packed_function_id(raw: &str) -> Result<FunctionId, ApiError> {
    raw.parse::<u64>()
        .map(FunctionId::from_packed)
        .map_err(|err| ApiError::bad_request(format!("invalid functionId '{raw}': {err}")))
}

fn resolve_speedscope_profile_path(repo_root: &Path, raw_path: &str) -> Result<PathBuf, ApiError> {
    let logs_root = repo_root.join("logs");
    let logs_root = logs_root.canonicalize().map_err(|err| {
        ApiError::internal(format!(
            "failed to resolve logs directory '{}': {err}",
            logs_root.display()
        ))
    })?;

    let requested_path = Path::new(raw_path);
    let candidate_path = if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        repo_root.join(requested_path)
    };
    let candidate_path = candidate_path.canonicalize().map_err(|err| {
        ApiError::bad_request(format!(
            "failed to resolve requested profile '{}': {err}",
            requested_path.display()
        ))
    })?;

    if !candidate_path.starts_with(&logs_root) {
        return Err(ApiError::bad_request(format!(
            "speedscope profiles must live under '{}'",
            logs_root.display()
        )));
    }
    if candidate_path.extension().and_then(|ext| ext.to_str()) != Some("json") {
        return Err(ApiError::bad_request(format!(
            "expected a .json speedscope profile, got '{}'",
            candidate_path.display()
        )));
    }

    Ok(candidate_path)
}

#[cfg(test)]
mod test {
    use super::{AppState, app, app_with_state};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use std::path::{Path, PathBuf};
    use tower::ServiceExt;

    async fn response_text(response: axum::response::Response) -> String {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collection should succeed")
            .to_bytes();
        String::from_utf8(bytes.to_vec()).expect("response body should be utf-8")
    }

    #[tokio::test]
    async fn serves_index_and_inspect_pipeline() {
        let app = app();
        let response = app
            .clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .expect("static request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let html = response_text(response).await;
        assert!(html.contains("/api/inspect_pipeline"));
        assert!(html.contains("/api/jit_clif"));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/inspect_pipeline")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"source": "def classify(n):\n    return n + 1\n"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .expect("inspect request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let payload: Value = serde_json::from_str(&response_text(response).await).unwrap();
        assert_eq!(payload["steps"][0]["key"], "input_source");
        assert_eq!(payload["functions"][0]["qualname"], "classify");
        assert_eq!(payload["functions"][0]["displayName"], "classify");
        assert!(payload["functions"][0]["functionId"].as_str().is_some());
        assert!(
            payload["functions"][0]["entryLabel"]
                .as_str()
                .is_some_and(|entry_label| !entry_label.is_empty())
        );
        let step_texts = payload["steps"]
            .as_array()
            .expect("steps should be an array")
            .iter()
            .filter_map(|step| step["text"].as_str())
            .collect::<Vec<_>>();
        let step_keys = payload["steps"]
            .as_array()
            .expect("steps should be an array")
            .iter()
            .filter_map(|step| step["key"].as_str())
            .collect::<Vec<_>>();
        assert!(step_keys.contains(&"jit_local_plan"), "{payload}");
        assert!(
            step_texts.iter().any(|text| text.contains("BinOp(Add,")),
            "{payload}"
        );
        assert!(
            step_texts
                .iter()
                .any(|text| text.contains("function") && text.contains("runtime_params")),
            "{payload}"
        );
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "soac-inspector-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp test dir should be created");
        dir
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent dir should be created");
        }
        std::fs::write(path, contents).expect("file write should succeed");
    }

    #[tokio::test]
    async fn serves_speedscope_profiles_from_logs_dir() {
        let temp_root = temp_test_dir("speedscope-profile");
        write_file(
            &temp_root.join("web").join("index.html"),
            "<!doctype html><title>test</title>",
        );
        write_file(
            &temp_root.join("logs").join("profile.json"),
            "{\"name\":\"demo\"}",
        );
        let app = app_with_state(AppState {
            repo_root: temp_root.clone(),
            web_dir: temp_root.join("web"),
        });

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/speedscope_profile?path=logs/profile.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("speedscope profile request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            "application/json; charset=utf-8"
        );
        let body = response_text(response).await;
        assert_eq!(body, "{\"name\":\"demo\"}");

        std::fs::remove_dir_all(&temp_root).expect("temp test dir should be removed");
    }

    #[tokio::test]
    async fn rejects_speedscope_profiles_outside_logs_dir() {
        let temp_root = temp_test_dir("speedscope-profile-reject");
        write_file(
            &temp_root.join("web").join("index.html"),
            "<!doctype html><title>test</title>",
        );
        write_file(&temp_root.join("outside.json"), "{\"name\":\"demo\"}");
        std::fs::create_dir_all(temp_root.join("logs")).expect("logs dir should exist");
        let app = app_with_state(AppState {
            repo_root: temp_root.clone(),
            web_dir: temp_root.join("web"),
        });

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/speedscope_profile?path=outside.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("speedscope profile rejection should succeed");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        std::fs::remove_dir_all(&temp_root).expect("temp test dir should be removed");
    }
}
