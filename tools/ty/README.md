# Offline strict-type analysis

`soac-ty` is SOAC's separate offline executable. It uses the committed vendored
`ty_project` database and `ty_python_semantic` queries; it does not import the
analyzed Python modules, parse diagnostic prose, or establish runtime
capabilities. The JIT does not link this workspace.

Artifact schema 7, strict-contract version 3, SOAC dialect version 2, and
deployment schema 3 bind resolved source-comment policy. Retired TOML strictness
tables and runtime parameter/return policy keys are rejected; old signed
publications must be regenerated, not silently reinterpreted.
Function signatures remain static facts. Only selected field
writes create runtime value obligations, including when ordinary generated
constructors perform those writes. The native/runtime migration remains under
implementation until the matching build and compatibility gates pass.
Optimization and benchmarks remain deferred until separately requested.

The schema retains field annotation, nominal-binding, and semantic base
provenance. The semantic exporter
distinguishes an explicit class/instance annotation from an inferred field type,
including a field assigned from an annotated constructor parameter. Dataclass
fields retain their own or inherited declarations; generated initialization does
not invent annotations. Parameter origins come from actual parameter or field
declaration nodes, not ty's signature-display flags: synthesized dataclass
receivers and comparison operands remain inferred even when their display types
are nominal. Source-written and field-derived annotations remain explicit.
Stdlib dataclass constructor receivers follow the actual native field-table
rule: a field named `self` selects `__dataclass_self__`, including inherited
`ClassVar`, `InitVar`, and `init=False` entries, but not a `KW_ONLY` marker or
ordinary attribute. The semantic producer records stdlib provenance explicitly;
custom transforms sharing flags or field specifiers do not inherit this rule.
The field-expanded `__replace__` proposal has an unnamed positional-only
receiver. Its exported collision-free label is not a native parameter name or
a source annotation. Genuine duplicate native constructor names still reject
publication; no signature validator is relaxed.
This provenance is signed and participates in shard,
generation, and runtime cache identities. Every field carries the actual unique
annotated assignment identity, or an explicit null when that provenance is
unavailable. Each supported simple-name nominal
annotation leaf records its exact function parameter/return target or field
declaration owner, source range,
checker-resolved class, and local binding definition and scope. Separate leaves
remain distinct even when type normalization merges equal class references;
different aliases may hold different executions of one class definition.
Inherited field references retain the original declaring class and assignment.
Generated constructor parameters consume those field references, not synthetic
source-parameter identities. Field and function leaves are disjoint owners;
runtime enforcement still requires the corresponding actual execution witness.
Each annotation's plan is complete or absent. One unresolved nominal union leaf
discards that owner's partial plan, including when type normalization merges
equal class references. Builtin/numeric, `None`, and `type[T]` arms need no
nominal target, and semantic `Annotated` metadata does not become a type leaf.
Explicit imports retain their actual local import definition, both with and
without an `as` clause; the foreign class definition and dependency digest stay
separate. A plain `from targets import Box` therefore consumes that module's
actual `Box` binding, not a lookup synthesized from the foreign class name.
Ambiguous bindings, star imports, and attribute expressions do not gain a
nominal binding plan. Ordinary ty/IDE alias-resolution behavior is unchanged.
Quoted annotations use ty's existing string-annotation parse and scope-aware
semantic submodel, including original byte ranges and resolved definitions.
The runtime can consume module bindings and exact structural direct-self
construction identities without evaluating their providers. A quoted lexical
or class alias that has no actual runtime binding operand remains unresolved;
the exporter does not fabricate a native provider capture or turn that missing
required target into an unchecked acceptance.
Quotation alone does not imply an empty provider closure: native method
providers can retain their special class-dictionary capture. Runtime layouts
must match the actual native provider, not an assumption based on string syntax.
Unsupported binding expressions remain unresolved, not guessed from names or
annotation text. Schema-1 through schema-6 artifacts and signatures must be
regenerated; omitted provenance is not accepted through compatibility defaults.

