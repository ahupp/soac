---
title: "Strict Modules"
---

# Strict Modules

Status: implementation in progress. The requested type-driven implementation
selects the language contract below, as refined by
`doc/TYPE_DRIVEN_OPTIMIZATION.md` and approved in `OPT_GOAL.md`. These are
implementation requirements, not claims that runtime enforcement, diagnostics,
or interoperability tests have already landed. Under the August 23–24, 2026
(PDT) scope amendments, the current milestone is authenticated interpreter
enforcement with SOAC JIT execution disabled. Optimization and benchmarks
remain deferred until separately requested. The 2026-08-25 (PDT) field-assignment
clarification removes all function-level runtime type enforcement, including
compiled/entry/CPython parameter and return checks and generated-constructor
factory-result checks. Static typing, actual source/type ownership and sealed
method metadata remain separate from field-write invariants. This latest scope
supersedes contradictory checked-call requirements in historical sections below;
the removal is implemented locally and combined compatibility validation remains
required.
The additional 2026-08-25 (PDT) traceback/frame amendment excludes SOAC-specific
frame reconstruction, inspectable locals, frame ancestry and native slot-layout
correspondence from both compiled and entry-interpreter execution. It does not
change ordinary CPython frames or weaken exception semantics, source scoping,
explicit callback order, suspension, cleanup, or installed storage contracts.
The subsequent observer amendment also excludes tracing/profiling/monitoring
fidelity and mandatory refusal on retained SOAC paths. Dedicated observer
reservations, setter interception and fallback gates are removed; ordinary
CPython observers, actual-object authentication and mutation barriers remain.

## Summary

A **strict module** explicitly exchanges selected dynamic Python behaviors for
enforced module, class, field, and method-mutation contracts. Fixed instance layouts
and optimized callable dispatch are separate optional capabilities, not
prerequisites for interpreter enforcement. Strict modules are SOAC's only
future optimization target. Ordinary modules retain
ordinary Python semantics as stock-compatible interoperability participants,
not as a separately optimized SOAC mode. Strict and ordinary modules share one
interpreter and can import, call, subclass, and exchange objects with one
another; an operation can omit dynamic checks only when its actual operands
carry verified strict contracts.

The initial contract is:

1. Package/module/class `# soac:` source rules independently select global
   binding and class/field contracts. They require authenticated publication
   and startup to enforce; ordinary CPython ignores comments.
2. With `strict_assign=true`, after module initialization new global names
   may be appended, but an
   existing binding is immutable unless the source contains a lexical
   `global NAME` declaration for that binding. Appending never changes an
   existing binding's stable index; its name-to-index map may rehash.
3. Classes are classified automatically from authenticated offline `ty` facts
   and their actual construction. Module membership, frozen class behavior,
   checked values, method dispatch, and physical instance storage are separate
   capabilities. No per-class SOAC annotation is required. Ordinary classes and
   dataclasses retain their real instance dictionaries; only source-requested
   slots or a verified construction adapter supply native slot layouts.
4. Participating dictionary-bearing instances retain ordinary dictionary
   storage in the current enforcement path. A separately installed optional
   storage capability may provide stable field-prefix indexes plus ordinary
   dynamic overflow; strict membership alone never promises those indexes.
   Class-data defaults keep their real
   class bindings and initially absent instance overrides. Protected method
   and `ClassVar` attribute writes raise, while same-name dictionary entries
   remain permitted but are ignored by every attribute/method lookup path.
   Genuine declared fields take precedence over inherited non-data methods,
   never over actual data descriptors.
5. Participating fresh types receive a native pending-instance barrier before
   class callbacks or other construction-time publication. Allocation and
   `__class__` reassignment into them are forbidden until the actual final
   decorated type is validated and its selected constraints are installed.
   Fresh replacements need their own linked guard. Only an unselected original
   with no permanent type contract may subsequently become dynamic; independent
   ownership and inherited storage contracts remain enforced. Eligible final classes and their
   dispatch-relevant function metadata freeze before instance admission; module
   binding finality remains a separate stage. Frozen annotation providers
   retain private cache slots so annotation evaluation remains lazy.
   Unsupported framework classes automatically remain dynamic before any
   irreversible construction; installed restrictions and sealed capabilities
   are never revoked.
6. `checked_attr=true` selects eligible classes and supported annotated field
   writes independently of module freezing. Every supported writer checks the actual
   selected storage value before committing its write. Function calls retain
   ordinary argument binding and body behavior without runtime parameter or
   return-type checks. Shape alone never proves a value type, and a nominal
   field check never proves a strict layout.
7. Strict-to-strict execution may use direct global loads, stable field indexes
   or native offsets, direct calls, and compact virtual dispatch only with the
   required authenticated capabilities. Boundaries involving ordinary Python
   retain their actual checks and generic fallbacks. Instance destruction may
   use the approved physical-layout field-release order, not skip finalizers.

This is an explicit language policy, not a new permission to change the
semantics of existing transformed modules.

## Source policy rules

The 2026-08-27 (PDT) source-comment policy replaces the old configuration/future
double opt-in. There is no strictness config file. Both settings default to
false; omitted keys inherit their enclosing package/module defaults.

```python
# package/__init__.py
# soac: package(strict_assign=true, checked_attr=true)

# This override affects __init__.py itself, not its descendants.
# soac: module(strict_assign=false)
```

```python
# package/models.py
# soac: module(strict_assign=false, checked_attr=true)

class Checked:
    value: int = 0

# soac: class(checked_attr=false)
class Dynamic:
    value: int = 0
```

| Rule | Effect and scope |
| --- | --- |
| `package(strict_assign=..., checked_attr=...)` | Defaults for that package and descendants, only in `__init__.py`. Inner packages override specified keys. |
| `module(strict_assign=..., checked_attr=...)` | Overrides this source file only, including when that file is `__init__.py`. |
| `class(checked_attr=...)` | Overrides one exact class declaration, before its first decorator and at the same indentation. |

`strict_assign` freezes selected global bindings after initialization; it does
not type-check global values. `checked_attr` selects automatic eligible class
contracts, supported annotated field-write checks and class/method-mutation
barriers. Neither setting implies the other. Class opt-out does not disable
checks inherited from a participating ancestor or retained by escaped storage.
Class opt-in still requires supported actual construction; unsupported bases,
metaclasses, decorators and frameworks remain dynamic. `Any`, unsupported
annotation types and inferred-only fields do not acquire value predicates.
There are no runtime parameter, return, `InitVar` or factory-result call checks.

Package and module directives belong in the module header: before executable
statements, allowing an initial docstring and future imports. A class rule
binds by source range, not name, and does not flow into lexically nested
classes. One directive per package/module/class is allowed. Only lowercase
`true` and `false` are values. Unknown keys, duplicate settings and invalid
placement are errors. Parenthesized settings may span consecutive comment-only
lines:

```python
# soac: module(
#     strict_assign=true,
#     checked_attr=false,
# )
```

The offline checker resolves package ancestry within `--source-root` and
authenticates consulted `__init__.py` bytes **and absence**. Changing a rule or
creating a previously absent package file invalidates the publication. The
signed policy includes exact class-declaration ranges. The retired
`[tool.soac.strict]` table is rejected, and the old `strict` future is not a
policy-selection mechanism. Artifact schema 7, strict-contract version 3,
dialect version 2 and deployment version 3 require republishing older artifacts.

Comments request a contract; they do not install one. Ordinary CPython ignores
them. The authenticated loader compiles verified source with a native guarded-
code marker and binds actual objects before installing checks. Neither that
marker nor copied code grants execution authority. For an admitted mutable
module, native dictionary ownership protects authentication and terminal
lifecycle without freezing keys or values. Readiness is separate from binding
sealing (`StrictModuleRuntimeState::is_ready`); diagnostics report both.
Nominal field checks capture the actual authenticated type at construction and
do not change when a mutable module later rebinds the type's name.

### Source literal representation limitation

SOAC currently rejects active `\uXXXX` and `\UXXXXXXXX` escapes whose value is
a surrogate code point (`U+D800` through `U+DFFF`) in selected strict source.
The pinned Ruff parser replaces those values with U+FFFD, and SOAC must not
compile or sign that different Python string. The shared `soac_source`
validator examines actual lexer tokens before lowering and reports the original
escape's byte range. This is an explicit unsupported-source failure, not a
claim of lossless surrogate-literal support.

The restriction includes plain and implicitly concatenated strings, docstrings,
f/t-string literal portions and format specifications. It does not reject raw
literal portions, bytes, comments, escaped backslashes, or genuine U+FFFD
characters and escapes. A normal string expression inside a raw f/t-string is
checked using its own tokens. If the checker actually enters a second parse of
a string annotation, it validates that parse's tokens independently.

Ordinary modules still execute with native CPython string semantics. When an
ordinary dependency contains an unsupported source escape, SOAC analysis
withholds its exact string-literal facts as uncertainty instead of signing a
substituted U+FFFD literal; interpolation expressions are still analyzed.
This also applies to imported literal aliases and vendored source inputs.
The restriction does not forbid surrogate-containing strings created at runtime
or passed from ordinary Python, and it never makes those strings equal to U+FFFD.

## Motivation

Recent SOAC investigations found that preserving ordinary CPython method lookup
is intrinsically expensive: inherited method-lookup ancestry accounted for
approximately **19.238% of `richards`** and **21.740% of `deltablue`** in the
recorded native profiles. Those paths must currently account for instance
shadowing, arbitrary descriptors, mutable classes, dynamic MRO changes, custom
attribute hooks, and late replacement of function implementations.

The parallel class-layout work showed that CPython 3.15's
`__static_attributes__` can preseed split dictionaries, but it does not produce
a complete contractual field layout. Inherited instances can still have
different split-dictionary indexes, watchers do not replay preexisting keys,
and direct indexed writes can bypass dictionary watchers and version updates.

The intended win is to replace those guarded observations with enforced facts:

```text
ordinary Python:
    mutable module + mutable class + instance dictionary
    -> dynamic lookup + identity/version guards + fallback

strict Python:
    sealed module + independently enforced function/class/storage capabilities
    -> fixed binding + fixed field index/offset + direct body or vtable entry
```

These profiles identify promising targets; they do not establish a strict-mode
speedup or achievement of the strict-versus-stock pyperformance target.

## What CinderX demonstrates

Meta's [Static Python overview][cinder-overview] describes specialized modules
that interoperate with normal Python, automatically slotted classes, immutable
modules and types, fixed-offset field access, and entry checks that may be
omitted between statically compiled callers and callees.

Its [tutorial][cinder-tutorial] distinguishes `import __strict__` from
`import __static__`; the latter adds checked type information and implies the
former. It also documents runtime-checked containers and boxing/unboxing at
ordinary-Python boundaries. Its [compatibility guide][cinder-patterns] records
real migration problems: weak references, multiple-inheritance layout
conflicts, field/method collisions, dynamic intermediate base classes, mocks,
and observable changes to keyword argument representation.

The [strict helper API][cinder-strict-api] includes migration concepts such as
extra slots, loose slots, mutable classes, and frozen types. The
[strict module implementation][cinder-strict-module] returns a fresh copy for
`module.__dict__` while keeping actual globals in a separate private dict; its
loader also has an explicitly enabled testing-patch mode. A
[strict-codegen regression][cinder-strict-global-test] directly exercises a
function that declares `global abc` and successfully updates its strict
module's `abc` binding.

CinderX's [default-slot regressions][cinder-default-slot-tests] also preserve
`C.x == 42` and `C().x == 42` for an annotated class default, while allowing
an instance to replace its own `x` without changing `C.x`. Its
[default-value descriptor][cinder-default-descriptor] uses a fixed instance
offset, but replaces the original class-dictionary value with a descriptor,
checks the assigned type, and dispatches through descriptor getter/setter calls
rather than its ordinary optimized field instruction. Its tests also document
that subsequent class replacement can discard an instance override.

SOAC should borrow the opt-in boundary and enforced-storage model;
fixed layouts remain deferred. SOAC source comments request independently
authenticated contracts; an import marker or future flag is not authority. For class defaults,
preserve the actual class-dictionary entry, cover both annotated and unannotated data, and
emit a fixed-index/default-fallback field operation without inferring checked
values from the layout. Value checks require the declaring class's resolved
`checked_attr=true` source rule and
their own complete enforcement. Frozen classes prevent CinderX's class-patching
conflict. SOAC should **not** copy import-side-effect bans, snapshot
`module.__dict__` semantics, unchecked primitive overflow, or observable
keyword-to-positional rewrites. Its current module objects expose a real exact
dictionary, and preserving that stronger interoperability is the preferred
design.

## Opt-in and runtime availability

Source comments request the independently selected contracts:

```python
# soac: module(strict_assign=true, checked_attr=true)
```

