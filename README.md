# Repository Components

The current milestone is authenticated offline `ty` contracts, actual runtime
type/function binding and interpreter enforcement; see [OPT_GOAL.md](OPT_GOAL.md).
Optimization and benchmarks are deferred. Under the 2026-08-24 (PDT)
execution-compatibility clarification, SOAC's compiled and entry-interpreter
paths preserve source semantics, safe ownership, required cleanup and installed
contracts without matching CPython's transient reference counts or implicit
finalizer/weakref schedule. The CPython backend uses ordinary CPython execution;
its specialized bytecodes and supported C APIs still enforce contracts.
The 2026-08-25 (PDT) amendments make this field-only runtime enforcement:
annotations do not impose argument, return, or dataclass-factory checks.
Selected storage writes remain checked regardless of their caller. SOAC
traceback reconstruction, frame inspection, and CPython-compatible tracing,
profiling and monitoring are excluded, including mandatory observer refusal.
Ordinary CPython frames and observers are unchanged. Source identity, exception
semantics, comprehension scoping, recursion safety and cleanup remain required;
SOAC's internal compiler counters and diagnostic logging are separate facilities.

## Rust Crates

See [Module Lifecycle](doc/MODULE_LIFECYCLE.md) for the complete dataflow and
generated dependency graph. Names below are Cargo package names.

### Important Crates

- `soac_config`
  Central typed configuration for SOAC environment variables, logging, work
  directories, optimization modes, and runtime feature flags. Runtime, JIT,
  inspector, and benchmark entrypoints should parse environment state through
  this crate instead of re-reading raw variables.

- `soac_core`
  Core data model crate for BlockPy IR, profile/counter dump formats, runtime
  function IDs, and structured pretty-printing helpers. It owns the serialized
  shapes that are shared between lowering, optimization, JIT, and inspection.

- `soac_contracts`
  Owned strict-language policy and type-fact proposals, deterministic module
  shards, signed generation manifests, and source/configuration/dependency
  verification. A verified proposal is not a runtime capability; actual
  construction and sealing must still enforce it. The offline checker is not a
  dependency of this crate.

- `soac_ir_blockpy`
  Resolved pre-optimization BlockPy instruction/module shapes, semantic
  instruction identities, constructor entry preparation, and IR validation.

- `soac_ir_typed`
  Typed instructions, value facts, resolved field/call operations, and the v3
  plan/emission vocabulary consumed by optimization and code generation.

- `soac_driver`
  Codegen-preparation orchestration layered on top of `soac_lowering`. It owns
  pre-optimization module-cache lookup/store, prepared codegen facts,
  env-configured instrumentation, and cache path/metadata helpers used by the
  runtime and optimizer.

- `soac_jit`
  Actual strict module/function/type ownership, authenticated interpreter
  loading, protected field writes, and class admission. Its retained JIT branch
  turns resolved BlockPy into Cranelift code and manages counters, constants,
  guards, and deoptimization; interpreter enforcement does not require it.

- `soac_lowering`
  Parser-to-BlockPy lowering pipeline, transformation passes, pass tracking,
  source fixtures, validation, and structured render helpers. It is where raw
  Python syntax is turned into progressively more resolved compiler IR, and it
  owns the public lowering entrypoints.

- `soac_opt`
  Optimization planning from profile evidence and cached BlockPy modules. It
  owns fact analyses, ownership/local-environment plans, typed rewrites,
  specialization decisions, and optimization-family status reporting.

- `soac_instrument`
  Profile/verify counter definitions and typed instrumentation. Counter
  observations select candidates; they do not grant strict runtime authority.

- `soac_pyo3`
  The `_soac_ext` Python extension module. It bridges startup-authenticated
  loading into ordinary CPython execution or the retained SOAC lowering/JIT
  path, and exposes runtime diagnostics without granting execution authority.

### Utility / Helper Crates

- `build_support`
  Shared build-script identity and vendored-CPython linking helpers.

- `soac_cpython`
  Embedding and test support for the selected vendored interpreter, including
  native library/search paths and Rust-side Python initialization.

- `soac_source`
  Range-preserving validation of Ruff source tokens before lowering or offline
  literal inference. `validate_source_literals` rejects unsupported surrogate
  escapes with `UnsupportedSurrogateEscape`; it does not change ordinary
  CPython strings or add parser dependencies to `soac_contracts`.

- `soac_jit_runtime`
  ABI-shaped, raw-CPython helpers compiled to CLIF for inlining into generated
  code. It has no PyO3 wrapper dependency.

- `soac_macros`
  Mechanical procedural macros for IR delegation and match boilerplate.

- `soac_inspector`
  The local web inspector and CLI tools for passes, typed plans, counters,
  CLIF/VCode, and benchmark artifacts. Inspection does not authorize execution.

`bench/pystone-rust` is a separate benchmark package, outside the main workspace.
The pinned `tools/ty` exporter also has its own workspace and lockfile; it is
not linked into the runtime.

## Codex Skills

- `analyze-pystone-perf`
  End-to-end pystone performance-analysis workflow that combines benchmark
  counters, perf capture, specialized CLIF rendering, and ranked optimization
  suggestions.

- `benchmark-compare`
  Compares two SOAC pystone benchmark result directories, creating missing
  one-off benchmark artifacts when needed and checking throughput, counters,
  and specialized CLIF differences.

- `fix-test-case`
  Focused workflow for one failing test: reproduce it, add a minimal regression
  under `tests/`, identify the root cause, implement the fix, and rerun the
  narrow check.

- `inspect-pass`
  Opens the local web inspector at a named tracked lowering pass for a concrete
  source example, useful for visually comparing transform stages.

- `python-debug`
  Uses `pdb` to step through Python scripts, continue to exceptions, and
  inspect runtime state at specific lines.

- `python-monitoring-trace`
  Uses `sys.monitoring` to trace selected Python execution events with
  include/exclude controls and optional log output.

- `run-cpython-tests`
  Runs vendored CPython regression tests through SOAC's import-hook path and
  writes structured logs for full, partitioned, or single-file regrtest runs.

- `soac-annotate`
  Profiles a small Python snippet, collects post-opt-v3 BlockPy, specialized
  CLIF, and VCode views, and prepares annotation context for explaining
  generated blocks, guards, counters, and helper calls. It defaults to the
  post-opt-v3 view unless CLIF or VCode is requested.

- `soac-profile-benchmark`
  Runs the SOAC pystone profile/verify/apply benchmark workflow and summarizes
  the generated `work/bench/` result directory.

- `soac-selfdoc`
  Refreshes component/CLI/skill inventories and the source-grounded module
  lifecycle and crate dependency graph.

- `summarize-cpython-failures`
  Summarizes CPython regrtest logs, computes file and test-case totals, and
  groups failures by likely root cause.

## Offline strict-type analysis

Interpreter-enforced strict contracts are under implementation; optimization
and benchmarks are deferred until separately requested. The complete contract
and acceptance matrix are in `doc/TYPE_DRIVEN_OPTIMIZATION.md`. Current progress
and pending evidence are recorded in
`doc/optimization-attempts/2026-08-21-type-driven-strict-contracts.md`.

Run `just ty-prepare` in the Ubuntu VM to verify the exact committed Ruff/ty
source at the tracked `vendor/ruff` gitlink. Its configured origin is
`https://github.com/adamh-oai/ruff.git`; checker changes are logical commits in
that repository, not an applied SOAC patch series. Verification compares raw
checkout bytes, modes and index entries against an independently checked-out
Git tree. It neither fetches nor repairs sources, and rejects partial/promisor
clones and untracked checker files. The one upstream notebook symlink previously
materialized by archive preparation is now an explicit regular-file portability
commit. Preparation emits the actual source path and commit identity as JSON.

`just ty -- ...` builds and runs the separate offline semantic exporter. It uses
the actual vendored checker, accepts an explicit `[tool.soac.strict]` project
policy and selected CPython 3.15 executable, and publishes authenticated module
shards without importing project modules. See [the offline tool guide](tools/ty/README.md)
for signing-key creation, `check` options, deployment authority, dependency
invalidation, and focused tests. `just ty --debug-build -- ...` uses the isolated
debug build; `just ty --update-lockfile` refreshes workspace/path dependencies in
its separate lockfile while preserving compatible locked external versions.
Use `just ty --debug-build --test-upstream ty_project --` (or
`ty_module_resolver`) for vendored upstream library tests. This uses their pinned
lockfile and verifies the committed source before and after the test run.
The root compiler workspace and exporter resolve Ruff crates from that same
submodule. Checker identity includes the actual gitlink, tree and checkout
digest; exporter identity includes its source, dependency locks and verifier.
The build-only `SOAC_TY_RUFF_REVISION` value is supplied by the verified runner
from that gitlink; it is not an environment override for source selection.
Source verification/use holds a shared checker-source lock, distinct from the
strict-fixture build-serialization lock. Stop consumers before changing the pin
or checkout. Regenerate dependency locks in a mutable review checkout, preserve
compatible external versions, and commit generated locks separately at the top.
Source preparation, authenticated artifact loading, and runtime
construction/sealing remain separate boundaries: a signed type fact is not a
live runtime capability.