Direct bases and logical MRO entries use an explicit `BaseReference`: a
source-bound `ClassReference` or a semantic `BuiltinType`. The checker resolves
builtin aliases through `KnownClass`, not names such as `object` or
`builtins.object`; a user class with that spelling keeps its source identity.
Implicit roots appear in the logical MRO, not the direct source-base list.
Builtin `object` must terminate a complete logical MRO when present. Modeled
typing/ABC entries remain logical source references, not physical CPython
layout. A builtin proposal alone grants no participation: unsupported builtin
subclasses remain dynamic, and runtime admission must match the selected actual
base object independently.

Lambda identities use the same semantic lexical ancestry as other definitions,
including class bodies and defaults evaluated in an enclosing scope. The signed
lexical convention leaves comprehension scopes transparent and does not invent
a function-local scope beneath a class or lambda. Exact original byte ranges
distinguish anonymous definitions with the same lexical name. Native code-object
names are a separate projection from the original parsed source: nested lambdas
have native `<locals>` components, and generator-expression bodies have native
`<genexpr>` components while their first iterables remain in the outer scope.
The runtime matches this exact validated native projection together with the
signed identity, native source stamp, signature, and opcode source ranges; a
name alone never authorizes a callable.

All commands below run in the Ubuntu Lima guest checkout.

## Build and run

The source of truth is SOAC's `vendor/ruff` gitlink, with
`https://github.com/adamh-oai/ruff.git` as the submodule origin. Checker changes
are logical commits there; generated upstream dependency locks belong in a
separate top commit. The root compiler workspace and this exporter use the
same vendored Ruff crates. Do not edit Cargo's Git cache or maintain a second
applied patch generation.

`just ty-prepare` verifies this local committed checkout without fetching,
applying patches, resetting or repairing it. The shared source verifier checks
the exact index, raw file contents and executable/link modes against a fresh
canonical checkout of the pinned Git objects. Partial/promisor clones,
untracked checker files and source changes are rejected. `just ty` holds a
shared source lock and rechecks the same identity around compilation and use.
The lock is distinct from the strict-fixture build-serialization lock.

Signed checker identity uses the actual Ruff revision, tree and checkout
digest; the exporter fingerprint covers the wrapper/verifier, its source and
dependency locks. A changed source pin requires rebuilding and republishing
contracts, not editing a saved marker or deployment digest. Upstream test
builds retain `--locked`; refresh locks only in a mutable review checkout while
preserving compatible external versions.

The 2026-08-23 PDT migration is local-only by user request. Independent local
checkout reproduction is checked; remote checkout and CI availability remain
deferred until publication is separately requested.

Use `just fmt-rust soac_ty` and `just fmt-rust-check soac_ty` for this
standalone Cargo workspace; the recipes select its manifest explicitly.

Policy comes from Python source comments, not a configuration file. Both
`strict_assign` and `checked_attr` default to `false`. For package defaults in
`pkg/__init__.py`:

```python
# soac: package(strict_assign=true, checked_attr=true)
```

Package settings inherit outer-to-inner through `__init__.py`; omitted keys
retain the inherited value. A module override changes only that file. For
example, `pkg/model.py` can keep checked classes without sealing its globals:

```python
# soac: module(strict_assign=false)

class Checked:
    value: int

# soac: class(checked_attr=false)
class Dynamic:
    pass
```

Package/module directives are standalone header comments before the first
statement other than an initial docstring and future imports. A class directive
precedes the class and all its decorators at the same indentation; it binds the
exact class AST node, including nested or repeated-name declarations, not a
name pattern. Class directives accept only `checked_attr`, with either `true`
or `false`; missing keys inherit. They do not implicitly select nested classes.
No per-class annotations or eligibility list are required.

`strict_assign` selects module assignment invariants independently of
`checked_attr`, which selects eligible class invariants and supported field-write
checks. Framework exclusions still cause dynamic fallback. An explicit class
opt-out adds no new local contract but cannot revoke checks inherited from a
protected base. Module `strict_assign=true` does not force `checked_attr=true`,
and `checked_attr=true` does not seal otherwise mutable module globals.