The shared comment grammar permits package/module rules in the header before
the first executable statement, allowing an initial docstring and ordinary
future imports. Class rules precede the exact class and all its decorators.
Package defaults and omitted-key inheritance follow the
[source-policy rules](#source-policy-rules); neither flag implies the other.
For example:

```python
"""Optional module docstring."""
from __future__ import annotations
# soac: module(checked_attr=true)
```

These comments add no Python statements or imported feature binding.
Ordinary CPython ignores them and executes ordinary Python; comments alone
neither install enforcement nor guarantee rejection when SOAC is absent.
The retired `strict` future and `[tool.soac.strict]` configuration are not
alternative selection mechanisms. Imports, aliases and assignments involving
a Python name `strict` do not select or revoke a contract.

A deployment promising selected contracts must authenticate its offline
publication and runtime support before any selected module body executes.
Every execution of internally guarded module code is checked against
authenticated compiler provenance, an
explicitly registered strict module, its actual protected globals, and its
active runtime policy before the first user instruction. If the correct SOAC
loader, module contract, or enforcement support is absent, it raises
`StrictRuntimeUnavailableError`. Recognizing a source comment or copying a
code flag must never substitute for that authenticated execution context.

An ordinary CPython process can host strict and ordinary modules together when
it has the required SOAC runtime and interpreter support. An authenticated
selected module may not fall back to ordinary execution when its authority
is missing or stale.
No inference from a module's name, location, type annotations, optimization
mode, or prior profile is sufficient to opt it in.

### Internal guarded code and dynamic-code semantics

The pinned CPython runtime retains `CO_FUTURE_STRICT` as an internal code
ownership guard. Despite its historical name and retained parser recognition,
it is not source-policy opt-in. The trusted raw compiler sets the guard while
compiling verified source; it does not insert a future import, rewrite the
source bytes, or introduce a Python-visible feature binding.

Source-comment resolution and exact class ranges must be authenticated before
lowering removes comments or rewrites annotations. The resulting module policy
and source ownership are explicit compiler/runtime inputs. Ordinary future
imports retain their own Python syntax and placement rules.

Nested source code retains its authenticated module/source ownership; its
presence in that code tree does not override the resolved class-specific
`checked_attr` selection. Separately imported modules resolve their own source
policy. Compile them without inheriting caller flags so ordinary imports do
not acquire a guarded execution marker from their importer.

An inherited or manually supplied `CO_FUTURE_STRICT` bit does not create a
module contract. Strict-flagged dynamic code may execute only when its
authenticated compiler provenance and actual globals both match an existing
strict module; targeting an ordinary or unregistered namespace fails
explicitly. Neither that bit nor comments in dynamically compiled text can
change resolved policy, certify new classes/functions, or mint optimizer
facts. Ordinary dynamic writes still obey the installed storage policy.
New contracts require a separately authenticated compiler/construction path.

One narrow exception is CPython's own annotation-format replay.
`annotationlib` implements `FORWARDREF` and `STRING` by recreating the
annotation-provider function with its original code and a synthetic globals
mapping. If that code inherited `CO_FUTURE_STRICT`, ordinary foreign-globals
rejection would incorrectly break both formats. Supply an authenticated,
interpreter-owned replay capability bound to the originating strict module,
exact registered annotation/evaluation callback and code, object owner when
present, approved temporary mapping, closure, and supported public
`FORWARDREF`/`STRING` request; alternatively emit a separately verified,
non-strict annotation-replay code object. The authenticated cloned callback
must also accept its actual internal `VALUE_WITH_FAKE_GLOBALS` argument. The
original callback's ordinary probe of that format still executes under its
real strict globals.

The same replay path supports compiler-generated evaluate functions for type
aliases and type-parameter bounds, constraints, and defaults. Their explicit
owner may be `None`, but originating-module and exact-callback provenance
remain mandatory. Preserve their lazy evaluation and their normal non-dict
return values rather than treating them as annotation dictionaries.

User-written callbacks need a separate replay-eligibility proof, not a forged
compiler annotation-provider role. The current code-return API admits an
actual source-owned callback only after validating its original code and
closure layout, exact live owner, and every nested native code pointer against
the authenticated compiler catalogue. Class bodies, generic-construction bodies, and
unrepresented native nodes are rejected. Compiler providers retain their
separate signature and capture-projection validation. Derived replay code has
no source ID, strict flag, function owner, or JIT authority; the original
callback and its ordinary call behavior remain unchanged. Removing function-
level type checks also removes the check-only replay exclusion; exact source,
closure and construction restrictions still apply. A custom callback that
directly implements the requested format does not need this replay path.

Each replay capability is freshly minted, unavailable to Python code, bound to
one temporary function and invocation, and valid only while that approved
replay is active. Lexically nested lambdas, comprehensions, and generators
belonging to its verified code-object tree may execute under that replay; an
object that legitimately escapes receives its own identity-bound ordinary
replay-derived execution status, never transferable strict authority. The
capability is not inherited by unrelated callbacks, dynamically compiled code,
other threads, arbitrary functions using escaped synthetic globals, or later
unrelated invocations, and is invalidated on success or exception. Replay and
its derived objects execute ordinary dynamic annotation code, never acquire the
owner's strict execution or mutation authority, and cannot publish strict
optimizer facts. Arbitrary
`types.FunctionType(strict_code, foreign_globals)` remains forbidden.

To intentionally execute ordinary dynamic code against a foreign namespace,
compile it without inheriting the calling strict frame's future flags:

```python
ordinary_code = compile(source, "<dynamic>", "exec", dont_inherit=True)
exec(ordinary_code, ordinary_globals)
```

Ordinary code objects can also execute against strict globals but remain
subject to the module's write barrier. The runtime must enforce authenticated
ownership at function construction, module execution, transformed/JIT entry,
and CPython's directly inlined frame-entry paths; guarding only a public
evaluation helper is insufficient. `types.FunctionType`, `code.replace`,
marshaled code, a forged `__module__` string, a manually supplied feature bit,
or a matching global-dictionary shape never establishes a strict capability.

This also applies when an unchecked function's `__code__` is replaced during
initialization with another function's original strict code. The assignment
does not transfer the donor's execution owner, even when both functions share
the same globals; calling that transplanted code fails explicitly. Replacing
an unchecked, not-yet-frozen function with ordinary dynamic code remains a
different, permitted path. A failed strict initializer publishes neither a
module seal nor usable source-entry facts and cannot retry through ordinary
execution.

## Module lifecycle

Every selected module has an authenticated runtime lifecycle. Readiness and
binding sealing are distinct: `strict_assign=false` completes admission without
freezing module globals or ordinary free functions. Selected class/field/method
contracts and failed/terminal ownership checks remain enforced independently.
The following binding-seal lifecycle applies when `strict_assign=true`:

```text
DISCOVERED -> INITIALIZING -> SEALING -> SEALED -> TEARING_DOWN -> CLEARED
                    |             |
                    +-----------> FAILED
```

During `INITIALIZING`, normal module initialization may bind and rebind names,
execute decorators, construct registries, perform imports, and have ordinary
import-time side effects. Existing module-level loops, mutually exclusive
assignments, dataclass decorators, and initialization-time class configuration
do not require artificial purity rules.

Participating classes have an earlier instance-admission boundary.
Authenticated offline facts propose the declared fields, class-default shadow
catalog, name policies, bases, and transformation phases. An explicit,
single-use construction handle binds that proposal to the actual module
execution, lexical class plan, namespace function, metaclass, and construction
phase. The real type allocator retains source-requested storage and installs
the native Pending barrier before `PyType_Ready`, `__set_name__`, and inherited
`__init_subclass__` callbacks. Callbacks may observe the type, but allocation
and `__class__` reassignment into it are forbidden until final admission.
Source-function ownership remains active during construction. A mutable
helper global, guessed name, namespace attribute, or thread-local value is not
construction authority.

Unknown metaclasses, decorators, and framework-managed classes automatically
remain dynamic before physical allocation or any irreversible restriction. A
recognized replacement transformation receives a linked phase-specific handle
for its own actual type allocation. In particular, `dataclass(slots=True)`
gives its fresh replacement an independent linked Pending barrier. The original
retains its observed storage, not the replacement's slots or contract. Only an
unselected provisional type without a permanent type contract may become
dynamic; independent method-metadata and inherited contracts remain intact.

Validate the actual final decorated result and bind its own `Self` and field
requirements without inventing a layout. Final admission installs and seals
the selected class/method constraints before enabling instances or releasing
temporary admission operands. This applies during module initialization as
well as in later factories. Existing defaults may be configured before that
class admission, not until some later module seal. No subsequent mutation may
extend or reclassify published storage or invalidate installed name policies.

The module enters `SEALING` only after its complete body and recognized
class/function finalizers have succeeded. This state activates the global write
barrier and freezes still-pending eligible owned functions. Classes and adopted
methods already sealed at final class admission stay sealed. Before each
class/function freeze, reserve its private annotation-cache storage and freeze
its annotation-provider binding without calling the provider. Module sealing
fixes previously unresolved module-only nominal targets once; it cannot replace
established targets or reopen metadata. Active calls retain their post-binder
target snapshot through return. Reentrant user code cannot change a final
binding during the transition. Module-seal facts become available only when
all required barriers are installed and the state changes to `SEALED`.

The corresponding participating-class lifecycle is:

```text
PLANNED -> PREPARED -> PENDING -> DECORATED -> VERIFIED -> ENFORCED + SEALED
```

A factory or nested definition executed after its module seals gets a fresh
construction handle and class identity on every execution. Its class follows
the same final-admission boundary as an initializing module's class; it never
reopens the owning module's lifecycle.

Nothing may optimize an initializing or sealing strict module as though it were
sealed. In particular, circular imports and functions called from module
top-level code cannot consume unavailable module-seal facts; installed checked
boundaries still apply. Initialization or sealing failure prevents a module
seal and terminalizes unfinished work, preserving normal import failure. It
does not revoke any already published class or function contract.

A sealed module never becomes dynamically mutable, never loses its strict
contract, and is never deoptimized to accommodate monkeypatching or an
incompatible C extension. `TEARING_DOWN` is the only transition out of
`SEALED`: it blocks new trusted execution, quiesces live users, and then permits
the interpreter-owned clearing required by module garbage collection and
shutdown. A privileged teardown clear is legal only after strict code can no
longer observe the cleared state.

The loader authenticates the internal guarded-code marker, resolved source
policy, module identity,
protected globals, and registered `INITIALIZING` contract before entering the
module body. Guarded code without that authenticated contract is rejected;
a claimed selected contract without matching guarded compiler provenance is
also rejected. No source future or Python-level initialization call can
replace this boundary.

## Global binding contract

The binding restrictions in this section apply when `strict_assign=true`.
Selected modules with `strict_assign=false` retain ordinary binding mutation,
subject to authentication/terminal ownership and any independently installed
class or storage contracts.

### Declared mutable names

A global is **mutable** if and only if a `global NAME` statement appears in an
actual lexical scope of the module's parsed source. Statements inside module,
function, nested-function, and class scopes all count when their resolved target
is the module binding. The declaration is syntactic: the function containing it
does not have to run. Any previously absent name may receive its first binding
after sealing, but that new binding immediately becomes final unless the name
belongs to this statically declared mutable set.

```python
# soac: module(strict_assign=true, checked_attr=true)

global requests

LIMIT = 100
requests = 0
cache = {}


def record() -> int:
    global requests
    requests += 1
    cache[requests] = True
    return requests
```

After sealing:

```python
module.requests = 20      # allowed: requests was explicitly declared global
module.cache["x"] = True  # allowed: the object is mutable, its binding is not
module.LIMIT = 200        # StrictMutationError
module.cache = {}         # StrictMutationError
module.unlisted = 1       # allowed: appends a new, immediately final binding
module.unlisted = 2       # StrictMutationError: the binding already exists
del module.unlisted       # StrictMutationError: deletion would permit rebinding
```

An existing nested `global requests` already declares the binding mutable; the
module-level declaration is useful when the name will be changed only by stock
Python or native extensions. Normal package-child publication appends a new
binding and does not require a `global` declaration. `nonlocal` never declares
a module binding mutable unless normal scope resolution independently
identifies the target as a module global through a separate `global`
declaration.

Assignments, annotated assignments, augmented assignments, imports, class and
function definitions, exception bindings, comprehension boundaries, and deletes
use their normal Python lexical scope rules. A dynamically executed string
containing `global NAME` does not retroactively add `NAME` to the statically
declared mutable set.

### Initialization and deletion

Immutable names may be assigned more than once during `INITIALIZING`; the value
present at the seal boundary is their final binding. This avoids banning normal
conditional initialization and import-time construction.

After sealing, a declared mutable name may be reassigned or deleted. Deletion
removes the visible dictionary key, but its compiler-owned slot and permission
remain reserved. Reading the absent global preserves ordinary builtin fallback
and `NameError` behavior. Reassignment restores the same logical binding slot.

A final name may never be deleted: otherwise deletion followed by first
insertion would bypass its immutable identity. A previously absent name may be
appended through module attribute assignment, `globals()`, `module.__dict__`,
`exec`, normal import publication, or a supported native dictionary API. Its
first successful insertion is atomic and immediately establishes its final
identity unless the original module source declared that name mutable.

Stable module metadata names such as `__name__`, `__spec__`, `__package__`,
`__loader__`, `__file__`, `__cached__`, and package `__path__` are initialized
before sealing and are immutable bindings by default; mutable values they
reference remain mutable. `__annotations__` and `__annotate__` may first
materialize after module initialization through the ordinary append-once rule.
Frozen classes and functions instead use their own pre-reserved private
annotation-cache slots; populating those slots never authorizes rebinding
ordinary protected metadata.

### Packages and late submodule imports

CPython publishes `package.child` by assigning the child module onto its parent
package. Because an absent global may be appended after sealing, ordinary
import machinery can publish a previously absent child without pre-enumerating
submodules, reserving import-only slots, or requiring `global child`:

```python
# widgets/__init__.py
# soac: module(strict_assign=true, checked_attr=true)

# widgets/lazily_loaded_child.py exists but has not been imported.
```

After `widgets` seals, `import widgets.lazily_loaded_child` appends the child
binding normally. It immediately becomes final: assigning a different object,
deleting it, or replacing it with a different module fails unless the package
explicitly declared `global lazily_loaded_child`.

This works equally for source, bytecode, native-extension, namespace,
archive-backed, newly created, and custom-finder children; mutating the
existing `__path__` sequence or invalidating import caches requires no
strict-package discovery catalog. Only the stable-index, first-binding-wins
rule distinguishes this operation from ordinary module publication.

An existing final package export remains a real collision. If
`widgets.child = placeholder` was already present when the package sealed,
importing `widgets.child` cannot replace it; import the child during
initialization or declare `global child` if replacement is intentional.
Removing a published child from `sys.modules` likewise does not authorize
replacing its final parent binding.

CPython normally executes a child module and installs it in `sys.modules`
before assigning it onto its parent. A narrow existing-binding preflight can
reject an already-final collision before the child body runs, but it does not
reserve absent names or authenticate ordinary first publication. If concurrent
code claims an absent binding while the child initializes, first-writer-wins
still applies; publication fails rather than overwriting the final value.

### Mutable binding is not object immutability

`LIMIT = SomeObject()` freezes the identity stored in `LIMIT`, not the entire
object graph. Lists, dictionaries, instances, closure-cell contents, default
argument contents, descriptor state, context variables, and imported stock
objects retain their ordinary mutation rules unless a separate checked/frozen
contract explicitly covers them.

Likewise, a final binding to a stock function does not freeze that function's
`__code__`; a final binding to a stock class does not freeze its methods or
MRO. The optimizer must distinguish a stable reference from a stable target.

## Enforcing append-only module bindings

### Preferred Python-facing design: a protected live exact dictionary

For Python-facing interoperability, SOAC should preserve these normal
identities when its explicit native-compatibility boundary allows them:

```python
module.__dict__ is module.function.__globals__
```

When `globals()` executes in that module, it returns the same real dictionary.
The dictionary remains a CPython exact `dict`, including for modules that must
use an ordinary globals dictionary for source-backed named generator frames.
This goal is conditional: exposing the exact authoritative dictionary is not
compatible with unrestricted use of every native dictionary-mutation API.

The pinned vendored CPython gains an explicit per-dictionary strict policy. All
mutation entrypoints consult the policy before changing a protected dictionary:

- `module.NAME = value`, `setattr`, `delattr`, and import machinery;
- `module.__dict__[name]`, `globals()[name]`, and
  `function.__globals__[name]`;
- `dict.update`, `clear`, `pop`, `popitem`, `setdefault`, merge, and `|=`;
- interpreted `STORE_GLOBAL`/`DELETE_GLOBAL`, `exec`, and `eval` writes;
- C-extension calls through `PyModule_GetDict`, `PyDict_SetItem`,
  `PyDict_DelItem`, and other supported mutation APIs that can report an
  error; and
- SOAC's raw indexed-global insertion, replacement, deletion, and ordinary-dict
  fallback paths.

Preserve the public `PyDictObject` ABI and field offsets. Native extensions see
that layout through CPython headers, and `soac_jit_runtime` mirrors it directly
in `RawPyDictObject`. Prefer a dictionary-identity sidecar or a compatible
existing metadata bit over inserting a new struct field; any incompatible
layout change would require an explicit ABI break and a complete rebuild/audit
of every native consumer.

### Stable append-only global storage

Separate name lookup from the storage position of its value:

```text
index_map: name -> stable integer index
values:    [value_at_0, value_at_1, value_at_2, ...]
```

A new name obtains the next index and appends one value. The hash table used by
`index_map` may grow, resize, or rehash normally: moving its hash buckets never
changes the index assigned to an existing name. Existing binding indexes must
never be reassigned, renumbered, compacted, or reused. A compiler-known name
that is physically indexed but logically absent fills its existing value slot
instead of acquiring another index. Every statically declared mutable name also
receives its own stable index even when it has no initial visible binding.

A declared mutable name that is deleted leaves a tombstone in its existing
slot; reinsertion restores the same stable index. Stable index order and Python
insertion order are distinct: first filling an old unbound slot, or reinserting
a deleted mutable name, appends the visible key at the current end of the
dictionary's logical iteration order.

The values array may itself grow and move **only** if every optimized consumer
reloads its current base before indexed access, no borrowed slot address
survives an operation that can append, and concurrent readers cannot race its
reallocation. SOAC's ordinary indexed-global helpers already reload the
dictionary's value pointer on each access. Some existing prepared
optimizations additionally cache raw `ma_keys` and `ma_values` addresses;
strict mode must remove cached hash-map/`ma_keys` pointer identity assumptions
unconditionally because a valid index-map rehash may replace that allocation.
Value consumers can either reload the current `ma_values` base or use
reserved/segmented/nonmoving value storage when an optimization needs an
absolute slot address. Rehashing the name index alone never invalidates a
logical global binding.

SOAC's current custom indexed dictionary is not automatically append-safe: its
slow helper documents that an unprofiled key can promote the dictionary, and
ordinary CPython dictionary growth can compact deleted entries or discard a
split value array. Extend the actual indexed representation to preserve stable
name-to-index assignments instead of claiming ordinary insertion already does
so. A source-backed named-generator module that currently requires ordinary
globals needs the same stable-index contract or must forgo optimizations that
depend on it.

All ordinary dictionary-facing operations must observe one coherent namespace:
lookup, containment, length, iteration, insertion order, views, copying,
`PyDict_Next`, supported C APIs, monitoring, and legitimate dictionary
notifications include all appended bindings. If an exact `dict` cannot expose
that representation and its mutation policy soundly, reject the strict
configuration rather than silently weakening the stable-index guarantee.

An exact module dictionary also accepts non-string keys through mapping/C APIs.
Either support those keys in the same stable-index model or representation-safe
side storage while preserving every optimized string index, or explicitly
reject them as a documented strict-mode compatibility restriction. Never let
a non-string insertion silently convert the indexed dictionary and renumber its
existing globals.

Single-key mutation APIs that can return an error reject a forbidden write
before touching the dictionary. A new-name insert, explicitly mutable update,
or permitted delete must commit atomically against the current binding state.
A multi-key operation is allowed only if it can stage a stable source
snapshot, validate every destination key without executing user code, exclude
reentrant destination mutation, and commit against the same protected-policy
epoch. Validation preserves input order and duplicate writes: an update that
writes a previously absent final name twice must fail atomically rather than
deduplicating the pair into an incorrect last-wins insertion. Arbitrary mapping
iteration, custom hashing, equality, finalizers, and `__del__` callbacks can
otherwise invalidate naive "prevalidate then write" logic. The initial
implementation should reject unsupported bulk updates outright rather than
promise atomicity it cannot enforce; this is an intentional strict-mode
restriction. Protected dictionaries must not silently fall back to unprotected
or index-changing writes during generator handling, reload, cache restoration,
or shutdown.

The current protected-module annotation setters have a related compound-write
restriction. Assigning or deleting `module.__annotations__` also removes
`__annotate__`; assigning a non-`None` `module.__annotate__` also removes
`__annotations__`. During initialization they retain CPython's sequential
writes and finalizer order. Once sealing starts, these compound setters accept
only a first primary insertion with an absent companion binding; other shapes
fail before either binding changes, even when their names are explicitly
mutable. Releasing a replaced value could otherwise run a finalizer that adds
the companion between writes. A one-key dictionary mutation remains available
for an explicitly mutable metadata name. Lazy getter publication is still an
ordinary append-once insertion, and setting `__annotate__ = None` retains its
ordinary single-key behavior. This restriction does not authorize delaying or
reordering ordinary finalizers.

Plain CPython bytecode already routes `STORE_GLOBAL` through `PyDict_SetItem`,
so a dictionary-level policy can cover interpreted stores. SOAC's existing
indexed stores can write directly to dictionary storage and therefore need their
own mandatory barrier. In particular, `soac_jit_runtime`'s
`store_global_indexed_body` explicitly skips insertion bookkeeping, dictionary
watchers, and versions. Its strict first-binding path must instead publish the
new logical entry, insertion order, required notifications, and any
absence-dependent invalidation before the value becomes visible. Dictionary
watchers, type versions, module `__setattr__` hooks, mapping proxies, dict
subclasses, and Python-only wrappers cannot enforce this contract by
themselves. Pinned CPython even reports dictionary-watcher exceptions as
unraisable; a watcher cannot veto the underlying mutation.

### Irrevocable sealing and the native C API boundary

One relevant stable C API returns `void` rather than an error code:

```c
void PyDict_Clear(PyObject *mapping);
```

CPython documents `PyDict_Clear` as unconditionally emptying its dictionary,
and its stable-ABI signature cannot return an exception or a failure status.
The ordinary Python `dict.clear()` wrapper also calls this function and then
returns `None`, so a strict-aware Python wrapper must reject a protected
dictionary **before** reaching the native `void` function.

There is no sound way to satisfy all three properties simultaneously:

1. Final bindings stay immutable for the entire life of a sealed module.
2. Its authoritative namespace is exposed as a real exact `dict`.
3. Arbitrary C extensions may apply every existing dict-mutation API to it.

Property 1 is mandatory. The runtime must never resolve this conflict by
revoking the module, deoptimizing its callers, making a final binding mutable,
silently ignoring a C mutation, or leaving an exception pending behind a `void`
API.

A protected live exact dictionary is therefore valid only with an explicitly
narrowed native compatibility contract: native code may inspect it and use
supported mutation entrypoints that can reject forbidden writes, but invoking
`PyDict_Clear` or an equivalent non-rejectable mutation on strict module,
strict class, keyword-default, or frozen-builtin storage is unsupported. This
requires a trusted extension boundary; it is not compatibility with arbitrary
native mutation.

If arbitrary third-party C code must receive strict objects, the authoritative
strict storage must instead be hidden behind a protected/read-only projection,
compiler-owned cells, or another representation that cannot leak the real
dictionary through `module.__dict__`, `function.__globals__`, `PyModule_GetDict`,
or `PyType_GetDict`. That gives up some exact-dictionary/C-API compatibility,
but preserves permanent strict semantics. If neither enforcement boundary is
available, the loader must reject strict mode rather than silently weaken it.

`PyFunction_SetVectorcall` also returns `void`, but it is a different case:
CPython explicitly requires extensions using it to preserve the behavior of
the original callable. The vectorcall pointer is mutable implementation state,
not an immutable semantic fact. SOAC may use a stable strict body/trampoline or
observe the current compatible entry without unsealing its owner. A native
replacement that changes the function's behavior violates that API contract
and is unsupported for strict objects.

Interpreter-owned teardown still follows `TEARING_DOWN -> CLEARED`, after live
strict execution is impossible. First module annotation bindings use ordinary
append-once insertion; frozen classes and functions lazily populate only their
pre-reserved private annotation-cache slots. None of these operations revokes
or changes the semantics of a live sealed module.

The supported threat model includes Python code, read-oriented C extensions,
and explicitly supported C mutations that can reject forbidden changes.
Arbitrary native memory writes, `ctypes` corruption, behavior-changing
vectorcall replacement, and non-rejectable native mutation of authoritative
strict storage are outside that boundary.

### Alternative: private cells and a projected module namespace

A second possible implementation is compiler-owned final/mutable global cells
plus a read-only or copied module namespace projection. This makes strict
global loads cheap and avoids relying on normal dictionary mutation hooks, but
changes `module.__dict__`, `globals()`, `function.__globals__`, C-extension
integration, and synchronization of explicitly mutable globals.

CinderX demonstrates one relevant facade tradeoff: its strict module returns a
fresh copy from `__dict__` rather than exposing its internal globals dict. That
is not sufficient protection for SOAC's current exact-live-dict module model.
Choose private cells only if a measured implementation justifies the explicit
compatibility break and every escape route to the authoritative cells is
closed.

## Automatic class capabilities

Every source-defined class in a strict module is an automatic classification
candidate, not an automatically slotted or frozen class. Offline `ty` analysis
proposes source-bound class contracts; actual type construction and final
decorator adoption determine which capabilities can be installed and sealed.
One project-level policy selects this behavior without requiring SOAC
decorators on individual classes.

For example:

```python
# soac: module(strict_assign=true, checked_attr=true)


class Point:
    x: float
    y: float

    def __init__(self, x: float, y: float) -> None:
        self.x = x
        self.y = y
```

`Point` retains its ordinary instance dictionary and weak-reference behavior.
When a stable indexed-dictionary capability is installed, `x` and `y` occupy
SOAC-assigned prefix positions in that actual dictionary. They are not newly
invented slot descriptors. Uninitialized fields are absent from `vars(point)`;
assignment and deletion update the real dictionary. Additional ordinary
instance names and supported non-string dictionary keys use overflow storage
without renumbering the declared prefix. Source-requested `__slots__` instead
uses the actual final class's member descriptors and verified native offsets.

Keep these capabilities independent:

| Capability | Required evidence and enforcement |
| --- | --- |
| Strict module bindings | Authenticated module execution and protected append-only globals. |
| Frozen class behavior | Final class identity, stable relevant MRO/descriptor dependencies, and type/dictionary barriers. |
| Frozen callable target | Owned function identity, code, consumed defaults/closure bindings, private entry, and callee environment. |
| Checked field | Policy-selected field value contract and complete supported storage-write enforcement. |
| Method dispatch family | Verified binding/ABI, inherited method-slot agreement, protected lookup, and frozen targets. |
| Stable indexed dictionary | Construction-installed fixed field prefix, coherent overflow, owner-aware writes, and verified replacement policy. |
| Native object slots | Source-requested real slot layout with actual member offsets, inheritance, and GC metadata. |

A class can have frozen methods and a real dictionary without checked field
values. A nominal field check can accept a dynamic subclass instance without
giving it a strict layout or method table. A class that cannot enforce one
capability remains generic for that operation; strict module membership does
not force all of its classes to participate.

### Offline field catalog and actual construction

The authenticated offline class contract records logical declared fields,
annotation-only fields, ordinary class defaults, method kinds, inherited
members, recognized transformations, and relevant uncertainty. It includes
the pinned checker, source, resolved source policy, dependencies, and environment in
its identity. `ty` supplies logical facts, not physical field offsets or
method-table indexes. CPython constructs the source-requested storage, and
SOAC validates actual final members before admission. Optional fixed-layout
capabilities require their own separately verified publication.

Receiver-write evidence must identify the bound receiver, not merely a name
spelled `self`; nested functions contribute only when they capture that
receiver. Aliases, nonliteral `setattr`, dynamic factories, unresolved imports,
`Any`, `Unknown`, ignored checker errors, and unsupported transformations do not
prove additional fixed fields. Such names can remain ordinary overflow or
dynamic operations; programmers need not add per-class slot annotations to
obtain correct behavior. Missing a fixed field fact must not change an
otherwise valid dictionary-bearing class into an error.

An annotation-only `value: T` can propose an ordinary instance field, but
neither its presence nor its value type is assumed. Both `value = expression`
and `value: T = expression` keep their actual class bindings and initially
absent instance overrides. Ordinary source dictionaries promise no fixed
indexes. A separately published indexed-storage capability may reserve an
`UNSET` prefix position; all writers must then use that same declared position.
A recognized dataclass transformation supplies its real field catalog,
excluding `ClassVar` and `InitVar` pseudo-fields.

For participating names, a recognized `typing.ClassVar` denotes protected class
state: instance attribute assignment is rejected, while a direct dictionary
entry may exist but cannot override attribute lookup. Recognize direct,
qualified, imported-alias, future-stringized, and statically parseable string
forms through side-effect-free source/provenance analysis and validate the
actual imported identities. Never invoke a lazy annotation provider or
evaluate an annotation expression to classify storage or recover checker
facts. Unrelated unresolved forward references retain normal lazy behavior.
An ambiguous `ClassVar` or decorator identity declines the affected capability
before irreversible construction; it cannot gain authority from a later
annotation evaluation or retroactively change a published layout.

Classify each actual MRO entry as a plain class value, instance method, static
method, class method, data descriptor, non-data descriptor, declared field,
cached descriptor, or dynamic member. A plain callable object without
descriptor behavior is class data; invocation loads the instance override
before the class default and never implicitly binds a receiver. Methods,
`ClassVar` names, descriptor-owned names, and interpreter-owned dunders do not
acquire ordinary class-default shadow fields merely from a spelling match.
Mutable descriptor types or behavior cannot supply permanent precedence facts.

Genuine declared instance fields take precedence over inherited non-data
methods. This is distinct from protecting an effective method against arbitrary
instance monkeypatching. Never mask a real data descriptor, including a
read-only property whose setter raises. Preserve ordinary CPython errors for
invalid explicit slots and incompatible physical bases; unsupported otherwise
valid classes automatically remain dynamic before publication.

CPython 3.15's `__static_attributes__` remains Python-visible metadata with its
normal compiler behavior. It can contribute hints, but its literal-`self`
rules, nested scopes, global/nonlocal bindings, and private-name treatment do
not prove a complete source-authenticated class contract.

### Stable instance dictionaries and replacement

Current source-class participation preserves ordinary dictionary storage;
class sealing alone does not reject whole-dictionary replacement. Any selected
field checks must attach to the actual incoming dictionary before publication,
and an escaped old dictionary retains its own installed obligations. The
indexed storage capability described below is optional and deferred for this
interpreter-enforcement milestone.

A class with that capability keeps one authoritative real instance
dictionary. Its protected schema reserves a stable inherited prefix, while
ordinary added names and non-string keys live in overflow. Reserving a field
does not insert a visible key. First assignment, deletion, reinsertion,
iteration, views, copying, serialization, and `PyDict_Next` preserve the
dictionary's ordinary visible contents and insertion order independently of
physical index order. Clearing an instance dictionary may clear its values
while retaining the hidden schema; it does not revoke the layout.

Dictionary growth, materialization, and overflow rehash must never renumber
declared fields. Generated accesses reload a movable values-array base after
operations that can move it; prepared addresses cannot outlive that guarantee.
Each visible field has exactly one owned value reference, not independent
hidden-slot and dictionary copies. GC, watchers, refcounting, weak references,
and supported native APIs must see the same state.

Whole-dictionary replacement requires an explicit construction policy. The
compatibility-preserving path normalizes the incoming dictionary in place,
validates its checked fields and ownership before publication, and preserves
`instance.__dict__ is replacement` plus references retained to the old
dictionary. Shared owners, aliases, reentrancy, and non-string keys require
sound handling. Copying the new dictionary into the old one is not equivalent.
A class whose replacement behavior cannot support fixed indexes remains on
generic storage, or uses an explicitly selected rejecting/cooperative-adapter
policy before publication. Intercept the shared `_PyObject_SetDict` and
managed-dictionary attachment seam, including direct native generic setters.

### Class defaults and fixed instance override fields

An ordinary class-data binding remains a class binding even when an instance
overrides it:

```python
# soac: module(strict_assign=true, checked_attr=true)


class A:
    foo = 1


a = A()
assert A.foo == 1
assert a.foo == 1
assert vars(a) == {}

a.foo = 2
assert A.foo == 1
assert a.foo == 2
assert a.__dict__["foo"] == 2

del a.foo
assert a.foo == 1
assert A.__dict__["foo"] == 1
assert vars(a) == {}
```

Reserve one fixed-prefix position for every eligible class-data name in the
construction contract before a type-creation callback, instance, subclass,
or native consumer can observe the participating layout. It is hidden only
while unpopulated; a stored override is the real visible dictionary entry.
Each instance starts with the field `UNSET`. Reading a populated field returns
that instance's value; reading an empty field falls back to the actual
receiver class's current class/MRO default, which becomes frozen when the
owner seals. Assignment replaces only the instance field. Deleting a populated
field restores the empty state and class fallback; deleting an already-empty
override raises `AttributeError`, as it does for an ordinary instance with no
such dictionary entry.

The class dictionary keeps its original value: `A.__dict__["foo"]` remains
`1`, not a synthesized descriptor. Separate instances receive independent
overrides in their own dictionaries; verified indexed operations avoid the
attribute-name hash lookup. Attribute writes, direct dictionary mutation, and
supported C APIs must update the same authoritative value. The fixed prefix
does not grow after publication; additional names use its declared overflow
policy. A truly slotted class retains its actual source-requested descriptor
and class-default behavior rather than receiving fabricated shadow slots.

Assignments retain ordinary Python value semantics: `a.foo = "two"` is legal
even when the default was annotated `foo: int = 1` unless the declaring class's
resolved `checked_attr` rule selects and enforces that field. Opting out in a
subclass never revokes its inherited checks. The indexed storage remains
GC-visible and follows ordinary per-field `INCREF`/`DECREF`, exception,
deletion, and reentrant-finalizer rules; instance destruction may use the
approved physical-layout field-release order described below. Python
object-state discovery, `copy.copy`, `copy.deepcopy`, pickling, and
reconstruction must retain populated shadow values and distinguish them from
empty default-fallback fields; invisible physical storage must not silently
disappear during a copy or round trip.

Protected methods and `ClassVar` names reject attribute assignment, including
`setattr` and `object.__setattr__`, but permit direct same-name entries in an
available instance dictionary. Those entries remain visible to dictionary
operations and are ignored by all attribute reads and method lookup. This
explicit strict deviation makes method protection independent of whether the
instance has a dictionary. Descriptor-owned fields instead preserve their
descriptor setters and backing-storage behavior. A genuine declared field
overriding an inherited non-data method follows its field policy, not the
protected-method rule.

Ordinary subclasses and automatically dynamic classes do not inherit strict
receiver capabilities merely because storage is physically inherited. Their
ordinary method shadowing, dictionary authority, live MRO/defaults,
descriptors, and source-requested slots remain visible on the generic path.
The runtime must preserve any already-installed physical invariants without
mistaking them for frozen dispatch authority. An ordinary subclass with
`__slots__ = ()` may still inherit a real dictionary; it is not automatically
dictless. Physically inherited strict fields retain their documented
field-release-order exception without authorizing an optimized receiver fact.

### Instance destruction and finalizer order

A fixed-layout strict instance can release its populated public, private, and
inherited fields in deterministic physical-layout traversal order when it is
deallocated or cleared by cyclic garbage collection. That order can differ
from the field-release order of an equivalent ordinary instance dictionary;
stock shared-key storage does not always follow visible dictionary insertion
order either:

```python
# soac: module(strict_assign=true, checked_attr=true)

events = []


class Recorder:
    def __init__(self, name):
        self.name = name

    def __del__(self):
        events.append(self.name)


class Record:
    first: Recorder
    second: Recorder


record = Record()
record.second = Recorder("second")
record.first = Recorder("first")
del record

# One possible stock instance-dictionary order: ["second", "first"]
# Allowed strict physical-layout order: ["first", "second"]
```

This difference is explicitly approved: finalizers and weak-reference
callbacks triggered by the released field values may run in the corresponding
different relative order and can observe different sibling fields already
cleared. No extra insertion-order tracking is required solely to reproduce
stock destruction order; ordinary visible dictionary insertion/reinsertion
order is still preserved.

Every populated field must still be cleared before its exactly-once `DECREF`;
empty fields release nothing. Preserve callback execution, resource cleanup,
exceptions, GC traversal, cyclic clearing, reentrancy, and resurrection. The
receiver's own finalizer and weak-reference phases, ordinary field
assignment/replacement/deletion, independently scheduled cleanup of unrelated
objects, and module/class/function teardown retain their existing guarantees.
Different field-release order never authorizes dropping or duplicating a
finalizer. Ordinary or dynamic subclasses physically inheriting strict field
storage retain the same layout-order exception for those fields without
acquiring strict dispatch or optimization facts.

### Weak references and special fields

Preserve each class's ordinary source-requested weak-reference support.
Dictionary-bearing classes keep their normal weakrefs; explicitly slotted
classes and `dataclass(slots=True, weakref_slot=...)` keep the behavior of their
actual bases and options. Do not add or remove `__weakref__` merely because the
module is strict.

Explicit source `__slots__` remains real Python slots. Requesting `__dict__`
does not require a dynamic escape hatch and does not inherently preclude
protected methods, checked values, or verified native-slot accesses. A slotted
subclass can also inherit a real dictionary. Validate the actual final member
descriptors, offsets, dictionary presence, and compatible native bases rather
than converting an offline field declaration into a slot descriptor. Invalid
slot names, conflicting class bindings, and incompatible physical bases retain
their ordinary CPython errors; otherwise unsupported layout facts remain
dynamic instead of silently rewriting the class.

Participating layout/receiver policies reject incompatible `__class__`
reassignment before it invalidates a stored field index or method table. An
ordinary object accepted by a nominal field check has no such automatic
protection: a later type-dependent operation needs a fresh guard after effects
that can change its actual class.

### Inheritance

A participating subclass of a compatible strict base inherits its base's
actual fixed dictionary prefix or native member offsets. Compatible declared
fields extend that prefix without renumbering it; source-requested native
slots follow CPython's actual layout rules. The storage contract remains valid
even when inherited constructors initialize the fields. Method-family slot
positions are a different layout and need independent binding/ABI validation.

Inherited class-data override fields retain the base's existing prefix index.
A participating subclass may install its own frozen plain-data class default
without allocating a second field; an empty instance field uses the actual
receiver class and MRO to select the appropriate default. A declared field can
override an inherited non-data method, but cannot bypass a data descriptor.
Incompatible transitions between class data, `ClassVar`, methods, and
descriptors require automatic dynamic classification before publication, or
rejection once they would violate an installed contract.

A candidate may inherit a stock builtin or stock class, including a
dictionary-bearing base. Classify the actual resulting layout, attribute
hooks, MRO, and metaclass for each requested capability. Dictionary presence
alone is not an exclusion. Inherited stock descriptors and methods remain
dynamic unless their relevant owners and behavior can be independently proven
immutable; a frozen immediate class does not freeze a mutable ancestor.

An inherited plain class-data default owned by a mutable stock ancestor is not
a frozen strict default. A strict receiver may use a shadow field for that name
only when every read/set/delete validates the live MRO and descriptor
precedence or selects ordinary generic behavior; replacing the stock ancestor's
value with a property or other descriptor must remain visible. Never publish
an unguarded strict shadow-default fact for a mutable stock owner.

An unsupported metaclass, dynamic intermediate base, custom lookup hook, or
unresolved descriptor dependency automatically declines affected capabilities
before irreversible construction. The class retains ordinary behavior while
its surrounding module can still seal. Incompatible multiple slotted bases
continue to produce CPython's physical-layout error. Never describe a
dictionary-bearing instance as slot-only or require a per-class annotation
merely to keep an unsupported framework class working.

Ordinary Python remains free to subclass a non-final strict class. A stock
subclass can introduce `__dict__`, override a method, install descriptors, add
custom hooks, or change its own MRO. Such an instance is **not** a strict
receiver even if `isinstance(value, StrictBase)` is true. Strict code must
either guard the actual receiver's strict capability or use ordinary Python
dispatch.

Multiple inheritance is allowed only when ordinary CPython accepts the physical
layout and each participating strict contract remains valid. Method dispatch
also requires a common verified family, interface table, or adjustment plan;
identical method spellings do not establish common slot numbers. Unsupported
dynamic mixins automatically use generic behavior. Runtime-enforced
`typing.final` rejects prohibited subclassing or overriding during actual type
construction, including from ordinary Python, before publishing a finality
fact.

That policy applies only to participating classes. Current source construction
declines class-level decorators other than the registered dataclass adapter,
including class-level `typing.final`; such a class remains dynamic and its
`__final__` marker is advisory, with no native finality authority. An undecorated
participating class can still have `@final` methods: its actual installed
final-name barrier rejects prohibited overrides. The annotation alone never
substitutes for either native contract.

### Descriptors and hooks

Fixed storage does not replace Python's descriptor protocol. Properties, data
descriptors, `__get__`, `__set__`, `__delete__`, `__getattribute__`,
`__getattr__`, `__setattr__`, and `__delattr__` retain their documented Python
behavior. Unsupported hooks or descriptor dependencies decline the affected
capability automatically before construction; no implicit rewrite can turn
their effects into raw field accesses.

A direct indexed-field or native-slot load requires proof that the selected
field is the real descriptor result, every relevant MRO owner is immutable,
and no applicable custom lookup hook changes its behavior. A missing field
must still invoke any
applicable `__getattr__` or produce the correct `AttributeError`. User-defined
data descriptors are calls, not raw storage loads.

A class-data override field occupies the ordinary instance-dictionary position
in lookup precedence: after an applicable data descriptor, but before its
plain class default. Declared fields likewise precede inherited non-data
methods. Protected effective method and `ClassVar` names instead ignore
colliding dictionary entries; this is an explicit name-specific strict rule,
not a blanket change to descriptor precedence. Generic attribute get/set/delete,
`object.__getattribute__`, `object.__setattr__`, `PyObject_GenericGetAttr`,
`PyObject_GenericSetAttr`, both `_PyObject_GetMethod` variants, member-descriptor
setters, specialized bytecodes, and generated executor/tier equivalents must
agree. A Python-only `__getattribute__`/`__setattr__` hook cannot enforce it.

Disable or adapt incompatible warmed `LOAD_ATTR_INSTANCE_VALUE`,
`LOAD_ATTR_WITH_HINT`, `LOAD_ATTR_SLOT`, corresponding `STORE_ATTR` operations,
and specialized method/call paths. Both specialization selection and already
cached operations must respect installed policies. Checked dictionary fields
also need owner-aware item/update/setdefault/pop/clear/replacement and supported
C-API barriers; a raw write followed by an advisory watcher is insufficient.

Standard `cached_property` on a dictionary-bearing class keeps its normal
descriptor miss, visible dictionary cache hit, explicit assignment, deletion,
and recomputation behavior. Classify it as a cached descriptor, not a protected
method or a field whose mere annotation proves a getter can be bypassed.

### Dataclasses and framework classes

Class namespace preparation and decorator execution run before final class
admission, behind the Pending instance barrier for participating fresh types.
Recognize the actual standard
`dataclasses.dataclass` implementation and options, not a local spelling or
copied attributes. Ordinary `@dataclass` retains its real instance dictionary;
no fixed prefix or implicit `slots=True` is required.
Preserve inherited fields, `ClassVar`, `InitVar`, keyword-only fields, defaults,
default factories, `__post_init__`, ordering/hash options, descriptor-typed
fields, weakrefs, and cached properties.

`@dataclass(slots=True)` creates a distinct replacement class. Its trusted
adapter installs a linked replacement-specific Pending barrier in that
replacement's actual type allocation. The original retains its observed
storage, while the selected final type binds its own requirements and receives
the permanent type contract. Preserve both callback sequences, escaped
provisional type identities, final class identity, and `__class__` closure-cell
repair; neither Pending type may have instances. An unselected provisional
type may become dynamic only without an installed permanent type contract.
Independent function and inherited constraints are never revoked.
A slotted replacement may inherit a real dictionary from its base; inspect
actual storage rather than assuming it is dictless. `frozen=True` preserves its
generated assignment rejection and generated initialization; it neither
freezes class metadata nor authorizes bypassing its setters with raw writes.

Authenticate generated functions, including those created with `exec`, against
their actual owning transformation, code, globals, defaults, and closure
environment before adopting them. Do not freeze shared standard-library
functions merely because a strict dataclass references them.

Nonparticipating metaclasses, unknown decorators, Pydantic models/dataclasses,
Django models, SQLAlchemy declarative/decorator/imperative mappings, and other
framework-managed classes automatically remain dynamic unless a verified
adapter enforces their complete construction and validation contract. Exact
`type(cls) is type` is not sufficient evidence: framework instrumentation may
occur through decorators or later registration. Preserve framework validation,
coercion, descriptors, dictionary replacement, and deferred rebuilding on the
dynamic path. Once strict restrictions are installed, later incompatible
mapping must fail rather than revoke them.

## Frozen classes and functions

### Classes

After initialization and final decorator adoption, a class granted frozen-class
capability has a frozen class dictionary and the verified stable bases/MRO and
metaclass behavior on which its plans depend. Its storage capability remains
independent: it may have a real indexed dictionary, source-requested native
slots, or generic storage. Attempts to replace
methods or class defaults, add class attributes, alter `__bases__`, change
slot descriptors or shadow eligibility, or replace dispatch-relevant dunder
methods raise `StrictMutationError`. Instance values remain mutable under their
field and descriptor policies unless the source requests frozen-instance
behavior. Supported annotated field checks follow the declaring class's
resolved `checked_attr` rule, independently of module `strict_assign` and any
future fixed-layout capability.

This requires an actual type-level and dictionary-level enforcement boundary,
not only a Python mapping proxy or `Py_TPFLAGS_IMMUTABLETYPE`. Cover
`type.__setattr__`, metaclass setters, base/MRO updates, the exact type
dictionary returned by `PyType_GetDict`, direct `tp_dict` mutation through
supported APIs, mutable slot/descriptor objects, and relevant inherited owner
dictionaries. A stock ancestor that can replace a relied-upon descriptor stays
dynamic even if the immediate strict class is frozen.

An exact `# soac: class(checked_attr=false)` rule declines a new local class
contract before construction. It does not install a separate mutable fixed-layout
capability or make inherited storage extensible. Ordinary method/default and
base changes remain subject to CPython and any inherited or already-installed
name, field, dictionary, and layout restrictions. Mutable methods/defaults never
supply frozen dispatch facts. Unrestricted descriptor/MRO changes require
ordinary generic behavior with no conflicting installed contract, selected
before construction; dictionary presence alone is neither an escape hatch nor
an optimization veto.

The freeze occurs after decorators and legitimate class-construction
configuration, at final admission before instances become possible. Code may
rebind defaults or populate declared class-only constants before that boundary,
not merely because the owner module has yet to seal.
Adding class data cannot retroactively extend or reclassify a published fixed
field schema. It uses the preselected generic/overflow policy or is rejected
when incompatible with installed restrictions.
Rebinding an existing class-data default before sealing does not discard an
existing instance override or change that field's published eligibility.
Freeze the final annotation-provider binding and reserve private
annotation-cache storage before publishing the frozen class; neither step
evaluates annotation expressions, invokes a descriptor, or inserts a new
observable class-dictionary cache key. Preserve any annotation cache already
materialized by ordinary initialization. Objects reached through class
attributes can still mutate; freezing the attribute binding does not
recursively freeze their contents.

### Functions

Class freezing alone is insufficient. This remains observable in ordinary
Python:

```python
FrozenClass.method.__code__ = replacement.__code__
FrozenClass.method.__defaults__ = (different_default,)
```

A strict-dispatch function therefore also protects its code object binding,
defaults/kwdefaults bindings, user-controlled annotation-provider bindings,
closure tuple, signature-relevant metadata, and descriptor-relevant behavior.
Cover both Python attribute setters and supported native
entrypoints such as `PyFunction_SetDefaults`, `PyFunction_SetKwDefaults`,
closure setters, and SOAC's own direct raw-function writes. Function watchers
cannot enforce this policy: pinned CPython treats their callback exceptions as
unraisable notifications rather than vetoes. Its public Python function identity
and normal call protocol remain available. The function's existing private
annotation-cache field remains lazy if it has not already been materialized;
freezing its provider never requires evaluating the provider or discarding an
existing annotation result.

Function annotations do not install runtime argument/return predicates or an
early code-freeze barrier to protect those predicates. Authenticated construction
and source-authorized execution still validate the actual function and code
owner; selected method code and metadata seals remain independently required.
Neither annotations nor completed calls supply runtime argument/result proofs.
The read-only `checked_native` entry diagnostic names the actual native call
entry; it does not assert parameter/return checking or grant an optimization
capability.

Unsupported framework methods may retain ordinary code-replacement behavior
only where no independent source-ownership or frozen-method requirement forbids
it. Classification uses exact source ownership, including intervening nested
scopes, not an unrelated function's later namespace assignment. Dynamic fallback
and late admission failure cannot revoke an already installed protection.

`__kwdefaults__` requires protection of its **dictionary contents**, not just
its pointer:

```python
function.__kwdefaults__["limit"] = 10
```

changes omitted-argument behavior without rebinding `function.__kwdefaults__`.
Use a protected exact kwdefaults dict or decline every default-dependent direct
call. Mutable objects stored as keyword-default values remain mutable; only the
default-name-to-object bindings are frozen.

The native read-only policy freezes the actual exact dictionary in place. It
preserves arbitrary existing keys, insertion order, key identity, and CPython's
normal hash/equality callbacks during binding; it does not normalize keys to
strings or convert this dictionary into indexed instance storage. Freezing its
entries does not make key equality immutable, so it supplies no stable lookup
or `NoLookupAliases` proof. A dictionary shared with an ordinary instance or
module does not make that owner strict: replacing the owner's dictionary or
ordinary instance class remains permitted while the old dictionary stays
read-only. A terminalized mapping prevents a new sealed-function invocation;
an already active frame continues with its captured owner and bound values.

The public `PyFunction_SetVectorcall` API may change the implementation pointer
without changing Python semantics: its documented contract requires the new
entry to preserve the behavior of the original callable. SOAC itself already
installs and restores vectorcall pointers through this API. Strict semantic
facts therefore depend on frozen code/defaults and a verified private direct
body, not on an immutable public vectorcall pointer. Use a stable dispatch
trampoline or treat compatible pointer changes as ordinary implementation
state; neither operation changes the module's `SEALED` state.

Freezing a defaults tuple does not freeze mutable objects inside the tuple.
Freezing a closure's cell identity does not freeze its current contents:
`nonlocal` mutation and normal closure state remain dynamic unless separately
proven immutable.

Still-pending eligible owned functions allocated while a module initializes
become frozen when the module seals, including tracked strict functions that
escaped their global namespace. Adopted methods and providers may already
have frozen at their earlier class/function admission; module sealing never
reopens them. Imported/shared foreign functions are not frozen merely
because a strict binding references them. Eligible strict nested functions
created after sealing become frozen after their
decorators have run and before any trusted strict ABI or callable fact is
published. Reserve each such function's private annotation-cache slot
before its individual freeze; annotation expressions remain lazy even when
the function is created after its owner module has sealed. Decorators,
function watchers, metaclasses, `__set_name__`, and `__init_subclass__` can
observe or retain the function identity earlier; those observations are not
forbidden or assumed impossible. An escaped function cannot acquire a trusted
target until its required barriers are installed. With `strict_assign=false`,
ordinary free functions retain mutable metadata; this does not unseal methods
or providers protected by selected class contracts. There is no per-function
mutable decorator in the source-comment interface.

### Frozen providers and reserved lazy annotation caches

Pinned CPython evaluates annotations on demand and materializes their metadata
only when it is first observed:

- a module does not cache computed `__annotations__` while it is initializing;
  its `__annotate__` getter may materialize its own binding when first read;
- a class can cache `__annotate_func__` and `__annotations_cache__` in the
  dictionary returned by `PyType_GetDict`, including a `PyType_Modified` call;
  and
- a function can replace lazy annotation state with a materialized annotation
  dictionary after invoking its annotation provider.

Before freezing a strict class or function, freeze its final annotation-provider
binding and reserve private cache storage owned by that exact object. Reserving
storage must not invoke the provider or a descriptor, evaluate an annotation
expression, or expose a placeholder in the Python-visible class dictionary.
An unread annotation cache remains empty; an annotation result already
materialized during initialization remains unchanged. A frozen provider and an
empty private cache remain compatible: the provider is immutable, but its
cached result has not yet been computed.

For a class, freeze the raw owner-dictionary provider binding without resolving
it through `getattr`. The first actual annotation read still performs normal
metaclass and descriptor lookup; a descriptor may bind or produce its callable
at that time, just as in ordinary CPython. If a custom metaclass or mutable
descriptor makes that behavior incompatible with another strict class
guarantee, reject that class or select its documented dynamic escape hatch.

When an ordinary `__annotations__` read first requires evaluated values,
invoke the original provider with its normal owner, globals, builtins,
closure, and `VALUE` format. After it returns successfully, an owner- and
cache-slot-scoped interpreter operation publishes the result into that
object's reserved slot. A provider exception leaves a previously empty cache
empty so a later read can retry after a forward reference or mutable global
becomes available; it never discards a result already published by a nested
read. Preserve normal dictionary identity and repeated-read caching semantics.

If recursive or concurrent class/function annotation reads each finish a
legitimate native getter invocation, authorize only those exact-owner cache
publications required to reproduce CPython's completion behavior. This narrow
authority does not permit public cache replacement or arbitrary class/function
mutation. Module globals remain append-once: if nested module annotation
evaluation attempts to replace an already-published final `__annotations__`
binding, reject that second publication with `StrictMutationError` rather than
weakening the module's immutable-binding policy.

Freezing the provider and its cached-dictionary binding does not freeze the
dictionary's contents. Ordinary writes such as
`function.__annotations__["argument"] = replacement` remain legal, and
subsequent `VALUE`/`FORWARDREF` reads observe those writes exactly as CPython
does. The same rule applies to class and module annotation dictionaries;
`STRING` requests continue using the preserved original provider where normal
CPython does so.

For classes, reserve private storage for both `__annotate_func__` and
`__annotations_cache__` when the corresponding pinned-CPython getter may need
to materialize them. An annotated class can already have a compiler-installed
`__annotate_func__` provider in `Class.__dict__`; preserve its existing
visibility and freeze that binding. If an unannotated class would first create
`__annotate_func__ = None` through its getter, retain the normal first-read
timing. An unmaterialized `__annotations_cache__` remains absent from
`Class.__dict__` and `PyType_GetDict` until its ordinary first value read. Use
hidden empty slots or an explicit sidecar plus first-read projection into the
real class dictionary; a newly visible sentinel inserted while sealing is
invalid. Retain the normal `PyType_Modified` side effect when CPython would
perform it without treating annotation-only cache publication as a changed
method, slot, or strict module contract.

Functions can reuse their existing private `func_annotations` cache field. A
first successful annotation read may populate that reserved field or
materialize an existing CPython tuple representation into its normal
dictionary. Public assignment to the frozen `__annotate__` provider or
replacement of protected function metadata remains forbidden. Preserve the
original provider after caching so `annotationlib` can still support `VALUE`,
`FORWARDREF`, and `STRING`, including unresolved references and its normal
owner/closure rules. Direct `FORWARDREF` and `STRING` requests retain their
ordinary behavior; their temporary proxies or source strings must not replace
the cached `VALUE` dictionary. Their synthetic-globals provider replay uses
the narrowly authenticated dynamic execution boundary described above, never
an arbitrary exception to strict code ownership. Strict functions and classes
created after module sealing reserve their own private cache slots before
their individual contracts are published.

A module's first lazy `__annotations__` or `__annotate__` insertion is already
allowed by the append-once global rule and needs no private reservation or
special publication path. In particular, do not force or cache module
annotations while CPython still considers that module to be initializing.

Internal cache-publication authority applies only to the expected reserved
slot of its authenticated owner, after user-provided annotation code has
returned. Code executed by the provider retains its ordinary permissions to
append a new global or update an explicitly mutable one, but never inherits
authority to replace a final export, rewrite the provider, modify other
protected class/function metadata, or change dispatch-relevant methods and
slots. Invoking a provider can execute arbitrary Python, so it remains a
reentrant boundary requiring validation before trusted native execution
resumes.

Never eagerly force annotation evaluation merely to freeze its owner. Preserve
first-read timing, provider side effects and exceptions, unresolved forward
references, `from __future__ import annotations` string behavior, annotation
dictionary content semantics, and the externally visible CPython class/module
metadata. Annotation-cache publication does not unseal or deoptimize its
owner; strict dispatch must tolerate any required metadata-only type-version
change without silently retaining a stale direct target.

## Explicit escape hatches

Automatic classification and package defaults are the normal path. Unsupported
classes require no escape-hatch annotations. An explicit source rule can decline
a new local class contract before construction:

```python
# soac: module(strict_assign=true, checked_attr=true)

# soac: class(checked_attr=false)
class PluginRegistry:
    pass
```

The supported source controls and ordinary language declarations are:

| Construct | Effect | Lost optimization facts |
| --- | --- | --- |
| `global NAME` | Declares one mutable module binding. | Constant global identity for that binding. |
| Source `__slots__` or stdlib `dataclass(slots=True)` | Requests Python's real native-slot behavior, including any inherited or explicit dictionary. | No capability follows from the spelling alone; construction validates actual storage. |
| `# soac: module(strict_assign=false)` | Keeps this module's global bindings mutable without disabling selected class contracts. | Final global binding identity. |
| `# soac: module(checked_attr=false)` | Declines new local class contracts unless an exact class rule opts back in. | New local class/field capabilities; inherited checks remain. |
| `# soac: class(checked_attr=false)` | Declines a contract for this exact class before construction. | New local class/field capabilities; inherited checks remain. |
| `typing.final` under the selected strict policy | Runtime-enforces no subclassing or method override for participating classes at actual type construction. | None once enforced; the annotation alone is not a finality proof. |

The previously proposed `strict.dynamic`, `dynamic_fields`, `mutable_type` and
`mutable_function` decorator APIs are not part of this comment-only interface.
Rules never mint capabilities or bypass mandatory installed restrictions.

No generic `allow_unsafe`, silent compatibility mode, test-only patch bypass,
or global process-wide disablement is part of the initial contract. Tests that
require monkeypatching must select an explicitly ordinary or mutable language
configuration before the module is initialized. They cannot patch an already
sealed strict module or downgrade its guarantees.

## Builtins and annotations

### Builtins

With `strict_assign=true`, a module freezes the binding of its captured
`__builtins__` mapping, but the shared process builtin namespace remains
ordinary and mutable. Replacing
`builtins.len` in stock Python must therefore remain visible to strict code
unless that strict module explicitly requests a separate frozen snapshot.

Append-only globals also preserve ordinary builtin shadowing:

```python
# soac: module(strict_assign=true, checked_attr=true)


def size(value):
    return len(value)
```

After sealing, `module.len = lambda value: 100` is a legal first binding, and
`module.size([])` must then return `100`. Strict lowering must allocate a
global slot for source-referenced names even when their value is initially
absent. The local implementation disables the source-builtin constant rewrite
for strict modules and retains these indexed global loads.
The strict fast path must check that stable slot on every builtin-fallback
lookup, or use a correctly invalidated per-name absence guard; it must never
snapshot the builtin while assuming the module can never define the name.

The same restriction applies to cached `NameError`, missing attributes,
module-level `__getattr__`, export sets, `dir`, and wildcard imports: a sealed
append-only module is not a closed namespace. A frozen per-module builtin
snapshot stabilizes builtin objects but still does not prevent a newly appended
module global from shadowing them.

A later, separately registered future feature may request such a snapshot:

```python
from __future__ import strict, strict_frozen_builtins
```

The additional feature requires `strict` and must be recognized before creating
any module functions. The loader installs a protected per-module exact builtin
dictionary and ensures every strict function captures that same dictionary.
The snapshot is a documented semantic difference: later mutations to the
process-wide builtins do not affect that module. An interpreter without this
optional feature rejects it through normal future-feature compilation; no
option may freeze or mutate the builtins observed by ordinary modules.

### Annotations and primitives

The source rules above select `checked_attr=true` independently of module
binding freezing. The same normalized policy and type contracts govern offline
diagnostics, artifact identity and runtime construction/storage. A field index,
annotation or module membership alone does not authorize a check. There is no
separate checked-fields configuration, nor a retained disabled call-policy
alias. No selection depends on profiles, warmup, optimization level or inlining.

No function-level runtime type checks apply, on any backend or generated
constructor. Bind all arguments normally, including positional-only/keyword-only
arguments, defaults, `*args`, `**kwargs`, and descriptor-bound receivers. Missing,
duplicate and unexpected arguments retain their existing errors and priority.
Annotation-mismatched values may enter and leave a body normally. A bad selected
field assignment fails at its actual write, after any preceding source effects.
Dataclass factories and `InitVar` arguments have no independent call check.

Supported normalized checks use genuine nominal builtin/class membership,
`None`, fully supported unions, and `Optional`. Nominal `int` accepts `bool`
and genuine integer subclasses. A `float` position accepts genuine `float` and
`int` instances, including `bool`, without coercion or a promise of exact float
storage. Ordinary genuine subclasses of a checked class are accepted but need
their own actual strict receiver capability for indexed fields or virtual
dispatch. Exact unboxing always requires a separate exact-type proof.

`Any`, `Unknown`, unresolved references/imports, generic containers such as
`list[int]`, protocols, virtual-subclass membership, custom metaclass
`__instancecheck__`, and unsupported annotation expressions remain dynamic for
the affected value. Do not drop an unsupported union member and check the
remaining narrower union. Generators, coroutines and async generators likewise
keep ordinary call and result behavior; do not check their immediate objects or
eventual results against annotations or change execution/exception timing.

Forward references, lazy annotation providers, type aliases, and type-parameter
bounds/defaults retain their normal evaluation behavior. Runtime enforcement
uses authenticated offline normalized facts and actual target identities;
sealing never evaluates annotation expressions just to recover those facts.
An unchecked or inferred annotation is not a trusted native value proof.

When checked fields are selected, enforce each supported write through Python
attributes, `object.__setattr__`, member descriptors, direct/whole dictionary
mutation, deserialization, generated constructors, warmed bytecodes, supported
C APIs, and SOAC indexed stores. Unsupported descriptor/framework validation
stays dynamic unless a verified adapter preserves its original validation and
coercion order. A missing mutation barrier prevents publishing a checked-field
fact; it cannot be compensated by an annotation or a non-rejecting watcher.

Check elimination remains deferred. Retained guarded execution needs an
independent runtime guard or protected-storage premise; annotations and
successful calls are not checked-argument or checked-return proofs. Preserve
the distinction between nominal acceptance and participating receiver/layout
capability. Recheck after an untrusted callback or other operation that can
invalidate the relevant premise through `__class__`, mutable closure contents,
or unsupported writes. An override cannot inherit a trusted return fact from
a base method's annotation.

Ordinary Python `int` arithmetic always preserves arbitrary precision. Native
integer and float regions may remain unboxed internally only while they retain
normal exception, rounding, and big-integer fallback behavior. Any future
fixed-width primitive must have explicit syntax/API and documented checked
overflow; silent wraparound is not an implicit consequence of strict mode.

## Interoperability and dispatch

### Boundary matrix

| Caller | Callee or receiver | Required behavior |
| --- | --- | --- |
| Ordinary Python | Ordinary Python | Stock-compatible CPython execution; no separate SOAC optimization target. |
| Ordinary Python | Strict object | Public Python/vectorcall entry, normal argument binding and results, and selected strict storage/mutation enforcement. |
| Strict Python | Ordinary object | Ordinary dynamic CPython lookup/call, or a CPython-correct guarded boundary specialization located wholly in the strict caller; no strict facts are inferred from annotations or module names. |
| Strict Python | Sealed strict object | Validated direct ABI, fixed global/field load, stable function target, or strict vtable dispatch when all required contracts hold. |
| Strict Python | Stock subclass of a strict class | Dynamic fallback unless the actual subclass independently satisfies a verified strict receiver contract. |

An individual function can therefore expose both a public Python ABI and a
private strict ABI:

```text
ordinary caller
    -> Python/vectorcall entry
    -> ordinary argument binding
    -> strict function body

verified strict caller
    -> independently guard the actual callable/body and operands
    -> direct(fn_env, tstate, args...)
    -> same strict function body
```

Direct code still receives the callee's actual `FunctionEnv`. Callee globals,
captured builtins, closure cells, defaults, lifetime ownership, and exception
state must never be recovered from the caller's module or inferred solely from
a matching name.

### Globals and imports

A strict importer may treat an imported binding as stable only after validating
the producer module's identity, source/contract version, seal state, export
mutability, and relevant target contract. A strict module importing an ordinary
module does not make the ordinary module immutable.

`from ordinary import value` still snapshots the object reference according to
normal Python import semantics. The local strict binding may be final, but
operations on the imported object remain dynamic unless its own object-level
contract proves otherwise.

### Fields and methods

A verified strict receiver with a sealed storage capability can use its
stable dictionary-field index or actual native member offset. Inherited fields
retain the corresponding base position; the fast path does not infer a layout
from ordinary split-dictionary observations or checker field ordering.

A class-data override access uses its construction-installed field index. A load
returns the populated instance value or, when that value is `UNSET`, the
frozen default selected from the actual receiver class and MRO. A store
replaces the instance value without a dictionary lookup; deletion clears it
and preserves normal absent-override errors. These are explicit typed
operations with normal ownership, finalizer, descriptor, and exception
semantics, not generic descriptor getter/setter calls.

A final strict class/method can dispatch directly to a frozen target only with
runtime-enforced finality or an exact actual receiver proof, verified binding,
protected lookup, and the callee's authenticated ABI/environment. An open
participating hierarchy uses separately assigned method-family slots; each
compatible override installs its own frozen target and environment at the same
slot. Dictionary-bearing receivers can participate when their name policies
are enforced. Ordinary subclasses, mutable targets, dynamic descriptors, and
unsupported hooks take the generic path. A callable field or plain class-data
default can still be shadowed, so its call loads the field/default and applies
ordinary callable binding rather than assuming a method target.

Method calls must preserve evaluation order, descriptor effects, argument
binding, visible keyword shape, exceptions, ownership/finalizers under the
approved lifetime policy, and overrides. Stable class/function facts remove
lookup work; they do not justify observable argument rewrites or calling the
wrong override.
Resolve and capture the callable and receiver before evaluating argument
expressions; moving virtual target selection past an effectful argument can
change which target runs. Static return annotations, including inherited method
annotations, supply no runtime result guarantee. Any retained result fact needs
an independent executed guard or protected-storage premise, not a call check.

### Object compatibility

Strict objects remain normal Python objects. Supported operations include
`import`, `from ... import`, `getattr`, `setattr` on valid mutable fields,
`isinstance`, `issubclass`, `inspect.signature`, descriptors, iteration,
context managers, source-requested weak-reference behavior, garbage collection,
ordinary exception handling, and passing values through containers or
extensions within the documented supported native boundary.

Automatic classification preserves ordinary dictionary presence, `vars`,
dynamic overflow, explicit slots, and dataclass options. The intentional
differences are capability-specific: protected method/`ClassVar` attribute
writes reject while colliding dictionary entries do not shadow lookup;
selected field writes reject invalid stored values; frozen
module/class/function bindings reject forbidden mutation; and strict physical
fields may use the approved destruction-order difference. None applies merely
because an ordinary object is referenced from strict code. No difference
authorizes skipped descriptors, finalizers, validation, or callbacks.

### Reload and replacement

`importlib.reload(strict_module)` is rejected with `StrictReloadError` in the
first implementation because reload would mutate final bindings in an existing
module object and invalidate live direct callers.

Removing an entry from `sys.modules` and importing again may create a distinct
strict module instance when that module is top-level or its parent binding is
ordinary/explicitly mutable. It cannot replace an already published final child
of a sealed strict package. An optional existing-binding preflight can reject
the collision before executing replacement code; without it, ordinary CPython
may already have executed and cached the replacement child when its parent
publication fails. Existing references still point to the old sealed instance,
as normal Python object references do. Every strict dependency fact is tied to
the actual module/object identity; matching module names alone do not
authorize reusing a direct target or layout.

## Compiler representation

The parser, lowering pipeline, resolved module IR, optimizer, runtime, and
codegen must share one explicit semantic contract:

```rust
enum ModuleLanguage {
    Python,
    Strict(StrictModuleContract),
}

struct StrictModuleContract {
    version: u32,
    mutable_globals: Vec<GlobalBindingId>,
    append_only_globals: bool,
    checked_values: CheckedValuePolicy,
    classes: Vec<ClassConstructionContract>,
    functions: Vec<StrictFunctionContract>,
    frozen_builtins: bool,
}

struct ClassConstructionContract {
    plan: StrictClassPlanId,
    source: SourceIdentity,
    fields: Vec<StrictFieldContract>,
    shadow_fields: Vec<StrictShadowFieldContract>,
    instance_storage: InstanceStoragePolicy,
    dictionary_replacement: DictReplacementPolicy,
    name_policies: Vec<NameResolutionPolicy>,
    class_mutation: ClassMutationPolicy,
    method_layout: Vec<MethodLayoutEntry>,
    finality: FinalityPolicy,
    participating_adapter: Option<AdapterIdentity>,
}

struct StrictFunctionContract {
    function_id: FunctionId,
    mutation: FunctionMutationPolicy,
}
```

These types are proposed shapes, not existing public APIs. Their final owner
should be `BlockPyModule`, resolved typed IR, or a sidecar validated against
them. Raw AST names still lower first to `UnresolvedName`; strictness does not
bypass semantic name binding or invent storage locations during parsing.

Runtime state separately records concrete sealed module instances, protected
dictionary policy, the stable name-to-index map and indexed value state, live
class/function identities, their frozen annotation-provider identities and
private reserved annotation-cache slots, per-execution construction handles,
stable dictionary-field indexes/native offsets, separate dispatch families and
method tables, selected field-write predicates, and dependency fingerprints.
Contract requests are not runtime capabilities: only the final actual constructed
objects can publish the latter after validation and enforcement.
`SharedModuleState` remains static/lowered module metadata; actual globals stay
in module runtime state and `FunctionEnv`, not a new borrowed global pointer
hidden in static module state.

The optimizer consumes explicit semantic facts:

```rust
enum StrictSemanticFact {
    SealedFinalGlobal {
        module: ModuleInstanceId,
        binding: GlobalBindingId,
    },
    FixedInstanceField {
        class: StrictClassId,
        field: StrictFieldId,
        storage: VerifiedFieldStorage,
    },
    FixedClassDataShadow {
        class: StrictClassId,
        field: StrictFieldId,
        storage: VerifiedFieldStorage,
        default: StrictShadowDefaultSource,
    },
    FrozenFunctionTarget {
        function: FunctionInstanceId,
        environment: FunctionEnvironmentId,
    },
    FrozenMethodSlot {
        class: StrictClassId,
        family: StrictDispatchFamilyId,
        method: MethodSlotId,
    },
    NominalTypeAccepted {
        value: ValueId,
        checked_type: CheckedTypeId,
    },
    VerifiedReceiverCapability {
        value: ValueId,
        class: StrictClassId,
        family: Option<StrictDispatchFamilyId>,
        layout: Option<StrictLayoutId>,
    },
    FrozenBuiltins {
        module: ModuleInstanceId,
    },
}

enum VerifiedFieldStorage {
    IndexedDictionary { layout: StrictLayoutId, index: u32 },
    NativeObjectSlot { layout: StrictLayoutId, offset: u32 },
}

enum StrictShadowDefaultSource {
    ExactReceiverOwner(ClassId),
    ReceiverClassLookup,
}
```

A fact exists only after its associated runtime enforcement has succeeded. The
v3 planner selects and validates typed global, field, direct-call, or dispatch
plans against those facts. Codegen mechanically emits the selected operation;
it must not infer strictness from source spelling, profile observations,
single-assignment analysis, or a Python object's `__module__` attribute.

An inherited physical field index or offset does not prove a single class
default: strict subclasses can override their frozen class values while
retaining that same field position. A shadow-load plan must either guard the actual exact
receiver/default owner or load the frozen default from the actual receiver's
class/MRO table. A base-class layout fact alone never authorizes substituting
the base default for an unpopulated subclass instance.

A present final global is a permanent positive fact; a declared mutable global
is dynamic; an absent global is **not** a permanent negative fact. Appending a
different name never invalidates an already-final global's value, stable
index, direct-call target, or strict module contract, even when the
name-to-index hash table rehashes. First binding a previously absent name can
invalidate only assumptions that depended on that particular name's absence or
on the namespace's complete key set. Such assumptions need a live slot check
or explicit per-name generation; dictionary watchers and coarse versions are
insufficient when raw indexed stores bypass them.

Profile data still estimates frequency and profitability. It never proves that
a module sealed, a function froze, a class installed a storage/dispatch
capability, a value passed a mandatory check, or a cross-module dependency
remains current. Check elimination additionally validates proof dominance and
invalidation in the current typed IR.

## Cache and dependency invalidation

Cached BlockPy, typed optimization decisions, profile/application artifacts,
and precompiled native code must distinguish at least:

- ordinary versus strict language mode;
- strict contract schema/compiler version and pinned CPython ABI/build
  identity;
- source hash, recognized future features, `CO_FUTURE_STRICT`, and normalized
  optional language extensions;
- offline artifact schema/exporter/checker revision, conservative analysis
  settings, Python version, platform, search paths, stubs, dependency versions,
  ignored-error state, and resolved per-file project checked-value policy;
- mutable-global declarations, append-only storage schema, stable source-global
  binding identities, and any explicitly consumed negative-lookup facts;
- strict class layout, base, method-slot, class-data shadow/default-owner, and
  function-signature fingerprints;
- frozen-builtin policy; and
- every external strict dependency whose contract, type, field offset, or
  callable identity was actually consumed.

Changing a dependency from ordinary to strict, strict to ordinary, mutable to
final, one field layout to another, or one callable signature to another must
invalidate affected plans. Validation must not import or execute dependency
modules solely to inspect cache metadata, particularly during circular imports.

CinderX's [dependency invalidation design][cinder-dependencies] explains why
source-only `.pyc` invalidation is insufficient for cross-module field and
callable assumptions. The patched interpreter must use a distinct `.pyc`
cache tag **and** incompatible bytecode magic, or an equivalently enforced
bytecode rejection boundary, including for copied or sourceless `.pyc` files.
A shared stock cache tag/magic is unsafe: stock CPython can load a cached strict
`co_flags` value without reparsing the source or rejecting its unknown future
feature. SOAC should retain explicit per-dependency fingerprints rather than
accidentally reusing same-source profiles or typed plans compiled under a
different strict contract.

## Diagnostics

Suggested exception families:

```python
class StrictSyntaxError(SyntaxError): ...
class StrictRuntimeUnavailableError(ImportError): ...
class StrictMutationError(TypeError): ...
class StrictLayoutError(TypeError): ...
class StrictReloadError(ImportError): ...
```

Examples:

```text
StrictMutationError: cannot rebind final global example.LIMIT;
declare `global LIMIT` in the strict module to permit mutation

SOAC capability declined: example.Child has an unverified inherited field
prefix from legacy.Base; retaining ordinary storage and lookup

StrictLayoutError: a transformation replaces the installed field policy for
example.Widget.value with an incompatible descriptor after publication

TypeError: example.Widget.value requires int; got str

StrictRuntimeUnavailableError: example opted into strict semantics,
but its module globals are not protected by the active SOAC runtime
```

Every diagnostic should identify the module, source location when available,
violated contract, and precise supported escape hatch. Excluded SOAC frame
and observer behavior does not require detection, refusal or fallback
machinery. Independently required source authentication, ownership safety and
installed contracts still fail explicitly when violated; ordinary CPython
frames and observers remain unchanged.

## Required implementation order

The current phases are the interpreter-enforcement phases in
[`TYPE_DRIVEN_OPTIMIZATION.md`](TYPE_DRIVEN_OPTIMIZATION.md), under the dated
scope and Pending-type amendments in [`OPT_GOAL.md`](../OPT_GOAL.md). Complete
authenticated offline facts, actual runtime binding, native enforcement and
compatibility with JIT execution disabled, then run `just test-all` against the
matched optimized interpreter. Retained optional execution paths must honor
installed contracts. Optimization and measurement require a separate request.

## Historical optimization-first roadmap (deferred)

The phases below preserve the earlier design, not current implementation
status or remaining prerequisites. Their pre-callback full-contract timing and
callback-created provisional instances are superseded by the Pending protocol
above. Claims about the then-current optimizer are historical. New indexed
layouts, dispatch, proof propagation, check elimination and benchmark phases
remain deferred even after interpreter enforcement completes. The 2026-08-25
(PDT) amendment additionally retires the earlier function parameter/return
checks and all dependent argument/return proofs; they are not future work
implicitly authorized by this historical roadmap.

### Phase 0: align policy and remove unsound assumptions

Keep this contract, `OPT_GOAL.md`, and `doc/TYPE_DRIVEN_OPTIMIZATION.md` aligned
on automatic capability classification, real instance dictionaries, actual
dataclass options, project-selected synchronous checks, protected-name lookup,
and the supported native boundary. The following phases describe required work,
not completed enforcement. No optimizer may rely on an older implicit-slots or
unchecked-annotation assumption while those capabilities are being built.

`doc/SPECIALIZATION.md` currently documents direct-call targets selected from
single-write module globals under a "strict-module assumption" while also
stating that external mutation is not runtime-enforced.

Before introducing the strict future feature, gate these static direct-call
decisions on a genuinely verified immutable compiler-owned target or a real
sealed strict-module contract. Non-opted-in modules execute through their
stock-compatible dynamic path and are not maintained as a separate optimized
SOAC mode.

SOAC's `UnsoundBuiltinRuntimeNameRewriter` snapshots names such as
`len` as runtime constants when they are not statically declared global. That
rewrite is incompatible with append-only strict modules because a later first
binding can shadow the builtin and ordinary Python can replace the function's
captured builtin mapping entry. It is disabled for strict code; existing
indexed-global loads check the module slot and use the function's live captured
builtin mapping on absence. Any future replacement needs both an explicit
indexed-global absence check **and** a live lookup/validated guard against that
actual mapping. An immutable
builtin constant is legal only with a verified optional frozen-builtin
contract.

The existing runtime-builtin scalar `Add`/`Sub`/`Mul` path also raises
`OverflowError` on signed-`i64` overflow instead of preserving Python's
arbitrary-precision result. Disable that path for strict code or replace its
overflow branch with the ordinary bigint fallback before admitting strict
optimization. Likewise, existing closed iterator fusion must not silently
inherit broader observer, generator/frame, finalizer, tracing, profiling,
monitoring, recursion, or cleanup differences. Preserve the selected
activation policy with the necessary guards or explicit unsupported-operation
errors, or keep the ordinary unfused implementation. Strict mode alone does
not approve a new overflow or observer-visible compatibility relaxation.

Add regression cases where ordinary external code replaces a function through
module attribute assignment, `module.__dict__`, `function.__globals__`,
`exec`, native C APIs, and SOAC's own indexed storage path. A watcher-only fix
is specifically insufficient. Enforce the selected narrowed native boundary,
which excludes `void PyDict_Clear` against immutable authoritative dictionaries;
if arbitrary native mutation must be supported, authority needs a protected
projection or that strict configuration must be rejected. Treat
behavior-preserving `PyFunction_SetVectorcall` updates as implementation state,
not semantic mutations.

### Phase 1: offline facts and authenticated policy

Register the feature in vendored CPython's `__future__` module, future parser,
code-object flags, and compiler-feature mask. Preserve the feature in SOAC's
existing future-import rewrite, collect lexical `global` declarations, record
the policy in validated module IR, and carry matching flags through original
code compilation, transformed execution, cache identities, and isolated `.pyc`
artifacts. Compile separately imported SOAC modules with `dont_inherit=True`.
Assign stable indexes to source-referenced globals, including initially absent
names that can later shadow builtins. Authenticate strict execution before any
module body runs; define inherited dynamic-code behavior and fail closed when
the loader or runtime cannot enforce it. Implement the owner-bound
annotation-format replay exception without allowing arbitrary strict code to
run against foreign globals. Keep ordinary modules on their stock-compatible
CPython semantic path without optimizing them as a separate SOAC mode.

Add the deterministic offline `ty` exporter and matched Python 3.15
parser/semantic/typeshed support, including the strict future feature and
conservative narrowing configuration. Export versioned source-bound module
shards, class/function/member facts, finality, shared checked-value policy,
strict diagnostics, and dependency/environment fingerprints. Preserve uncertainty
from `Any`, `Unknown`, unresolved imports, and ignored errors; never execute
imports or lazy annotations merely to produce a fact. Artifact validation must
precede execution and must not load the checker into imports or native hot paths.

### Phase 2: explicit actual type construction

Carry the authenticated class plan through lowering into an immutable
compiler-owned construction intrinsic. Mint a per-execution handle bound to
the module instance, lexical class plan, namespace function, actual metaclass,
and transformation phase. Reject replay, cross-class/module transfer, callback
theft, concurrent reuse, and expired handles. The CPython allocator consumes
the explicit contract and installs the requested physical schema and name
policies before `PyType_Ready` or callbacks; no namespace attributes, temporary
Python metadata, or TLS may transport authority.

Preserve namespace preparation, resolved bases, class-cell propagation,
`__set_name__`, `__init_subclass__`, and decorator evaluation/application order.
Automatically choose dynamic behavior for unsupported classes before
irreversible publication. Validate the actual final decorated result and use
fresh linked handles for replacement allocations. Repeated factory executions
and classes created after module sealing have separate runtime identities and
their own finalization; no frozen target is published while initializing.

### Phase 3: enforce module, class, and callable boundaries

Implement the protected exact-dictionary policy in vendored CPython, route
every SOAC indexed store through it, cover both indexed and ordinary generator
globals dictionaries, define module seal/failure states, and freeze strict
function dispatch metadata and keyword-default mappings. Implement a
rehashable name-to-index map, stable append-only value indexes, atomic
first-binding behavior, mutable-name tombstones, and safe values-array growth.
Make prepared global caches independent of movable map/value allocations.
Allow normal late package-child publication, rejecting only collisions with an
existing final binding. Implement the required `SEALING` and terminal
`TEARING_DOWN` transitions, Python-visible rejection of forbidden dictionary
clear operations, the chosen narrowed native compatibility boundary,
frozen annotation providers, private annotation-cache reservation before each
object freezes, owner-scoped lazy cache publication, and safe reentrant
boundaries. Reject reload; a sealed module never transitions back to mutable
Python.

Protect actual class dictionaries, bases/MRO, member-descriptor stores,
function code/default/closure bindings, and in-place keyword-default contents.
Implement owner-aware instance dictionaries, whole-dictionary replacement,
protected-method/ClassVar lookup, and incompatible `__class__` rejection across
generic Python, native APIs, both method helpers, warmed specialized bytecodes,
and generated tier-specific equivalents. Retain supported descriptor and
cached-property behavior. Advisory watchers cannot replace a rejecting barrier.

### Phase 4: selected fields and ordinary calls (revised)

Implement opt-in checked-field writes at every supported storage boundary.
Remove all runtime function-level type checks and their proof consumers while
retaining ordinary binding, body/result semantics, lazy annotations, source
ownership and safe cleanup. Nominal field acceptance does not prove exact
representation, layout or finality. Check elimination and proof propagation
remain deferred; a future call-enforcement layer needs a separate specification.

### Phase 5: stable storage and verified dispatch plans

Complete stable indexed instance dictionaries with inherited fixed prefixes,
ordinary overflow/insertion order, actual replacement identity, and coherent
Python/C/GC behavior. Support native offsets only for the actual source-requested
slot layout. Keep class-default values unchanged and represent `UNSET` and
receiver-specific frozen-default fallback explicitly. Preserve exactly-once
field release and the approved destruction-order exception, source-requested
weakrefs, and ordinary subclass behavior. Keep `__static_attributes__` separate
from authenticated layout evidence.

Add structured v3 alternatives for final globals, indexed/native field accesses,
callable-field loads, frozen direct calls, method-family virtual dispatch, and
runtime-enforced final targets. Preserve the callee `FunctionEnv`, binding/ABI,
lookup-before-argument order, generic receiver fallbacks, negative/builtin-shadow
guards, checked-value proofs, and complete cache/dependency identity. Map
rehash, dictionary growth, and compatible implementation changes do not revoke
sealed capabilities.

### Phase 6: recognized transforms and compatibility

Implement the authenticated stdlib dataclass adapter for ordinary and actual
`slots=True` forms, linked replacement construction, callback-visible original
instances, generated functions, closure-cell repair, and inherited layouts.
Preserve all requested options and descriptors. Verify automatic dynamic
fallback for unsupported metaclasses and framework classes, including Pydantic,
Django, and SQLAlchemy. A future cooperative adapter may publish additional
capabilities only after it enforces the framework's real behavior.

### Phase 7: measurement and completion evidence

Run `just test-all` and the strict-versus-stock pyperformance protocol from
`OPT_GOAL.md`, comparing previous strict SOAC when available. Record actual
sealed module/class/function coverage, direct/virtual/indexed operation use,
generic fallback rates, unsupported framework frequency, typed-IR/native-code
size, and offline analysis cost separately from steady-state execution. Keep
one optimization-attempt record per strategy, including failed or inconclusive
outcomes. A completed benchmark without the required authenticated strict hot
path cannot establish optimization progress.

Frozen-builtin snapshots, checked container element storage, coroutine/generator
contracts, and new unboxed ABI semantics beyond this selected policy remain
separate extensions. Each needs its own approval, enforcement, failure
behavior, typed plans, and compatibility tests.

## Acceptance tests

The current interpreter-enforcement acceptance criteria are authoritative in
`TYPE_DRIVEN_OPTIMIZATION.md`. This broader matrix also covers optional retained
capabilities: fixed indexes, virtual/direct dispatch, profile/apply and check
elimination clauses apply only to capabilities actually installed or consumed,
not as requests to implement new optimizations or run benchmarks.

Focused behavior and structured-contract coverage includes:

1. Standard future-feature placement, aliases, combined `annotations` imports,
   unsupported-interpreter `SyntaxError`, missing-runtime/ordinary-loader
   fail-closed behavior, nested `co_flags`, dynamic `compile`/`exec`/`eval`
   inheritance, `dont_inherit=True`, forged flags, source-less/cross-build
   `.pyc` rejection, independently imported ordinary modules through both the
   CPython and SOAC loaders, circular imports, failed initialization, and
   initialization-time rebinding. Ordinary modules must not enter strict
   profiling, optimization-plan selection, or optimized JIT execution;
   ordinary dynamically compiled code sharing strict globals and
   synthetic-globals annotation replay also remain unoptimized.
2. First-binding append, immediate finality, final-name rebinding/deletion
   rejection, and `global`-declared mutable bindings through module attributes,
   `globals()`, `module.__dict__`, `function.__globals__`, `exec`, interpreted
   global stores, every dictionary mutator, CPython C APIs, and direct SOAC
   indexed stores.
3. Name-index-map growth and rehash; stable existing `GlobalBindingId` values;
   safe value-array reallocation; prepared-cache pointer independence;
   mutable-name deletion/reinsertion at the original index; ordinary dictionary
   insertion order and views; supported non-string keys or explicit rejection;
   concurrent first-writer races; atomic rejection of mixed allowed/forbidden
   bulk updates and duplicate first writes; unsupported/reentrant source
   mappings; protected ordinary-dict generator modules; reload and
   `dict.clear()` rejection; documented exclusion or isolation of native
   `PyDict_Clear`; and safe module/dictionary GC and interpreter teardown.
4. Late source, bytecode, native-extension, archive, namespace, nested-package,
   newly created, and custom-finder child imports without pre-enumeration or
   `global child`; in-place package-search-path changes; existing-final export
   collisions; removal/reimport of an already-final child; and explicit
   mutable-global opt-outs for intentional replacement.
5. Later module globals shadowing `len` and other builtins; replacing,
   deleting, and restoring entries in the actual captured builtin mapping;
   absent names becoming defined; guarded/frozen builtin snapshots;
   missing-attribute hooks; wildcard-import/export visibility; and per-name
   negative-cache invalidation even when a raw indexed store does not update a
   dictionary version.
6. Offline field classification for renamed receivers, nested captures,
   declared and uncertain fields, inherited constructors, properties, genuine
   source-requested slot conflicts, lazy annotations, private names, and
   unchanged CPython `__static_attributes__` behavior. Cover annotated and
   unannotated class-data defaults; unchanged class-dictionary entries; absent
   reserved keys and ordinary visible dictionary order; independent
   instance override, replacement, deletion, and delete-while-unset errors;
   constructor/method and ordinary/strict/C-API external writes; inherited
   receiver-class defaults and unchanged prefix indexes/native offsets;
   stock-base mutation and descriptor transitions; callable plain-data overrides;
   correct data-descriptor precedence; genuine declared fields before inherited
   non-data methods; rejected protected `ClassVar`/method attribute writes but
   permitted ignored dictionary collisions, including generic native lookups,
   both method helpers, and warmed specialized bytecodes;
   side-effect-free direct, qualified, imported-alias, future-stringized,
   literal-string, `TYPE_CHECKING`-only, and genuinely ambiguous `ClassVar`
   classification; accepted unrelated unresolved forward references;
   automatic fallback for ambiguous classification; module-initialization
   rebinding, generic/overflow class-data additions when the installed policy
   permits them, rejected schema changes and descriptor transitions, and
   actual source-requested storage and Pending allocation/`__class__` rejection
   during `__set_name__`/`__init_subclass__` callbacks, followed by selected
   constraints on the actual final decorated type before instance admission;
   unchecked ordinary value assignments;
   GC/refcount/finalizer behavior; explicitly
   different physical-layout-versus-stock destruction order across public,
   private, and inherited fields, including ordinary/dynamic subclasses that
   physically inherit strict fields; direct destruction and cyclic GC clearing;
   exactly-once field-value finalizer/weakref callbacks; reentrancy and
   resurrection; unchanged assignment/replacement/deletion cleanup; and
   copy/deepcopy/pickle round trips, including self-referential shadow values.
7. Preserved real `__dict__`/`vars` for ordinary classes, fixed-prefix overflow
   and non-string keys, dictionary materialization/growth without renumbering,
   missing/deleted/reinserted fields, allowed instance-dictionary clear, and
   whole-dictionary replacement preserving actual incoming identity and aliases.
   Cover source-requested slots/weakrefs, slotted subclasses with inherited
   dictionaries, automatic dynamic fallback, optional dynamic-field opt-outs,
   ordinary method/`ClassVar` shadowing on dynamic receivers, stock-subclass
   dictionary authority, generic dictless subclass behavior, and incompatible
   `__class__` assignment. Include metaclass hooks, multiple-inheritance
   conflicts, compatible mutable-type operations, and rejected post-publication
   schema/descriptor/base changes. Exercise direct member-descriptor setters,
   native generic setters, warmed bytecodes, and shared dictionary ownership.
8. Function `__code__`, defaults, kwdefaults **contents**, public native
   function setters, compatible vectorcall replacement, annotation, and
   descriptor mutation; mutable closure contents and mutable default contents;
   SOAC-owned vectorcall setup/restoration; escaped functions; and explicit
   mutable-function fallback, all without unsealing the module.
9. Ordinary-to-ordinary, ordinary-to-strict, strict-to-ordinary,
   strict-to-strict, and strict-to-stock-subclass call/attribute behavior,
   including dynamic descriptors, overrides, keyword binding, exceptions,
   finalizers, and the callee's actual globals/builtins; an ordinary caller may
   still enter an optimized strict body through its public Python ABI.
10. Structured plan assertions that strict contracts enable stable dictionary
    indexes, actual native offsets, class-default-fallback loads/stores, separate
    method-family slots, and unguarded stable targets only when enforcement is
    present, and that inherited exact/polymorphic default owners are selected
    from the actual receiver; callable class-data shadows, missing seal state, mutable
    exports, dynamic receivers, changed source hashes, or changed dependency
    fingerprints reject an invalid optimization. Distinguish callable fields
    from instance/static/class methods and nominal acceptance from exact type
    and participating-receiver proofs. Check runtime-enforced finality,
    participating virtual overrides, ordinary subclasses, actual callee
    environments, and method lookup before argument evaluation.
11. Independent profile/apply runs across process restart, changed strict
    dependencies, ordinary/strict transitions, replaced `sys.modules` entries,
    and stale cache/profile artifacts.
12. Private class/function annotation-cache slots reserved before freezing
    without invoking their providers or descriptors; user-defined providers,
    custom descriptor binding, metaclass lookup, and descriptor-side-effect
    timing; unobserved caches
    remaining empty and already-materialized initialization results remaining
    intact; compiler-installed class providers retaining their existing
    visibility; no observable new result-cache keys before the first normal
    read; first post-seal reads of module
    `__annotations__` and `__annotate__`, class annotations, and function
    annotations; provider timing, side effects, retryable exceptions,
    unresolved forward references, closure-local references, `VALUE`,
    `FORWARDREF`, and `STRING` formats in different call orders;
    owner-authenticated synthetic-globals replay,
    `VALUE_WITH_FAKE_GLOBALS`, preserved closures, lazy type-alias values and
    type-parameter bounds/constraints/defaults, absent optional owners,
    non-dictionary evaluate results, escaped legitimate lexical lambdas and
    generators, and rejection of forged, transferred, inherited, or expired
    replay authority;
    future-stringized annotations, returned dictionary identity, ordinary
    mutation of cached dictionary contents, repeated-read caching,
    frozen-provider replacement rejection, reentrant attempted mutation,
    nested/concurrent class/function cache publication, explicit rejection of
    conflicting nested module cache publication, objects created after
    sealing, visible class-cache materialization, normal `PyType_Modified`,
    and intact live strict method/slot facts without unsealing their owner.
13. Strict optimized integer operations crossing the signed-`i64` boundary,
    including `(2**63 - 1) + 1`, preserve the arbitrary-precision result;
    existing scalar-demand paths cannot substitute `OverflowError`. Existing
    iterator/generator fusion preserves recursion, exception propagation,
    suspension/resumption, safe ownership and required cleanup under the
    approved lifetime policy. SOAC frame inspection, traceback reconstruction
    and observer correspondence impose no matching-only refusal or fallback
    requirements.
14. Ordinary function calls on every backend, including annotation-mismatched
    parameters/results, positional-only/keyword-only/default/variadic binding,
    generated constructors, factory outputs and `InitVar`. Keep body exceptions,
    cleanup, lazy providers and effects preceding a selected field write.
    Opt-in fields cover Python/C/raw writes, dictionary replacement and
    deserialization; their failures are independent of warmup and call path.
    Guarded execution must not infer argument/result types from annotations or
    completed calls. Old function-check policy/artifact versions are rejected.
15. Authenticated offline artifacts, incompatible schema/checker/Python/policy
    versions, stale source/config/stubs/dependencies, conservative narrowing,
    ignored errors/unknown facts, and no checker work during hot imports. Every
    optimization test must distinguish the offline proposal, consumed
    construction contract, actual runtime capability, and selected typed plan.
16. Single-use construction handle replay/theft, cross-class/module transfer,
    wrong namespace functions/metaclasses/bases/phases, concurrent reuse,
    expired handles, repeated factories, and classes created after module
    sealing. Preserve both callback order and already-observed restrictions
    when a later decorator mismatches; never retrofit a layout or revoke a
    capability. Assert no temporary namespace/attribute or TLS authority.
17. Ordinary dataclass dictionaries, defaults/default factories, `ClassVar`,
    `InitVar`, inherited/keyword-only fields, descriptors, `__post_init__`,
    frozen initialization/setters, ordering/hash options, and `cached_property`.
    Actual `slots=True` replacement identity, linked construction handles,
    original/replacement callbacks, escaped originals, inherited dictionaries,
    closure cells, generated-function adoption, and unmodified shared foreign
    helpers must match the selected contract. Verify automatic Pydantic,
    Django, and SQLAlchemy dynamic fallback, including validation/coercion,
    method-name collisions, cached/computed descriptors, dictionary replacement,
    deferred rebuilding, and later instrumentation. Optional frozen-builtin or
    additional type contracts need separate tests when implemented; no test may
    treat an unchecked annotation as proof.

Assertions should target real behavior, class layouts, dictionary identities,
typed facts, selected plans, and native operation structure. Rendered
BlockPy/CLIF strings are not an adequate semantic regression surface.

## Performance evaluation

For each measured workload, report two configurations:

```text
1. stock CPython, original ordinary-Python source;
2. strict SOAC, explicitly opted-in equivalent source under the documented
   strict contract.
```

Pin the benchmark set, list every changed/opted-in module, report transformed
strict hot-path coverage, and report the strict-versus-stock score. Compare
against a previous strict SOAC revision when one exists, but do not require an
ordinary-SOAC execution or comparison. Run independent profile/apply passes
for each strict revision and use the order-alternating comparison discipline
from `OPT_GOAL.md`. A completed run with no authenticated sealed strict module
on the meaningful hot path does not demonstrate strict optimization progress.

`chaos`, `richards`, and `deltablue` are useful early workload candidates
because they exercise classes, fields, inheritance, and dynamic method
dispatch. A strictified benchmark is a different language configuration; its
source changes must be disclosed and must not alter the workload algorithm or
observable result. Its full-suite strict-versus-stock geometric mean is the
primary performance target and satisfies that goal when it reaches at least
`1.10x`.

## Decisions intentionally deferred

- The exact compact representation and overhead of protected CPython dict
  policies.
- Whether recognized dataclass/third-party transformations need a public
  strict class-layout adapter protocol.
- Whether strict vtables are per-class, per-interface, or lazily materialized
  from existing SOAC function metadata.
- Additional checked type constructors, mutable container-element contracts,
  and coroutine/generator boundary semantics beyond the selected shared
  project-level synchronous policy.
- Whether a future immutable private-cell design can provide enough measurable
  benefit or stronger native isolation to justify weaker `__dict__`
  interoperability.

[cinder-overview]: https://github.com/facebookincubator/cinderx/blob/main/cinderx/website/docs/StaticPython/index.md
[cinder-tutorial]: https://github.com/facebookincubator/cinderx/blob/main/cinderx/website/docs/StaticPython/tutorial.md
[cinder-patterns]: https://github.com/facebookincubator/cinderx/blob/main/cinderx/website/docs/StaticPython/incompatibilities.md
[cinder-strict-api]: https://github.com/facebookincubator/cinderx/blob/main/cinderx/PythonLib/__strict__/__init__.py
[cinder-strict-module]: https://github.com/facebookincubator/cinderx/blob/main/cinderx/StaticPython/strictmoduleobject.c
[cinder-strict-global-test]: https://github.com/facebookincubator/cinderx/blob/main/cinderx/PythonLib/test_cinderx/test_compiler/test_strict/test_strict_codegen.py
[cinder-default-slot-tests]: https://github.com/facebookincubator/cinderx/blob/main/cinderx/PythonLib/test_cinderx/test_compiler/test_static/test_slots_with_default.py
[cinder-default-descriptor]: https://github.com/facebookincubator/cinderx/blob/main/cinderx/StaticPython/descrs.c
[cinder-dependencies]: https://github.com/facebookincubator/cinderx/blob/main/cinderx/PythonLib/cinderx/compiler/static/dependency_invalidation.md