The current source targets type-artifact schema 6 and strict-contract version 2.
Runtime parameter/return policy keys are removed, not accepted as disabled
aliases. Static signatures remain checker facts; selected field writes are the
runtime value invariant. Regenerate older publications for the new policy;
the ongoing native/runtime migration must be built and validated before this
source checkpoint is a working-runtime claim. Direct bases and logical MRO
entries distinguish source-class references from semantic builtin types; an
alias of builtin `object` is not identified by its spelling. Nominal leaves
explicitly belong to a source function annotation or an exact field declaration; inherited fields
retain their original declaring class and assignment. Field annotation provenance
must be present even when explicitly unresolved (`null`). Schema-1 through
schema-5 publications must be regenerated, not loaded with inferred defaults.

`soac.strict.StrictMutationError` and `StrictRuntimeUnavailableError` are aliases
of the native per-interpreter exception classes, also exported from `soac`.
Importing them does not enable strict mode. Missing or stale startup authority
raises the native `ImportError` subclass without changing it into an unrelated
Python-defined exception.

For native-linked Cargo commands in the guest, use `just --command cargo ...`.
This supplies the same selected out-of-tree CPython executable and library
paths as the test recipes; a bare Cargo invocation may instead discover the
source directory and try to link a nonexistent in-source `libpython`.

## CLI Inspection Tools

Most command-line inspection tools live in `soac_inspector` and can be run as
`cargo run -p soac_inspector --bin <tool> -- ...` inside the Ubuntu VM.
Optimization planning is library code, not a separate `soac_opt` CLI.

- `soac-ty` (separate workspace; `just ty -- ...`)
  Analyzes strict source offline and publishes authenticated type-artifact
  generations and startup descriptors. It never imports the analyzed modules.

- `soac_inspector`
  Starts the local web inspector server. It serves the interactive pass,
  BlockPy, CLIF, and typed-instruction views, binding to `HOST`/`PORT` or
  `0.0.0.0:8000` by default.

- `list_jit_functions`
  Prints the packed runtime function ID and qualified name for each lowered JIT
  function in a source file. Use this before rendering CLIF or typed
  instructions for a specific function.

- `render_jit_clif`
  Renders generated CLIF for one source file and function ID. It can render
  specialized apply-mode code, pre-inline CLIF, debug plans, CFG dot output,
  and lowered VCode.

- `render_instr_typed`
  Renders typed instruction-level JIT output for one source file and function
  ID, with the same specialized apply-mode module identity handling as
  `render_jit_clif`.

- `inspect_counters`
  Reads a `profile.bin` or `verify.bin` counter dump and prints counters,
  key-layout rows, specialization summaries, or JSON.

- `print_blockpy_module_cache`
  Reads and displays a serialized pre-optimization BlockPy cache for inspection.
  Cache contents are not trusted strict executable input.

- `precompile_blockpy`
  Uses profile counters and cached BlockPy modules to compile referenced
  modules into object files and link an offline shared library for binary
  inspection. These artifacts do not authorize runtime execution.

- `annotate_cranelift_perf`
  Correlates perf samples with SOAC JIT basic-block maps for a benchmark result
  directory, writes annotated VCode files, and prints block-level sample rows.

# Setup

## Execution environment

All testing and other project command execution must run inside the Lima
`ubuntu24` VM, in `/home/adamh.guest/soac` (or the corresponding guest path for
the active worktree). This includes setup, dependency installation, builds,
checks, formatting, code generation, Python repros, tests, benchmarks,
profiling, debugging, and development servers. Run the setup commands below
inside that guest, not on the macOS host.