The old `[tool.soac.strict]` table and its overrides are rejected, not converted.
Ordinary checker configuration remains supported and authenticated. Importing
a participating module does not opt its importer in. Ordinary CPython ignores
these comments; enforcement requires authenticated startup and actual runtime
module/type binding. Neither test helpers nor scenario adapters insert future
imports. The trusted raw compiler internally sets the strict ownership guard
flag when compiling verified source; it does not rewrite source or resolve
policy from that flag.

Create a private build-side key outside the artifact output directory:

```bash
mkdir -p work/type-authority
just ty -- keygen --signing-key work/type-authority/signing.key
```

Analyze with the exact selected CPython executable and absolute deployment
paths. Paths passed to `check` are otherwise relative to `--project`:

```bash
just ty -- check \
    --project /absolute/project \
    --python /absolute/selected-cpython/python \
    --signing-key /absolute/build-side/signing.key \
    --output /absolute/artifact-store \
    --deployment /absolute/startup-authority/deployment.json
```

`--source-root` sets the import root used for dotted module names. Explicit
`--module dotted.name=relative/path.py` entries select named modules, including
`--module __main__=driver.py`; they still obey project selection policy. Without
explicit modules, discovery excludes `.git`, `.jj`, `.venv`, `vendor`, `work`,
`target`, `__pycache__`, and the chosen artifact output directory. These are
discovery exclusions, not import exclusions: an explicitly consumed import into
one of those directories is still resolved and authenticated.

This command publishes selected contracts; it is not a replacement for ordinary
`ty check`. Type diagnostics in unselected ordinary inputs are informational and
do not block selected siblings. Consumed source bytes and configuration remain
authenticated. Unsuppressed errors in selected exports still block publication;
syntax, source-policy and authentication failures also remain fatal.

The initial supported target is CPython 3.15 on Linux. The isolated interpreter
probe uses `-I -S -B`; it reports actual site-packages/stdlib paths and the loaded
`libpython`, including an uninstalled source build. The selected executable path
is invoked without resolving its symlink first, so a virtual environment keeps
its own prefix and package paths. The canonical executable and actually loaded
library remain ABI inputs; the selected path, symlink target, and `pyvenv.cfg`
are independently observed for invalidation. It does not run `site`,
`.pth` Python code, user startup hooks, or project modules. The matching library
beside the real source-build executable takes precedence during the probe,
including when the selected command is a venv symlink.

The default build is release. Use `just ty --debug-build -- ...` for iteration.
To build and verify the normal executable without analyzing a project, use
`just ty --debug-build -- --help`. The separator is required: without it,
`--help` prints the wrapper's help and performs no build. A successful
`--test` run validates the test executable, not the separately built normal binary.
`just ty-prepare` verifies the pinned committed sources without rewriting them.
Normal builds use the separate
`tools/ty` workspace. `just ty --debug-build --test-upstream ty_project --`
and `just ty --debug-build --test-upstream ty_module_resolver -- FILTER`
test the pinned upstream libraries with their own pinned lockfile. All three
routes use `work/target-ty` and must run serially, with all Ruff-family dependencies
resolved from one verified committed checkout. Before and after compilation,
the source verifier checks raw bytes, filesystem kinds, executable bits, and
index entries against the exact pinned Git objects. A cache marker is never
accepted as its own authority. Verification does not rewrite the checkout or
rebuild unchanged checker dependencies. Normal builds use the tracked lockfile with `--locked`;
after an intentional dependency change, run `just ty --update-lockfile` and keep
the generated `tools/ty/Cargo.lock` change separate from source changes. This runs
`cargo update --workspace` against the verified checker configuration, adding or
updating workspace/path dependencies without upgrading unchanged external
dependencies. External dependency upgrades require a separate, intentional
Cargo update; do not regenerate the entire lockfile just to add a workspace
dependency.

