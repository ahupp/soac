# Redundant Guard And Refcount Elimination

## Article Spark

V8's optimizing tiers remove duplicate map checks and other redundant checks after a dominating
guard has proved a shape/type.

## SOAC Question

Can SOAC track guard facts and owned-reference facts within a JIT function so repeated type/version,
dict-key, deleted-sentinel, and refcount operations collapse?

## Concrete Experiment

- Add a fact lattice over BB blocks:
  - exact Python type / owner type version known
  - module/type key-layout guard known
  - local slot known not DELETED
  - value is an owned live temporary
- Use it only for instructions dominated by the guard and invalidated at Python-call / helper-call
  boundaries unless the callee is known not to mutate the relevant object.
- Remove or fold repeated guard/check/refcount sequences in CLIF emission.

## Success Signal

- Rendered specialized CLIF for pystone hot functions has fewer repeated owner-type/version guards
  and local deleted checks.
- Perf shows lower time in incref/decref and guard-helper code.
- Verification counters still report the same fast-path hit/fallback split.

## Risks

- Python calls can mutate modules, types, instance dicts, builtins, descriptors, and globals.
- Refcount elision must preserve destructor timing unless the change is explicitly guarded behind an
  unsound performance flag.
