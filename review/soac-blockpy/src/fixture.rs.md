# soac-blockpy/src/fixture.rs

## File Responsibilities

Parses and renders simple transform fixture files split into named blocks with input and expected
output separated by `# ==`.

## Datatypes

- `FixtureBlock`: one fixture case with name, input text, output text, and whether the separator was
  seen.
- `FixtureSection`: parser state, either waiting for a block or collecting a block.

## Functions

- `parse_fixture`: parses fixture contents, validates block headers/separators, preserves line
  endings, and returns blocks.
- `render_fixture`: renders normalized fixture blocks back to the project fixture format.

## Context Read

- `soac-blockpy/src/bin/regen_snapshots/mod.rs`
