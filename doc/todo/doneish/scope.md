 In general the scoping / name representation is a mess.  Some issues:

  1. We lower to explicit load/store operations very early, before even getting to blockpy.
  2. Scope analysis is done repeatedly because I wasn't confident it would carry through all the passes.
  3. There's inconsistnt naming/duplication between _dp_cell_X and X
  4. We have a home-grown analyzer and I'd have more confidence in the ruff one.
  5. The lowering of classes to real python was helpful in the beginning, but is awkward today.


I'd like to rethink how all this works.  A rough sketch, in order:

 1. We do a Ruff semantic analysis once, early on, probably immediately after rewrite_ann_assign_to_dunder_annotate.

 2. Add sanity checks through all passes to


enum Location {
    Globals,
    Locals,
}

struct NameScope {
    read: Location
    write: Location
}