The upstream lockfile belongs in a separate generated top commit in the Ruff
repository. Regenerate it with `cargo update --workspace` in a separate,
explicitly mutable review checkout using the same dependency configuration,
then pin the resulting committed history from SOAC. Do not run unlocked Cargo
commands in the selected `vendor/ruff` checkout.
Offline lock refreshes can downgrade unrelated packages to locally cached
versions; inspect the full package delta and preserve compatible existing pins.
Both test routes use `--locked` and revalidate source integrity after success;
passing tests do not excuse an altered lockfile or marker.

## Publication and authority

Successful checks print a JSON record containing the generation, immutable
artifact directory, module count, and reused shard count. The content store is:

```text
OUTPUT/objects/<digest>.soac-types
OUTPUT/<generation>/manifest.json
OUTPUT/<generation>/modules/<digest>.soac-types
```

Every shard is validated before signing. Complete generations use atomic,
no-replace directory publication. Identical repeated checks reuse verified
objects; existing incomplete, missing, or changed artifacts are rejected rather
than silently repaired. The startup descriptor is replaced atomically only
after successful analysis and complete-generation validation. A failed check
does not replace a previously selected descriptor. Output and authority paths
may not overwrite or invalidate inputs consumed by the checker; a dedicated
authority directory avoids conflicts with genuine full-directory consumers.
An authority destination must be absent or a regular file, not a symlink.
Descriptor serialization is buffered and explicitly flushed before file sync,
final input revalidation, and atomic replacement. A flush error leaves the
previous descriptor untouched and removes the private staging file; buffering
does not change the serialized bytes or any observation boundary.

The signing seed is never stored in the artifact directory. The schema-3 descriptor
contains the public trust anchor, expected generation/environment, selected
interpreter identity, module policies, dependency source paths and actual per-file
checker settings, and observed inputs. It is **out-of-band startup authority** and must
be protected separately from writable artifacts. Copying a public key from an
untrusted artifact does not establish trust.

The shared dependency verifier reconstructs expected records from this startup
authority and current source bytes, not from manifest dependency entries. System
dependencies bind their path, SHA-256, historical source ID, size, and selected
strict policy. Vendored dependencies bind their relative bundled path and source
identity to the checker/typeshed build fingerprint. Interpreter ABI hashing uses
the shared `InterpreterIdentity::abi_fingerprint` routine; the runtime must also
independently identify its executable, loaded library, version, and platform.
Deserializing the expected identity cannot establish that the actual process matches.

Input verification includes source bytes and canonical symlink destinations,
negative import candidates, configuration, selected interpreter/library bytes,
distribution metadata, consulted environment variables, and resolved import
dependencies. Cached resolver listings record their actual exact-name, prefix,
or suffix query. Full enumerations remain full observations when the checker
consumes them. New relevant `.py`, `.pyi`, `.pth`, stub-package, or distribution
metadata entries invalidate the old analysis; unrelated output/cache filenames
do not invalidate a name query. No global rule pretends `__pycache__` is absent.
Inputs are checked again before publication and must be rechecked by the
runtime loader. Revalidation checks expected file sizes before bounded reads,
including racing growth. Files can still change after any filesystem snapshot.

## Facts and conservative limits

The checker exports owned source identities, globals, supported annotations,
logical fields, method binding/signatures, inheritance/finality, dataclass
options/defaults/factories/generated members, and attribute/call predictions.
`Any`, `Unknown`, protocols, unsupported generics, unresolved decorators and
frameworks remain uncertain. Unknown dataclass options do not become guessed
layout flags. An open nominal class does not become an exact receiver family.
Ignored diagnostics demote affected owners and dependencies, not unrelated
classes.