VCS operations may run on the host or in the VM. Host commands that launch work
in the guest are also permitted. Do not substitute host execution when the VM
is unavailable. See [the execution policy in AGENTS.md](AGENTS.md#execution-environment-ubuntu-vm).

## Ubuntu 24.04 prerequisites

Install the C compiler and build tools, the libraries needed by the vendored
CPython build, Git, `just`, Rust's toolchain manager, `gdb` for test-hang
diagnostics, and Graphviz for the repository dependency diagram:

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential curl gdb git graphviz just pkg-config rustup \
  libbz2-dev libffi-dev libgdbm-compat-dev libgdbm-dev \
  liblzma-dev libncurses-dev libnss3-dev libreadline-dev \
  libsqlite3-dev libssl-dev libzstd-dev tk-dev uuid-dev zlib1g-dev

rustup default stable

curl -LsSf https://astral.sh/uv/install.sh -o /tmp/soac-uv-install.sh
sh /tmp/soac-uv-install.sh
. "$HOME/.local/bin/env"
```

The `rustup` Ubuntu package provides the `cargo` and `rustc` commands.
`just setup-dev-env` installs the additional nightly Rust toolchain and its
Cranelift codegen component automatically.

`ripgrep` is optional but useful for searching the repository:

```bash
sudo apt-get install -y ripgrep
```

### Enable profiling and debugger access in a trusted development VM

Native `perf` profiling, JIT-symbol attribution, and attaching `gdb` to test
processes require less restrictive kernel settings than a fresh Ubuntu image
usually provides. Install the `perf` tools for the running guest kernel:

```bash
sudo apt-get install -y linux-tools-common "linux-tools-$(uname -r)"
perf --version
```

`just setup-dev-env` also installs Inferno's `inferno-collapse-perf` tool for
the deep-profile recipes. Configure the required kernel settings persistently
inside the Ubuntu guest:

```bash
sudo tee -a /etc/sysctl.d/openai.conf >/dev/null <<'EOF'
kernel.perf_event_paranoid = -1
kernel.yama.ptrace_scope = 0
kernel.kptr_restrict = 0
EOF
sudo sysctl -p /etc/sysctl.d/openai.conf
sysctl kernel.perf_event_paranoid kernel.yama.ptrace_scope kernel.kptr_restrict
```

These settings apply to the entire guest: they allow unprivileged performance
monitoring, permit ordinary same-user debugger attachment, and expose kernel
pointer information. Use them only in a trusted, isolated development VM; do
not apply them to a shared machine or production system.

## Initialize and build the checkout

The vendored CPython and Ruff submodules must be initialized before
`just setup-dev-env`. Each tracked Git submodule pointer is the source revision
to use; the host and guest must see the same source files at `vendor/cpython`
and `vendor/ruff`. CPython must also be built before setup.
On a case-sensitive checkout, the default is an in-source shared-library build:

```bash
git submodule update --init --recursive vendor/cpython vendor/ruff
just build-python
just setup-dev-env
just test-all
```

CPython must be built on a **case-sensitive filesystem** because its `Python/`
source directory and `python` executable must coexist. A macOS checkout shared
into a Linux VM over virtiofs usually remains case-insensitive. For that setup,
keep the sources on the shared mount and select a separate guest-local ext4
build directory. Run these commands **inside the Ubuntu guest**:

```bash
git submodule update --init --recursive vendor/cpython vendor/ruff
CPYTHON_BUILD_DIR="$HOME/.local/share/soac/cpython-build" just build-python
just cpython-info
just setup-dev-env
```

A successful build saves its source/build selection in ignored
`work/cpython-selected-build.json`. Later ordinary `just` commands reuse it
without changing the guest's login configuration. To select an existing
verified build, use `just select-cpython-build /absolute/build/directory`.
Explicit path environment variables override the saved selection; a saved
selection for another source checkout is not reused.

Keep selected external builds in persistent guest storage so a VM restart does
not remove the interpreter and break its dependent virtual environments. Saving
a selection rejects resolved paths beneath `/tmp`, `/var/tmp`, or the configured
system temporary directory, including symlink aliases. Checkout-owned build and
test-fixture paths remain allowed because their lifetime is already tied to the
checkout. `$HOME/.local/share/soac/builds` is a suitable persistent guest location.
Guest disk and `/tmp` free-space reports do not represent independent host
physical capacity: the VM disk consumes host storage. Check both filesystems
before moving caches, include the peak space needed while both copies exist,
and never start a cache copy when the host cannot hold that peak.

To build a candidate without switching that saved selection, use a fresh
`CPYTHON_BUILD_DIR` with `just build-python optimized --no-select` (or
`development --no-select` / `stackref-debug --no-select`). It performs the same
source, native-extension and provenance checks;
`just select-cpython-build /absolute/candidate/build` makes
the later switch explicit. This does not keep an old build valid after its
shared source or tracked source commit changes: stop native consumers before a
source promotion, and do not mix new headers with the old interpreter/library.

Do not bind-mount a separate source checkout over `vendor/cpython`: that hides
host edits and permits the source revisions to diverge. Existing setups that
used the old source overlay must preserve both checkouts before removing it.
`scripts/migrate_cpython_shared_source.py stage` takes the backing
`--guest-source` directory and the separately verified `--host-revision`. It
stages a clean copy of the repository's pinned source under ignored `work/`
without changing either original. After reviewing that staging record and
stopping source/build users, its explicit `promote` command backs up
`/etc/fstab`, removes only the recorded source bind, unmounts without force,
preserves the old host tree, and promotes the shared source. The original
guest source/build is retained. Promotion requires permission to change the
mount and fstab; rebuilding uses a **new** guest-local build directory.

`vendor/cpython` is pinned to the complete native implementation commit from
`https://github.com/adamh-oai/cpython.git`. Maintain native changes as logical
commits there; generated interpreter cases belong in a separate top commit with
the regeneration commands. There is no maintained patch manifest or applying
build prerequisite. `just prepare-cpython` and `just prepare-cpython --check`
both verify without fetching, resetting, applying patches or changing sources.
The common committed-source verifier checks raw bytes, executable/link modes,
and index entries against immutable Git objects using canonical Git attribute
conversion. Local filters, index flags or uncommitted ignore rules cannot hide
source changes. CPython permits only its explicit build receipts and outputs
ignored by committed rules; those build outputs are not source authority.
In Jujutsu workspaces, the pin comes from the recorded `@` tree, not Git's
parent index. A failed Jujutsu query fails verification; plain Git checkouts use
the resolved index pin. This JJ version does not snapshot submodule updates:
integrate the gitlink change into the JJ revision and verify its actual tree
before building, rather than relying on `git add` or submodule checkout alone.

Pin-query failures retain the bounded, escaped command, return code and output,
plus the checkout and submodule paths; they never retry against the Git index.
The checker runner identifies the verification/build/execution phase. Each
strict integration fixture keeps its own combined checker log under
`work/logs/strict-integration-checker-build-*.log`, including failed and timed-out
invocations, so a later worker cannot overwrite the original diagnostic.

Build provenance schema 2 records the actual gitlink, tree, checkout digest,
build paths, mode and executable/library identity. Legacy patch-generation
receipts are rejected before starting the old interpreter: rebuild rather than
restamping a receipt. The lock remains under `work/cpython-patches/` solely to
interoperate with earlier in-flight preparers; neighboring old manifests are
historical evidence, not authority.

CPython's `sys.version` revision label is compiled build metadata, not the
source-verification result. Its current `GITTAG` command uses `--git-dir`
without selecting the source worktree, so an out-of-tree build can report
`-dirty` even when the native checkout matches its gitlink exactly. Use the
canonical source and schema-2 runtime checks; that label never permits ignoring
a failed source check. Future version generation should explicitly select the
source worktree with `git -C` or `--work-tree`. Do not restamp provenance or
rebuild a verified runtime solely to change this label.

Use `just regenerate-cpython-cases --check` to regenerate and compare the
generated-only pinned top commit in a disposable checkout. For reviewed native
development, use `--source /shared/staging/checkout --revision <logical-commit>
--output work/cpython-generated/<review-name>` to emit generated files for a
separate commit. This never edits, commits or promotes the selected source.

The 2026-08-23 PDT migration keeps both repositories' new commits local at the
user's request. Independent local checkout reproduction is required; fetching
these new pins through the configured remotes or CI remains unverified until
publication is separately requested. Preserve the local repositories meanwhile.

`just test-source-tooling` runs the focused source/pin/build-wrapper tests with
guest system Python and the repository's pytest version, without requiring a
valid selected native build. It is a bootstrap tooling gate, not native
enforcement validation or a substitute for `just test-all`. It retains a unique
log under `work/logs/` for every run. Git and Jujutsu are required for its real
repository fixtures.

`just build-python` (equivalently `just build-python optimized`) builds shared,
nondebug CPython with PGO/LTO. For faster native iteration, use a separate
guest-local build directory with `just build-python development`: it keeps the
same nondebug shared ABI but omits PGO/LTO. The selected mode is part of the
build provenance. Benchmark and pyperformance entrypoints require an
`optimized` build; development and stackref-debug results are not performance
evidence.

Use the nondebug `development` mode for iterative Rust/JIT checks. The JIT's
generated refcount support rejects `Py_REF_DEBUG`, so a StackRef-debug native
build is not a supported interpreter for `cargo check -p soac_jit --tests` or
Rust/JIT execution. Keep the guard and use StackRef-debug for native C/Python
reference-handle tests instead.

CPython prepare/build commands sharing a source directory must run serially,
even with separate build directories or modes. A build holds the source lock;
concurrent source/runtime preflights fail explicitly rather than checking a
generation while it may change. Wait for the build instead of bypassing the lock.

Guest-local build directories improve I/O isolation, but do not add physical
disk capacity: the sparse Lima disk and shared checkout use the same host
volume. Check free space on both before a large build or copy. A cache move
implemented as copy-then-delete temporarily needs the entire duplicate plus a
reserve. Host-deleted shared files may remain allocated while VirtioFS holds
them open; verify actual reclaimed space. After disk-full/I/O errors, pause
project execution and revalidate source/build hashes before using their results.

For native reference-handle diagnostics, use a fresh guest-local build directory:

```bash
CPYTHON_BUILD_DIR="$HOME/.local/share/soac/builds/stackref-debug" just build-python stackref-debug --no-select
CPYTHON_BUILD_DIR="$HOME/.local/share/soac/builds/stackref-debug" python3 scripts/cpython_environment.py check-runtime --require-mode stackref-debug
```

`stackref-debug` configures `--with-pydebug`, explicitly sets
`CPPFLAGS=-DPy_STACKREF_DEBUG=1`, uses `-O0 -g`, and omits PGO/LTO. This mode
deliberately replaces ambient `CPPFLAGS`; optimized and development retain
their existing environment handling. Neither `Py_DEBUG` alone nor `-X dev`
proves native StackRef handle checking is enabled.

Signal/watchdog traceback readers cannot consult the debug handle table or
require an attached thread state. GIL-enabled StackRef-debug frames therefore
carry a diagnostic-only borrowed executable pointer, atomically published and
cleared with the existing code support. It adds no Python owner or GC edge;
normal execution and traversal still use checked handles. This changes only
the debug frame size, so native probes must use the matching build's layout
query rather than a fixed header size. Diagnostic reads retain CPython's
best-effort freed-frame checks, not a general concurrent-snapshot guarantee.

Before publishing provenance or selecting a debug build, the actual candidate
interpreter runs an isolated proof of its executable, source/build paths,
loaded libpython identity, debug configuration and `_Py_stackref_*` debug-only
exports. The probe looks up exports without invoking handle operations. On
Linux it checks the mapped `INSTSONAME` and its `LDLIBRARY` hard-link identity.
The versioned `stackref_debug` proof is recorded and rechecked only for this
mode; existing nondebug runtime records keep their shape and remain subject to
their unchanged freshness checks. `just cpython-info` includes the proof for a
verified debug build. No debug interpreter is accepted by optimized benchmark
readiness. A debug build does not itself prove that native reference controls
pass; those must run separately with matching native headers and compile flags.

All modes verify the selected committed source before configuring
and hold the source lock throughout the build. After configure creates
`pyconfig.h`, they syntax-check public `Python.h` as C++ using `CXX` from that
build's Makefile, before compilation or PGO training. A header failure stops
the build without publishing provenance or changing the saved selection.
They record the source
gitlink revision, tree and actual checkout fingerprint,
compiled source/build paths, and interpreter/shared-library identity in
`.soac-cpython-build.json` inside the build directory. Runtime preflights reject
a hidden source overlay, a stale submodule checkout, a mismatched build path,
or sources/artifacts changed since the recorded build. A preexisting build
without this record needs one `just build-python` rebuild. `just cpython-info`
reports the source pin, mount, selected interpreter, and verification result
without claiming that an unverified existing binary matches the source.
The build also imports `_ctypes`, `_testcapi`, and `_testinternalcapi` before
publishing its provenance or selecting the interpreter: a successful `make`
alone does not show that CPython's extension modules are usable.

After editing CPython bytecode definitions, use
`just regenerate-cpython-cases --source <shared-staging-checkout>
--revision <logical-commit> --output work/cpython-generated/<review-name>` to
regenerate review files in a disposable guest checkout of the exact logical
commit, without editing selected sources. Commit those generated files in a
separate top commit in CPython and record the regeneration command in its
message. `just regenerate-cpython-cases --check` verifies the pinned
generated-only top commit against its logical parent. There is no maintained
generated patch file to apply.

The workflows in `AGENTS.md` also require Jujutsu for version-control
operations; install it with `cargo install --locked jj-cli` if `jj` is not
already available on the machine where those operations run. The pystone
`just benchmark` recipe itself runs `jj status` and `jj log`. Since benchmarks
run inside the Ubuntu guest, `jj` must also be installed in the guest even
when other VCS operations run on the macOS host.

The raw CLIF input crate, `soac_jit_runtime`, is deliberately separate from the
main Cargo workspace. Use `just test-jit-runtime` for its focused tests;
`just test-all` includes them as a serial stage. Both `just fmt-rust` and
`just fmt-rust-check` accept that package alongside ordinary workspace packages.
After a successful build, `just test-all` attempts workspace Rust tests, raw
runtime tests, and pytest in that order even if an earlier test stage fails. It
records each stage's status and returns the first nonzero status; a failed
build still prevents tests from starting. The workspace Rust stage uses
`--no-fail-fast` to report later test-target failures too, with one compiler job
and one test-harness thread. Concurrent large debug-test linkers
can otherwise exhaust the 12 GiB Lima VM before any tests execute.
For focused Cargo commands from a fresh guest launcher, use
`just --command cargo ...` so the selected CPython executable and library
directory are exported; the shared `vendor/cpython` directory is source, not
the selected out-of-tree build.
Extension installation and the fast runtime recipes ask Cargo for its actual
target directory, including `CARGO_TARGET_DIR` and Cargo configuration. Embedded
Rust tests use the profile directory recorded by Cargo when their binary was
built, not a runtime environment override or an assumed `target/debug`. Build
`soac_pyo3` in that same target/profile before embedded tests; a missing matching
extension is an explicit setup failure, even if an older staged library exists.

Runtime integration tests use `tests._strict_integration.create_strict_project`
to analyze explicitly selected source modules with the pinned offline checker,
then `StrictProject.run` to launch the selected interpreter with the generated
native startup configuration. Selected sources must include the real
`from __future__ import strict` opt-in; the shared helper does not insert it.
For delimiter-based cases, `run_case` also
checks actual module-seal/source/generation diagnostics before executing the
validation tail outside the sealed module. `SOAC_MODULE_ENABLED` and test mode
flags do not grant strict authority. Keep ordinary controls on
`tests._integration.stock_module`; the old in-process `soac`/`entry` helpers
fail explicitly until their callers are migrated.

An integration validation tail either declares one synchronous
`validate_module(module)` or `validate(module)` function, which the dispatcher
calls exactly once, or contains ordinary top-level assertions. Do not mix both
forms. Validation mode flags are available only in the validation globals, not
as mutations of the actual module. Unexpected failures remain failures;
unsupported behavior needs an explicit, reviewed per-case expectation rather
than a global exception-message-to-xfail rule.
The [strict test migration inventory](doc/STRICT_TEST_MIGRATION.md) records the
legacy coverage gaps, reviewed cohorts, and unresolved compatibility cases.

New source-level tests can use the [single-file scenario format](doc/STRICT_SCENARIO_TESTS.md)
in `tests/strict_scenarios/`: `# module:name` sections define the analyzed modules,
then `# ok` and `# raise:Exception` sections run in fresh authenticated processes.
Only the final top-level statement is covered by a `raise` expectation; module
setup and earlier statements must succeed. The adapter adds the strict opt-in
explicitly and uses the existing real-checker/native-startup helper. Run these
with `just pytest-fast --require-batch-runner tests/test_strict_scenarios.py`.

The development dependency group includes Pydantic, Django, and SQLAlchemy for
the real framework-fallback compatibility tests. These cover model construction,
descriptors, validation/coercion, dictionary replacement, and instrumentation
inside an authenticated strict containing module; the frameworks' own
implementations remain untransformed.
After changing these dependencies, regenerate `soac_py/uv.lock` with
`just --command uv lock --project soac_py --python .venv/bin/python`, then run
`just update-venv` once with network access before offline test refreshes.
Native source builds in `.uv-cache` are excluded from the SOAC Cargo workspace;
give concurrent dependency builds a separate `CARGO_TARGET_DIR`.
The Django test dependency stays on the 5.2 LTS line: Django 6.1's ordinary
startup calls `inspect.getfullargspec(annotation_format=...)`, which the pinned
CPython 3.15 alpha does not yet provide. This is a stock-baseline limitation,
not a strict-runtime fallback result.

Lima does not automatically copy host package-index credentials or proxy
settings into guest commands. Use the repository helper to launch project
commands from the host and forward the required configuration:

```bash
python3 scripts/run_lima_with_host_environment.py \
  --instance ubuntu24 \
  --workdir /home/adamh.guest/soac \
  -- just pyperformance-compare all 1
```

The helper passes only package-index, proxy, certificate, and explicitly
requested `PYPERFORMANCE_INHERIT_ENV_EXTRA` settings over stdin. Host-loopback
proxy addresses are rewritten to `host.lima.internal`; credential values never
appear in guest command arguments, helper output, or temporary files. Generic
`ALL_PROXY` / `all_proxy` settings are excluded by default because SOCKS proxies
can interfere with benchmarks that issue local HTTP requests; explicitly opt
them in with `PYPERFORMANCE_INHERIT_ENV_EXTRA` only when required.

`setup-dev-env` reuses an already-installed nightly Rust toolchain and Cranelift
codegen component rather than upgrading them on every run, because a nightly
refresh forces rebuilds. It also installs the `ruff` command with uv. The repo
keeps uv and XDG state under the working tree (`.uv-cache`, `.uv/`, `.xdg/`,
and `work/tmp/`) and puts the repo-local uv tool bin directory on `PATH`, so
later test and benchmark recipes can run uv in offline mode instead of fetching
through the sandbox. The Python dependency lockfile is `soac_py/uv.lock`;
freshness checks also honor a root `uv.lock` when one exists. Cargo uses the
caller's normal `CARGO_HOME`.

For jj worktrees, `just setup-dev-env` infers the parent checkout from a
file-backed `.jj/repo` when possible. Set
`SOAC_PARENT_REPO=/path/to/parent/checkout` to override that inference or when
the parent cannot be inferred. The parent checkout owns `work/` as a regular
artifact directory, and the setup recipe symlinks `vendor/cpython`, `work/`,
`.uv-cache`, `.uv/`, and `.xdg/` from the parent checkout so temporary
worktrees can reuse the already-fetched offline state and shared benchmark
artifacts.

## Documentation Site

The Markdown files under `doc/` can be rendered as a local Astro Starlight
site. The machine running the recipes needs Node.js, npm, and `just`:

```
$ just docs-build
$ just docs-serve
```

`docs-build` always runs `docs-install` first to install the Node dependencies
declared in `package.json` without rewriting `package-lock.json`, then writes
the generated site to ignored `work/docs-site/`. Run `just docs-install`
directly when you only want to install the dependencies.
`docs-serve` serves it on `0.0.0.0:8001` by default; pass a port to override
it, for example `just docs-serve 9000`.



# Environment Variables

This repo consults a number of environment variables directly. The list
below is the user-facing set that changes runtime behavior, profiling,
benchmarking, test wrappers, or the local web UI. Pure `Justfile`
plumbing such as `REPO_ROOT`, `VENV_DIR`, `WEB_DIR`, and similar helper
exports are intentionally omitted here.

## Local Tooling

- `CPYTHON_SOURCE_DIR=/path/to/shared/pinned-cpython`
  Optional explicit source checkout, defaulting to `vendor/cpython`. The
  selected checkout must match the repository's tracked submodule revision.
  This is useful for a separately staged shared source tree or build worktree;
  it does not fix or hide an existing overlay on `vendor/cpython`.
  `just cpython-info` reports both the selected source mount and the vendored
  source mount. Stdlib resolution and embedded Python use the selected source.

- `CPYTHON_BUILD_DIR=/guest-local/path/cpython-build`
  Build directory for `just build-python`. An explicit `CPYTHON_LIB_DIR` is
  the next fallback; otherwise the repo-local saved selection is reused when
  it matches the selected source, then `CPYTHON_SOURCE_DIR` is the fallback.
  Sources remain in the selected source checkout; use a case-sensitive
  guest-local directory when that checkout is shared from macOS. Successful
  builds and `just select-cpython-build` save the selection under ignored
  `work/`, so later guest commands need no repeated export or login changes.

- `CPYTHON_BIN=/path/to/python`, `CPYTHON_LIB_DIR=/path/to/build`
  Optional explicit interpreter and shared-library/build directory overrides.
  They default to `CPYTHON_BUILD_DIR/python` and `CPYTHON_BUILD_DIR`.
  The selected paths flow through venv creation, tests, benchmarks, Rust
  linking, and embedded-Python extension-module lookup. `build-python` requires
  its normal adjacent executable/library layout; runtime preflights verify
  that the interpreter was built from the current shared source and selected
  library directory.

- `SOAC_PARENT_REPO=/path/to/parent/checkout`
  Optional override for `just setup-dev-env` inside a jj worktree. The recipe
  normally infers the parent checkout from a file-backed `.jj/repo`; the parent
  checkout owns `work/` as a regular artifact directory, `vendor/cpython`, and
  the shared offline state symlinked into the worktree: `.uv-cache`, `.uv/`,
  and `.xdg/`.

- `SOAC_PRECOMPILED_LIBRARY=/path/to/libsoac_precompiled.so`
  Retired. Any present value, including an empty value, is rejected; unset the
  variable. The old library lookup did not authenticate strict template,
  policy, or native ABI identity. Neither executable entries nor cached native
  object images from these libraries are loaded by the runtime. Offline object
  emission remains available for inspection, not as an execution cache.

- `UV_OFFLINE=1`
  Normal test and benchmark recipes set this for uv-backed venv refreshes after
  `setup-dev-env` has populated the repo-local cache and installed tools. Use
  plain `just update-venv` or rerun `just setup-dev-env` when dependency changes
  intentionally require network access.

## Import Hook And Runtime Behavior

SOAC Rust runtime/JIT environment variables are parsed into a typed config once
at the relevant entrypoint. Unset variables use documented defaults; present
typed variables must use recognized values. Boolean knobs accept `1`, `true`,
`yes`, or `on` for true and `0`, `false`, `no`, or `off` for false.

- `SOAC_MODULE_ENABLED=path:/absolute/or/relative/root[,path:/another/root]`
  In `def _module_is_enabled`, at
  [soac_py/src/soac/import_hook.py](soac_py/src/soac/import_hook.py),
  restrict the import hook to resolved source paths under the listed
  file-tree roots. When unset, the hook asks the native loader about every
  supported source import. Only an authenticated startup-selected strict
  module is transformed; ordinary imports keep their original source or frozen
  loader and native CPython execution, without SOAC profiles or JIT metadata.
  Module names such as `soac.runtime` do not bypass this rule. Compiler-owned
  intrinsic operations require their own explicit provenance.

- `SOAC_COMPILE_MODE=eager`
  In `fn eager_clif_compile_requested`, at
  [crates/soac_pyo3/src/jit_runtime.rs](crates/soac_pyo3/src/jit_runtime.rs),
  eagerly compile direct CLIF/JIT entries for outermost transformed modules
  before the lowered module body runs, including nested callable bodies that do
  not yet have a live Python function object. Later function-instance
  registration attaches those ready entries instead of waiting for first
  execution.

- `SOAC_EXEC_TRACE=<selector>`
  In `SoacEnvConfig::from_env`, at
  [crates/soac_config/src/runtime.rs](crates/soac_config/src/runtime.rs),
  enable basic-block tracing. Accepted forms are:
  - `all`, `1`, `*`, or empty selector: trace all functions
  - `<exact-qualname>`: trace one function
  - append `:params` to include block parameters

- `SOAC_LOG=<tracing-filter>`
  Controls SOAC Rust diagnostic logging. The filter portion uses
  `tracing-subscriber` syntax, for example `SOAC_LOG=trace` or
  `SOAC_LOG=soac_jit=info,soac_blockpy=trace`. Append
  `;json=/path/to/events.jsonl` to write tracing JSONL to that file
  instead of formatted stderr. Module-load timings are emitted as
  `soac_module_load` tracing events, and JIT-codegen timing is emitted
  through `soac_jit_codegen`. Apply-mode indexed specialization hit/fallback
  summaries and deopt-entry summaries are emitted through
  `soac_specialization_runtime`. BlockPy module-cache hits and stores are emitted through
  `soac_blockpy_module_cache`.
  Enable them with
  `SOAC_LOG=soac_module_load=info,soac_jit_codegen=info,soac_specialization_runtime=info`.
  When `SOAC_LOG` is unset and `SOAC_WORK_DIR` is set, SOAC writes
  default JSON events to `$SOAC_WORK_DIR/events.jsonl`, including the
  `soac_specialization_runtime` target.

- `SOAC_PYTEST_TRACE=1`
  Enables the verbose pytest module-load and JIT-codegen JSON trace for
  `just pytest ...` and `just test-all` when `SOAC_LOG` is unset. The trace is
  disabled by default for green correctness runs. It writes to
  `SOAC_PYTEST_EVENTS_LOG` when set, otherwise `work/logs/pytest_events.jsonl`.

- `SOAC_PYTEST_EVENTS_LOG=/path/to/events.jsonl`
  Overrides the pytest trace output path used when `SOAC_PYTEST_TRACE=1`.

- `SOAC_PYTEST_BATCH_TIMEOUT=<seconds>`
  Per-batch timeout for the parallel pytest runner used by `just pytest ...`,
  `just pytest-fast ...`, and `just test-all`. The default is `300` seconds.
  These parallel controls apply to selector-only invocations. Passing pytest
  options such as `-q` or `-v` uses serial passthrough instead, without per-batch
  timeouts or progress reports.
  Use `just pytest-fast --require-batch-runner tests/test_test_all_workflow.py` to reject
  that fallback before collection. The guard accepts file/node selectors and
  positive `PYTEST_NUMPROCS` (or `auto`), including a one-worker batch run; it
  also refuses empty collection without launching pytest again. It is consumed
  by the runner, not passed to pytest, and works with `just pytest` too.
  Batches stay file-local and contain at most four collected tests; suite
  growth cannot enlarge them beyond that ceiling.
  Integration cohorts execute only this worker's collected case/mode pairs;
  their reviewed analysis sources, dependencies and admission checks are unchanged.
  Set to `0` to disable the timeout. Interrupting the parallel runner with
  SIGINT or SIGTERM cancels queued batches and terminates all active worker
  process groups, including descendants whose original worker already exited.
  Cleanup gives the groups one shared five-second grace period before SIGKILL
  and preserves their captured diagnostics.

- `SOAC_PYTEST_PROGRESS_INTERVAL=<seconds>`
  Interval for live "currently running pytest batch" reports from the parallel
  pytest runner. The default is `10` seconds. Set to `0` to disable live batch
  reports.

- `SOAC_RUN_SLOW_TESTS=1`
  Includes pytest tests marked `slow` in `just pytest ...` and `just test-all`.
  Slow tests are deselected by default so ordinary green runs skip intentionally
  expensive broad-mode coverage. When invoking pytest directly, pass
  `--run-slow` to include the same tests.

- `SOAC_CRANELIFT_OPT_LEVEL=none|speed|speed_and_size`
  Override the Cranelift optimization level used by the process JIT.
  Normal runtime and benchmark runs default to `speed_and_size`. The
  `just pytest`, `just test-all`, and `just run-cpython-tests` recipes default
  this to `none` unless the caller already set it, because correctness tests are
  latency-sensitive and should not spend cold-start time optimizing import-time
  helper code.

- `SOAC_JIT_COMPILE_WORKERS=<positive-integer>`
  Cap the number of worker threads used to compile functions inside one
  reserved process-JIT batch. Unset defaults to `min(available_parallelism, 4)`;
  set `1` to force single-worker compilation for timing comparisons.

- `SOAC_BACKGROUND_JIT=0`
  Disable import-time background JIT compilation. Background JIT is enabled by
  default for normal runtime use; pytest and CPython regression-test recipes
  default it off so correctness runs do not accumulate asynchronous compile
  work across many short-lived test modules.

- `SOAC_JIT_EMIT_REFCOUNTS=0`
  Disable generated JIT INCREF/DECREF emission by inlining the SOAC runtime
  refcount helpers as no-ops. Refcount emission is enabled by default; only
  `0`, `false`, `no`, or `off` disable it. This is an intentionally unsound
  performance experiment knob and leaks Python references. It also disables
  guard-miss deopt replay, because the replay interpreter depends on normal
  owned-reference bookkeeping from generated code.

- `SOAC_JIT_HANDLE_PENDING_CHECKS`
  Generated JIT loop backedges check CPython's pending-event bits by default and
  call `_Py_HandlePending` only when an event is waiting. This preserves
  pending calls, signal handling, thread handoff, cyclic garbage collection,
  and async exceptions without an unconditional helper call in hot loops. Set
  to `0`, `false`, `no`, or `off` only for an intentionally unsafe diagnostic;
  disabling the checks can prevent other Python threads from running.

- `SOAC_JIT_BB_MAP=1`
  Emit the detailed `$SOAC_WORK_DIR/jit-bb-map.jsonl` per-basic-block artifact
  used by perf/VCode annotation. This is disabled by default because very large
  generated functions can spend seconds serializing the map. Aggregate code-size
  reporting stays on through `$SOAC_WORK_DIR/jit-code-summary.jsonl`.

## Counters And Specialization

- `SOAC_WORK_DIR=/path/to/work-dir`
  Runtime work directory for generated process-local output. In normal
  specialization workflows this directory contains:
  - `profile.bin`: specialization input recorded by the profile pass.
  - `verify.bin`: countered output recorded by the verify pass.
  - `events.jsonl`: default tracing JSONL when `SOAC_LOG` is not
    set.
  - `jit-code-summary.jsonl`: compact aggregate generated-code summaries used
    by benchmark reports.
  - `modules/`: root for inspectable pre-optimization BlockPy caches. Cached modules
    use stable per-module artifact paths such as
    `project/pkg/submod/mod.blockpy`, with source hash and build identity
    stored as cache metadata. Strict runtime imports lower verified source
    afresh; writable IR caches cannot replace authenticated executable input.

- `SOAC_OPT_MODE=none|profile|verify|apply`
  Select the runtime specialization phase:
  - `none`: execute admitted strict code without profile-guided specialization,
    do not instrument specialization counters, do not read `$SOAC_WORK_DIR/profile.bin`,
    and do not write counter dumps. This is equivalent to leaving
    `SOAC_OPT_MODE` unset, but is useful when a parent environment may
    already set it.
  - `profile`: run unspecialized, instrument specialization input
    counters, and write `$SOAC_WORK_DIR/profile.bin`.
  - `verify`: read raw profile evidence from `$SOAC_WORK_DIR/profile.bin`,
    apply v3 decisions while building `InstrTyped`, instrument specialization
    input counters again, and write `$SOAC_WORK_DIR/verify.bin`. Verify mode
    exercises indexed store fast paths so their hit/fallback counters measure
    the specialized steady-state path.
  - `apply`: read raw profile evidence from `$SOAC_WORK_DIR/profile.bin`,
    apply v3 decisions while building `InstrTyped`, and emit no specialization
    counter dump files.
    When event logging is enabled through `SOAC_LOG` or the default
    `$SOAC_WORK_DIR/events.jsonl`, apply mode still records in-process
    indexed specialization hit/fallback and deopt-entry counts long enough to
    emit `soac_specialization_runtime` summary events at module teardown.
  Set `SOAC_WORK_DIR` for any mode that reads or writes counters. Leave
  `SOAC_OPT_MODE` unset, or set it to `none`, for the
  unspecialized/no-counter path. These modes do not optimize ordinary modules
  or relax any installed strict contract.

- Runtime optimization uses the typed v3 path. `verify` and `apply` build the
  JIT module by lowering the verified source's pre-optimization BlockPy module to
  `TypedBlockPyModuleShape`, then applying v3 decisions from raw
  `profile.bin` evidence during typed JIT planning. Precompile uses the same
  raw profile evidence and cached pre-optimization BlockPy modules; there is no
  serialized optimization-plan artifact between profiling and codegen.

Notes:

- In normal workflows set one `SOAC_WORK_DIR` for the whole multi-pass
  run and change only `SOAC_OPT_MODE`.
- `SOAC_ENABLE_PROFILED_COLD_BLOCKS=1` replays `block_entry` counters
  from `$SOAC_WORK_DIR/profile.bin` as Cranelift `cold` block hints in
  `verify`/`apply`. This stays disabled by default; `profile` and
  `verify` only insert the underlying `block_entry` counters when this
  flag is enabled.
- Optimization mode does not establish frozen builtins, immutable ordinary
  objects, or permission to bypass strict write barriers. Selected fast paths
  still require their actual runtime capabilities or guards.

## Perf And Benchmarking

Benchmark sources live under the tracked `bench/` directory. Generated
benchmark results and other local artifacts live under the ignored `work/`
tree, with pystone benchmark runs writing to `work/bench/`.

- `just nqueens-slice <slice> <queen-count> [loops] [opt_mode] [work_dir]`
  Run one repo-local N-Queens-derived slice through the transformed runtime.
  The available slices isolate the `permutations()` tuple consumer, the two
  diagonal `set(genexpr)` consumers, their recomposed search loop, and the full
  `list(n_queens(...))` consumer. Each transformed run imports only that slice's
  module, so optimizer/JIT traces stay local to the selected repro. Use
  `opt_mode=profile` and then
  `opt_mode=apply` with the same bare fifth-argument work-dir path when you
  want a small staged specialization repro outside pyperformance, for example
  `just nqueens-slice diagonal_set_consumers 8 1 profile work/bench/nqueens-slices/diag`.

- `just nqueens-slice-compile <slice> <queen-count> [loops] [opt_mode] [work_dir]`
  Import the same N-Queens slice module, run only the selected slice once with
  a tiny compile-seed input of `1`, then exit. Use this with an existing profiled
  `work_dir` to trigger the selected apply-mode compile path without paying for
  the full benchmark runtime or compiling unrelated sibling slices.

- `just nqueens-slice-stock <slice> <queen-count> [loops]`
  Run the same slice on plain vendored CPython for sanity checks or rough local
  timing comparison.

- `just nqueens-slice-release <slice> <queen-count> [loops] [opt_mode] [work_dir]`
  Stage the `target/release-ext` SOAC extension before running an isolated
  N-Queens slice. Unlike `just nqueens-slice`, this recipe never stages the
  debug extension. It defaults to the production `speed_and_size` Cranelift
  optimization level, keeps refcounts enabled, and uses the existing benchmark
  CPU-mode wrapper for both stock CPython and SOAC. Run `stock`, `profile`,
  `verify`, and `apply` with the same bare fifth-argument work directory;
  verify and apply require that directory's `profile.bin`. For an
  apples-to-apples, checked eight-queen comparison:

  ```bash
  just nqueens-slice-release full_nqueens_list_consumer 8 10 stock \
    work/bench/nqueens-slices/release/full

  just nqueens-slice-release full_nqueens_list_consumer 8 1 profile \
    work/bench/nqueens-slices/release/full

  just nqueens-slice-release full_nqueens_list_consumer 8 1 verify \
    work/bench/nqueens-slices/release/full

  just nqueens-slice-release full_nqueens_list_consumer 8 10 apply \
    work/bench/nqueens-slices/release/full
  ```

  Both stock and transformed full slices validate the same 92-solution result
  and report `elapsed_s` and `iterations_per_s` for the workload itself. Do not
  call `just nqueens-slice` between release phases: its debug-runtime preflight
  deliberately restages the debug extension.

- `just nqueens-slice-perf <stock|apply> <slice> <queen-count> [loops] [work_dir] [output_prefix]`
  Capture Linux native perf for identical stock and release-specialized
  N-Queens slices without pyperformance, Inferno, or Speedscope. The release
  extension is built and staged before recording; apply mode consumes the
  existing profiled work directory. Recording uses the portable `cpu-clock`
  event with a fixed 1024-page (4 MiB) mmap buffer and rejects captures without
  actual samples, including empty JIT-injected results. It fails if the recorder
  reports lost samples or chunks, or if the symbol report contains nonzero lost
  samples. Both modes first run the slice's one-queen
  `--compile-only` warmup, stop the real benchmark Python process, and attach
  `perf` only for the measured workload; the existing CPU-mode wrapper remains
  active. Defaults are `PERF_FREQUENCY=99`,
  `PERF_CALL_GRAPH=dwarf,65528`, and `PERF_PERCENT_LIMIT=0.5`. Run the release
  profile command above first, then capture comparable workloads:

  ```bash
  just nqueens-slice-perf stock full_nqueens_list_consumer 8 10 \
    work/bench/nqueens-slices/release/full

  just nqueens-slice-perf apply full_nqueens_list_consumer 8 10 \
    work/bench/nqueens-slices/release/full
  ```

  Reports default to `work/logs/nqueens-slices/` and include `<slice>-<mode>.data`,
  `<slice>-<mode>_record.txt`, `<slice>-<mode>_report.txt`,
  `<slice>-<mode>_by_dso.txt`, `<slice>-<mode>_by_dso_symbol.txt`, and
  `<slice>-<mode>_callgraph.txt`. The apply capture also enables `SOAC_JIT_BB_MAP`
  for attribution and attempts to write `<slice>-apply.injected.data`; if JIT
  injection is unavailable, reports are generated from the original perf data.
  Leave `SOAC_JIT_BB_MAP` disabled in the separate headline-throughput runs.

- `just benchmark`
  The default benchmark recipe runs the transformed profile, verify,
  and specialized apply passes and writes the raw result directory under
  `work/bench/{change_id}_{commit_id}` for one-off runs or `work/bench/{change_id}`
  for finalized runs. It always records and prints the actual current `@`
  revision that it executed, so switch revisions first with `jj edit <rev>` if
  you want to benchmark some revision other than the current checkout. By
  default it keeps the benchmark log, raw counter files (`profile.bin`,
  `verify.bin`, `events.jsonl`), and the revision-scoped BlockPy module cache
  under `counters/modules`; it does not run `perf` and it does not build
  inspector-based counter/CLIF artifacts. The specialized apply phase reports
  both the default
  refcounts-enabled throughput and an additional unsound
  `SOAC_JIT_EMIT_REFCOUNTS=0` diagnostic throughput.

- `just benchmark-deep-profile`
  Run `just benchmark`, then add the heavier follow-on artifacts in the
  same result directory: counter/specialization text dumps, rendered
  specialized CLIF/VCode/CFG, `perf` capture, and perf-annotated VCode. The
  follow-on perf run enables `SOAC_JIT_BB_MAP=1` so block-level annotation has
  the full detailed map without charging ordinary benchmark runs for it.

- `just benchmark-deep-profile-from-profile <result-dir>`
  Start from an existing result directory with `counters/profile.bin`,
  rerun only the verify pass to produce `verify.bin`, then add the same
  deep-profile artifacts without rerunning the profile pass. The added perf run
  enables `SOAC_JIT_BB_MAP=1`.

- `just pyperformance [stock|soac|soac-single] [output] [benchmarks] [extra pyperformance run args...]`
  Run the pyperformance suite against the vendored CPython executable. The
  `stock` mode runs plain CPython and only prepares the Python environment; it
  does not build or stage the SOAC extension. The default `soac` mode prepares
  the release SOAC extension, runs pyperformance once with
  `SOAC_OPT_MODE=profile`, then runs it again with `SOAC_OPT_MODE=apply`; the
  requested `output` is the apply result, and the profile pyperf result is
  written beside it with a `.profile.json` suffix. Use `soac-single` for
  one-pass debugging; it honors the caller's `SOAC_OPT_MODE` and defaults to
  `none`.

  The unchanged-environment fast path reuses the existing `.venv` when its
  `.soac-ready` marker, Python and pyperformance executables, vendored CPython,
  `soac_py/pyproject.toml`, and `soac_py/uv.lock` remain current; a root
  `uv.lock` is also checked when present. SOAC modes reuse
  `target/release-ext/lib_soac_ext.so` until relevant Rust sources, Cargo
  manifests/locks, build scripts, Rust toolchain/Cargo configuration, or
  CPython inputs become stale. A missing or incorrectly staged extension
  symlink is repaired without invoking Cargo when the release artifact is still
  current. Missing or stale inputs fall back to the normal offline venv refresh
  or release build automatically.

  SOAC modes inject a recipe-local `sitecustomize` into pyperformance worker
  subprocesses and install `soac.import_hook` before benchmark imports. When
  `output` is omitted, final results are written to
  `work/pyperformance/{stock,soac}-<timestamp>.json`, and pyperformance's own
  benchmark virtual environments are created under `work/pyperformance/venv/`.
  A repo-owned pyperformance runner caches successful benchmark-environment
  preparation, so repeated profile/apply passes and comparison rounds skip
  unchanged pip install/freeze work. The first encounter, a missing benchmark
  environment, or changed benchmark dependencies, interpreter, environment,
  or lock inputs still uses the normal upstream setup. Initial setup may
  install benchmark-specific dependencies even though SOAC's own venv refresh
  runs offline. Declarative entries in `scripts/pyperformance_local_packages.json`
  prepare a benchmark's pinned, original local packages in both stock and strict
  environments before offline analysis or worker execution. This includes the
  suite's own vendored `lib2to3` on Python 3.13+. Preparation retains a deterministic
  source archive and verifies the installed payload against an accepted receipt;
  cache reuse and comparison metadata include that payload identity. It does not
  import the benchmark, change its algorithm, or grant strict execution authority.
  Stock and SOAC runs both pass the caller's configured package
  indexes, proxies, and TLS certificates into pyperformance's isolated
  benchmark environments. Allow network access for that bootstrap or populate
  the benchmark environments in advance; when launching Lima from the host,
  use `scripts/run_lima_with_host_environment.py` to bring those settings into
  the guest first.
  When `benchmarks` is omitted, pyperformance uses its default suite selection;
  pass a comma-separated pyperformance benchmark list such as
  `json_dumps,chaos` for a narrower run. Stock runs, `soac-single`, and the
  SOAC apply pass default pyperf sampling to `--fast --min-time=0.05` so
  comparison runs collect multiple values without paying the full default
  pyperf runtime. The SOAC profile pass instead defaults to
  `--fast --warmups=0 --min-time=0.01`, which keeps normal pyperf worker
  behavior while reducing the profiling budget. Extra arguments are passed
  through to
  `pyperformance run`; `--rigorous` and `--debug-single-value` replace the
  default sample mode, and `--min-time=<seconds>` overrides the default
  calibration window for both passes. Before workers start, the real offline
  `ty` driver analyzes an immutable source overlay using their actual prepared
  benchmark venv. The fixed `driver-local-static-imports-v1` policy opts in the
  driver and its statically imported local modules; unimported `.py` input data,
  dynamically imported dependencies, third-party packages (including `tomli`),
  and standard-library code remain ordinary. Class policy is automatic, unknown
  framework classes stay dynamic, parameter/return annotations remain static
  checker facts, and optional checked fields are disabled for this source policy.

  The `terminal-main-measurement-suffix-v1` preparation preserves definitions
  and setup in module initialization, then runs the unchanged measurement
  suffix in an ordinary copied namespace after the real strict loader seals the
  module. Workload functions retain their actual strict globals. Preparation
  rejects unsupported main shapes, suffix rebinding of workload globals, and
  unsupported reflective namespace access; source/AST, input-data, policy, and
  harness fingerprints record this disclosed split. The worker sets the exact
  opted-in path allow-list and reexecutes with the offline descriptor in native
  `-X soac_strict_config=...` startup configuration. Missing/stale authority is
  fatal before user code, never an ordinary run mislabeled as strict. Immediately
  before measured values, the worker checks actual native module seal/source
  diagnostics. Extending an allow-list or editing a provenance manifest grants
  no runtime authority. Workers also
  default `SOAC_BACKGROUND_JIT=0`, because pyperformance uses short worker
  subprocesses where background compiler threads can outlive interpreter
  shutdown, and default `SOAC_COMPILE_MODE=eager` because lazy first-call
  compilation can block pyperformance's single worker loop. In SOAC modes, the
  recipe treats `SOAC_WORK_DIR` as a root and the worker wrapper writes each
  benchmark invocation's counters, logs, and module cache under a stable
  per-script-and-variant subdirectory so full-suite runs can profile many
  `__main__` scripts without source-hash or type-observation collisions. SOAC
  worker directories also keep `pyperformance-worker-timing.jsonl`; after each
  SOAC pass, the recipe prints a compact rollup of setup time before pyperf's
  measured-value collection, measured-value collection wall time, and total
  worker lifetime.

- `just pyperformance-compare [benchmarks=chaos] [rounds=3] [baseline] [extra args...]`
  Compare stock CPython with separately profiled SOAC apply runs using
  independently started, alternating measurements. The comparison and its
  nested stock/SOAC rounds reuse fresh repo and benchmark virtual environments;
  SOAC rounds reuse the current release extension instead of rebuilding or
  restaging it unnecessarily. The default `chaos` benchmark is a quick mixed
  Python workload that exercises custom classes,
  mutable attributes, operators, calls, nested loops, lists, arithmetic,
  branches, and standard-library functions.

  ```bash
  just pyperformance-compare
  just pyperformance-compare all 3 '' --rigorous
  just pyperformance-compare chaos 3 work/pyperformance/<previous-comparison>
  just pyperformance-compare chaos 1 '' --debug-single-value
  ```

  The examples run the default comparison, the full suite, a comparison
  against a previous SOAC result, and a quick single-round smoke test,
  respectively; replace `<previous-comparison>` with an existing comparison
  directory. A prior baseline must be a strict-SOAC result; retired ordinary-SOAC
  measurements are not comparable. Its platform, interpreter, and pyperformance
  metadata are checked before a comparison directory or benchmark round is
  created. Benchmark drivers can emit multiple differently named pyperf
  results; comparisons validate every requested driver, every emitted result,
  and consistent driver-to-result attribution across stock and SOAC rounds.
  Before starting measurements, `comparison-plan.json` fixes the requested
  drivers, paired-round count, alternating stock/SOAC order, outputs, extra
  arguments, and prior baseline. `run-status.json` preserves each command's
  terminal exit status; every phase writes `<output>.status.json` with each
  driver's dependency-preparation, strict-preparation, or worker outcome.
  Fresh checker logs are copied beside that phase's output so a later apply
  attempt cannot overwrite a profile failure's diagnostics.
  A failed profile does not skip apply or later rounds, and successful drivers
  still get measured without narrowing the requested set. Missing or failed
  phases, drivers, results, or requested rounds produce `summary.txt` and
  `summary.json` with `complete: false` and a nonzero command exit. Such a report
  has no full-suite geometric mean or merged comparison results. It retains
  the original phase JSON files, individually comparable results paired over
  every requested round, and separately labeled partial native-seal/JIT/size
  evidence from available apply outputs. Any incomplete profile is disclosed
  alongside its result's diagnostic ratio; partial data is not an acceptance
  score. Existing result directories cannot be rerun in place, and a directory
  without its original plan cannot prove the requested round count.
  Extra arguments may control sampling but cannot replace the comparison's
  benchmark selection, interpreter, or output files.
  Every result must retain the same original input fingerprint, strict source
  selection, and harness policy across rounds and the previous strict revision.
  The schema-3 strict-source manifest also records the exact policy projection:
  an upstream `pyproject.toml` keeps its original bytes, comments, and values;
  only the declared `[tool.soac.strict]` table may be appended. An identical
  existing policy is reused unchanged, while conflicting policies or sealed
  TOML namespaces are rejected. Verification regenerates this projection from
  the original driver metadata, not merely from editable manifest hashes.
  Each complete `summary.txt` and `summary.json` reports
  benchmark-result-specific transformed project/dependency and standard-library
  module coverage, distinct measured apply-worker process counts, compiled
  functions, available pre-optimization serialized BlockPy
  bytes, optimized typed-IR final basic-block counts, and apply-mode emitted
  native-code bytes and machine-block counts. Module coverage requires matched
  native-seal snapshots from measured workers, not cache-file existence. A
  successful import and compiled-function inventory do not establish that the
  meaningful hot path ran in the JIT; inspect representative measured-worker
  profiles separately. Missing strict cache sizes are reported unavailable,
  not interpreted as zero-size IR.
  Use the full fixed pyperformance benchmark selection for authoritative
  optimization claims and compare against both stock CPython and the previous
  SOAC revision. If `chaos` fails on an unsupported compiler/runtime shape,
  report the failure and use the existing pystone benchmark as a fast regression
  sanity check. A large pystone slowdown requires investigation, but a pystone
  improvement does not establish a pyperformance improvement.

- `just pyperformance-deep-profile-from-profile <result.json> <benchmark> [worker=<worker-dir>] [loops=<count>]`
  Replay one measured pyperformance worker directly from a prior SOAC
  profile/apply run and collect `perf` plus Speedscope artifacts for the worker
  body. The SOAC pyperformance wrapper records replay metadata in
  `<result>.soac-work/worker_manifest.jsonl`; this recipe selects a measured
  profile worker, rejects calibration workers, and asks for `worker=<worker-dir>`
  if the benchmark has more than one measured worker. Replay requires the same
  verified strict source bundle, actual selected venv, and startup descriptor;
  ordinary records or changed source/harness/policy inputs are rejected. It uses
  the same post-seal worker entrypoint, not an ordinary script with a strict
  label. Artifacts are written
  beside the selected worker by default under
  `<worker-dir>/worker_perf*`. The replay worker pauses through
  `SOAC_PYPERFORMANCE_MEASURE_READY_FILE` immediately before pyperf starts its
  measured values, so the attached profile excludes benchmark-module import and
  any pyperf warmups. The recipe reuses a fresh release runtime and records the
  portable `cpu-clock` event with a 1024-page buffer; captures with missing or
  lost samples fail. Detailed JIT block maps default to `SOAC_JIT_BB_MAP=1`.
  Set `SOAC_JIT_BB_MAP=0` for symbol-level profiling when per-block map writes
  are prohibitively slow, such as on a VM-mounted checkout. Use this when
  pyperformance says a benchmark is slow and you need measured-worker
  attribution instead of profiling the pyperformance harness.

- `just precompile-shared-library counters=<profile.bin> out=<lib.so>`
  Offline precompile a counter-referenced set of cached BlockPy modules into
  relocatable object files and link them into a shared library. The counter
  file normally comes from a previous profile pass, and the matching
  pre-optimization BlockPy cache entries must still exist in the active
  `$SOAC_WORK_DIR/modules` cache. With the default benchmark cache isolation,
  that cache is the benchmark result's `counters/modules` directory. When
  `counters` is omitted, the recipe uses `$LAST_BENCHMARK_COUNTERS`. The resulting
  `.so` is an inspection artifact only. It is not loaded by runtime execution;
  strict code uses its individually authenticated JIT templates.

- `SOAC_JIT_PERF_HELPER_FRAMES=1`
  In `fn should_preserve_perf_helper_frames`, at
  [crates/soac_jit/src/jit/specialized_helpers.rs](crates/soac_jit/src/jit/specialized_helpers.rs),
  select profiling-oriented helper wrappers that preserve explicit stack
  frames. This improves perf call stacks but is slower than the default
  fast helper path. The perf recipes default it on.

- `jit-$PID.dump`
  SOAC always records JIT code-load events on Linux. The dump is written
  to `SOAC_WORK_DIR` when that variable is set, or `/tmp` otherwise.

- `WARMUP_LOOPS=<int>`
  In recipe `perf-pystone-jit-warm`, at
  [Justfile](Justfile), and its benchmark recipes,
  control the pre-measurement pystone warmup count.

- `BENCHMARK_CPU=<int>`
  In [scripts/run_benchmark_with_cpu_mode.sh](scripts/run_benchmark_with_cpu_mode.sh),
  choose the CPU core that the benchmark recipes pin to with `taskset`.
  The default is empty, which runs without CPU pinning. Set an explicit
  CPU core when you want lower scheduler or heterogeneous-core variance.

- `PYPERFORMANCE_RESULTS_DIR=<path>`
  Overrides the benchmark result and benchmark-venv root (default
  `work/pyperformance`). Nested comparison/profile/apply recipes preserve the
  selected root. A new task-owned guest-local directory can keep result I/O off
  the shared mount, but first verify its physical backing capacity; this does
  not move source or existing results.
  Check physical backing capacity too: Lima's sparse guest disk can occupy the
  same host volume. A large guest free-space number does not create host space.
  Budget and monitor both artifact-volume and shared/host free space during
  large copies and builds; stop before exhausting either volume.
- `PYPERFORMANCE_AFFINITY=<cpu-list>` / `PYPERFORMANCE_TIMEOUT=<seconds>`
  Optional `just pyperformance` pass-throughs to pyperformance's `--affinity`
  and `--timeout` options. If `PYPERFORMANCE_AFFINITY` is unset, the recipe uses
  `BENCHMARK_CPU` as the affinity list when that existing benchmark knob is set.

- `PYPERFORMANCE_INHERIT_ENV_EXTRA=NAME[,NAME...]`
  Adds explicitly named environment variables to pyperformance's isolated
  environments in both stock and SOAC modes. Common package-index, proxy, and
  TLS-certificate settings are inherited automatically. The Lima host helper
  also forwards explicitly named extras into the guest. Generic `ALL_PROXY` /
  `all_proxy` settings are not inherited unless explicitly named, because SOCKS
  proxies can break local HTTP benchmarks. SOAC modes continue inheriting their
  required transformed-runtime variables.

- Strict pyperformance worker plumbing
  `SOAC_PYPERFORMANCE_ENABLE=1` selects strict benchmark preparation and worker
  checks; `SOAC_PYPERFORMANCE_DRIVER=1` keeps the ordinary pyperformance manager
  out of worker activation and is not forwarded to measured children.
  `SOAC_PYPERFORMANCE_CHECKER` identifies the prebuilt offline CLI selected by
  the recipe. `SOAC_PYPERFORMANCE_STRICT_BUNDLE` identifies its immutable
  per-driver execution manifest; it is provenance, not startup authority.
  `SOAC_PYPERFORMANCE_WORK_ROOT` retains the original output root while each
  worker sets `SOAC_WORK_DIR` to its variant directory.
  `SOAC_PYPERFORMANCE_EXEC_WRAPPED=1` prevents recursive worker reexec and is
  accepted only with a matching native startup descriptor. These variables
  are recipe-owned; stock runs clear activation/bundle flags. Real authority
  still comes from authenticated native startup, never these environment flags.

- `BENCHMARK_CONSTANT_CLOCKS=0|1`
  In [scripts/run_benchmark_with_cpu_mode.sh](scripts/run_benchmark_with_cpu_mode.sh),
  control whether the benchmark wrapper temporarily forces steadier CPU clocks for
  the selected benchmark core and its related CPUs by setting the
  governor to `performance`, locking `scaling_min_freq` and
  `scaling_max_freq` to the hardware max frequency, and disabling boost
  when the kernel exposes a `boost` knob. The benchmark recipes default
  this to `0`, and the wrapper restores previous settings on exit when
  constant-clock mode is enabled. When direct writes are not permitted,
  the wrapper uses `sudo` automatically for the sysfs write and restore
  path. Set `BENCHMARK_CONSTANT_CLOCKS=1` to opt in.

- `SPECIALIZATION_PROFILE_LOOPS=<int>`
  In recipe `perf-pystone-jit-specialized`, at
  [Justfile](Justfile), control
  the first-pass profiling loop count used to derive specializations.

- `PERF_FREQUENCY=<int>`
  Set the `perf record -F` sample frequency. The warmed N-Queens recipe
  defaults to `99` so full DWARF call stacks do not produce excessively large
  profiles or lose samples; `perf-pystone-jit-warm` defaults to `999`. In
  `perf-pystone-jit-warm`, at
  [Justfile](Justfile), set the
  sampling frequency for the pystone workflow.

- `PERF_CALL_GRAPH=<mode>`
  In recipe `perf-pystone-jit-warm`, at
  [Justfile](Justfile), set the
  `perf record --call-graph` mode. The default is `dwarf,65528`, which
  captures a much larger user-space stack dump so mixed JIT/CPython
  stacks are less likely to truncate into misleading leaf-only C helper
  frames.

- `PERF_PERCENT_LIMIT=<float>`
  In recipe `perf-pystone-jit-warm`, at
  [Justfile](Justfile), control
  the threshold used when rendering perf text reports.

## CPython Test Selection

- `SKIP_EXPECTED_FAILURES=1`
  In [scripts/collect_cpython_skip_ids.sh](scripts/collect_cpython_skip_ids.sh),
  include expected-failure IDs when building the CPython skip list. Set
  it to `0` to stop filtering on `EXPECTED_FAILURE.md`.

- `CPYTHON_TEST_SETS_GLOB=<glob>`
  In [scripts/run_cpython_test_sets.sh](scripts/run_cpython_test_sets.sh),
  choose which test-set files to run.

- `CPYTHON_TEST_TEMPDIR=/tmp/...`
  In [scripts/run_cpython_test_sets.sh](scripts/run_cpython_test_sets.sh),
  choose the tempdir used for CPython regrtest set runs.

- `CPYTHON_TEST_LOG_DIR=/path/to/logs`
  In [scripts/run_cpython_test_sets.sh](scripts/run_cpython_test_sets.sh),
  choose where per-set CPython logs are written.

- `SKIP_FILE=/path/to/cpython_skipped_tests.txt`
  In [scripts/collect_cpython_skip_ids.sh](scripts/collect_cpython_skip_ids.sh),
  choose the base skipped-test list file.

- `EXPECTED_FAILURES_FILE=/path/to/EXPECTED_FAILURE.md`
  In [scripts/collect_cpython_skip_ids.sh](scripts/collect_cpython_skip_ids.sh),
  choose the markdown file that contributes expected-failure test IDs.

- `PYTHON_BIN=/path/to/python`
  In [scripts/collect_cpython_skip_ids.sh](scripts/collect_cpython_skip_ids.sh),
  choose which Python binary is used when collecting skip IDs.
