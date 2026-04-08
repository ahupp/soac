# Codex Optimization Log

## 2026-04-08 - Inline runtime guard and indexed-field helpers

- `kkoolpkp` inlines exact type/version guards into soac-runtime, replaces the
  `_PyObject_GetDictPtr` call with direct dict/inline-values access in indexed
  field helpers, and makes unsound indexed field stores return hit/miss status
  instead of an owned temporary.
- 100k pystone, default specialized: `154514 -> 160710 loops/s`.
- 100k pystone, unsound indexed stores: `142627 -> 159810 loops/s`.
- Headline after: default specialized `160710 loops/s`, opt-in unsound stores
  `159810 loops/s`, same-run stock CPython about `555k loops/s`.