For locally classified framework classes and actual user-defined metaclasses,
an unresolved attribute does not require an annotation cast to publish the
surrounding strict module. The exporter matches the structured ty lint and
exact expression to its semantic receiver, retaining a visible
`strict-unchecked-dynamic-type` warning. The receiver must have a genuine
framework exclusion, directly or through its resolved MRO. The affected
attribute values and consuming attribute/call proposals become Unknown/dynamic.
Lexical containment in a framework method, `Any`, an ignore, or an arbitrary
mutable base does not qualify. Candidate-class attribute mistakes, unresolved
names/imports, known-invalid declared writes, and strict finality/mutation
violations remain errors. Ordinary ty checking and ordinary files are unchanged.

A syntactic `global` declaration resolved to module scope may target an initially
absent binding, as permitted by the strict mutable-global contract. The exporter
reconciles only the exact structured `unresolved-global` diagnostic at such a
declaration, retains a visible warning, and records an explicitly mutable
`Unknown` value with no definition or boundness proof. Unrelated unresolved reads,
invalid nonlocal scopes, and assignment errors still reject publication; ordinary
ty diagnostics are unchanged.

Cross-module plain strict bases are not classified as mutable merely because
they come from another file. An incremental query reuses the complete semantic
class classifier and suppression normalization for the actual defining source,
then checks its resolved MRO recursively. Ordinary, unresolved, cyclic, ignored,
or framework-managed bases still make the subclass dynamic. These are source
proposals only: runtime admission must independently match the actual protected
base objects and their exact source identities. External dataclass/transform
bases remain dynamic when their defining policy cannot be established; the
importing file's source policy cannot authorize a foreign transform.

The offline export path registers and emits these strict diagnostics using the
shared policy and real checker symbol/member/call queries:

- `strict-final-global-rebind` and `strict-final-global-delete`.
- `strict-class-mutation`, `strict-instance-method-shadow`, and
  `strict-classvar-instance-write`.
- `strict-final-class-subclass`, `strict-final-method-override`, and
  `strict-incompatible-override`.
- `strict-incompatible-field-write` when supported checked fields are enabled.

When `strict_assign` is selected, general-function global writes are checked as
operations that must be valid after sealing. This is an assignment invariant,
not function argument/return enforcement. The exporter does not claim module-body
sealing during circular imports. Arbitrary aliases/reflection/native callbacks, external class
participation, physical undeclared-field/dictionary-layout restrictions, and
unsupported framework construction still require runtime enforcement or
dynamic fallback. These diagnostics are emitted by the explicit SOAC exporter,
not ordinary `ty check`; ordinary Python defaults and checking stay isolated.

The artifact only proposes facts. Actual type construction, checked field writes,
module/class sealing, native mutation barriers, and optimization eligibility are
separate runtime work. A signed shard alone cannot authorize direct dispatch or
physical storage assumptions.

## Validation

```bash
just ty --debug-build --test -- --test-threads=1
uv run --no-project --python /usr/bin/python3 --with 'pytest>=8,<9' \
    python -m pytest --noconftest tests/test_ty_toolchain.py
```

The executable tests use the selected `CPYTHON_BIN` exported by `just`, the pinned
committed checker, signed artifacts, and the shared verifier. ABI-drift tests copy
the interpreter/library to a private temporary directory; they never mutate
the shared CPython build. Upstream dialect/export/strict-rule regressions are
maintained in the pinned Ruff commits. The root `just test-all` gate remains the
integration gate for runtime changes.

When testing a private staged commit, use an explicit separate target and the
same root toolchain as the existing cache. A different `+toolchain` can rebuild
all dependencies even with the same target directory. The full project unit
gate is `cargo test ... -p ty_project --lib` with no name filter; `soac_` is a
focused subset. Record the actual passed and filtered counts from each log.
Use the selected `CPYTHON_BIN` or `.venv/bin/python` for an intentional raw
native control, not a guessed executable under the mounted source tree.

The toolchain-only Python suite deliberately needs neither the transformed
runtime nor the repository `.venv`; `uv` caches its small isolated test environment.
The CLI suite covers offline non-execution, deterministic publication, concurrent
writers, artifact tampering, transitive and negative import dependencies, per-file
policy changes, actual suppressed diagnostics, symlink retargeting, interpreter
ABI drift, and package metadata changes.
