//! Checked, source-bound class binding recipes from the same private native
//! compilation later used for strict catalog matching. These values are not
//! authentication, Python object owners, or native execution permissions.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{ensure, Context};
use soac_contracts::{Fingerprint, SourceRange};
use soac_core::block_py::{
    ClassBindingCaptureCreation, ClassBindingCodeNode, ClassBindingInitialValue, ClassBindingPhase,
    ClassBindingRecipe, ClassBindingSlotId, NativeCodeId, NativeCompileScopeKind,
    NativeLocalsPlusKind, NativeLocalsPlusSlot, NativeSymbolScopeKind,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalClassBindings {
    source_digest: Fingerprint,
    nodes: Vec<ClassBindingCodeNode>,
    classes: BTreeMap<NativeCodeId, ClassBindingRecipe>,
}

impl CanonicalClassBindings {
    /// The caller must independently prove that the node metadata came from
    /// its privately owned original code tree. Shape and digest validation do
    /// not turn caller-provided semantic data into runtime authority.
    pub fn from_native_entries(
        source: &str,
        nodes: Vec<ClassBindingCodeNode>,
        recipes: Vec<ClassBindingRecipe>,
    ) -> anyhow::Result<Self> {
        validate_nodes(source, &nodes)?;
        let mut classes = BTreeMap::new();
        for recipe in recipes {
            validate_recipe(source, &nodes, &recipe)
                .with_context(|| format!("native class recipe {:?}", recipe.class_code))?;
            ensure!(
                classes.insert(recipe.class_code, recipe).is_none(),
                "duplicate native class binding recipe"
            );
        }
        for node in &nodes {
            if node.compile_scope == NativeCompileScopeKind::Class {
                ensure!(
                    classes.contains_key(&node.id),
                    "native class {:?} has no binding recipe",
                    node.id
                );
            }
        }
        Ok(Self {
            source_digest: Fingerprint::digest(source.as_bytes()),
            nodes,
            classes,
        })
    }

    pub fn nodes(&self) -> &[ClassBindingCodeNode] {
        &self.nodes
    }

    pub fn node(&self, id: NativeCodeId) -> Option<&ClassBindingCodeNode> {
        self.nodes.get(id.0 as usize)
    }

    pub fn class_recipe(&self, id: NativeCodeId) -> Option<&ClassBindingRecipe> {
        self.classes.get(&id)
    }

    pub fn class_recipes(&self) -> impl ExactSizeIterator<Item = &ClassBindingRecipe> {
        self.classes.values()
    }

    pub(crate) fn matches_source(&self, source: &str) -> bool {
        self.source_digest == Fingerprint::digest(source.as_bytes())
    }
}

fn validate_range(source: &str, range: &SourceRange) -> anyhow::Result<()> {
    ensure!(
        range.start <= range.end
            && range.end as usize <= source.len()
            && source.is_char_boundary(range.start as usize)
            && source.is_char_boundary(range.end as usize),
        "native class binding metadata has an invalid source range {range:?}"
    );
    Ok(())
}

fn contains_range(outer: &SourceRange, inner: &SourceRange) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

fn validate_nodes(source: &str, nodes: &[ClassBindingCodeNode]) -> anyhow::Result<()> {
    let root = nodes
        .first()
        .context("native class metadata has no code root")?;
    ensure!(
        root.id == NativeCodeId(0)
            && root.parent.is_none()
            && root.compile_scope == NativeCompileScopeKind::Module
            && root.symbol_scope == NativeSymbolScopeKind::ModuleBlock,
        "native class metadata has an invalid module root"
    );
    let mut ancestors = vec![root.id];
    let mut class_ranges = BTreeSet::new();
    for (index, node) in nodes.iter().enumerate() {
        ensure!(
            node.id == NativeCodeId(u32::try_from(index)?),
            "native code IDs must be dense final-tree preorder ordinals"
        );
        if index != 0 {
            let parent = node.parent.context("non-root native code has no parent")?;
            while ancestors.last().is_some_and(|id| *id != parent) {
                ancestors.pop();
            }
            ensure!(
                ancestors.last() == Some(&parent),
                "native code parent is outside the final-tree preorder ancestry"
            );
            ancestors.push(node.id);
        }
        ensure!(
            node.first_line_marker(source).is_some(),
            "native code first line is outside its source"
        );
        if let Some(range) = &node.source_range {
            validate_range(source, range)?;
        }
        let first_free = node
            .slots
            .len()
            .checked_sub(node.freevar_count as usize)
            .context("native free-variable count exceeds localsplus slots")?;
        for (slot_index, slot) in node.slots.iter().enumerate() {
            ensure!(
                if slot_index >= first_free {
                    slot.kind == NativeLocalsPlusKind::FREE
                } else {
                    !slot.kind.is_free()
                },
                "native FREE slots must form the exact declared suffix"
            );
        }
        if node.compile_scope == NativeCompileScopeKind::Class {
            ensure!(
                node.symbol_scope == NativeSymbolScopeKind::ClassBlock,
                "native class code has a non-class symbol scope"
            );
            let range = node
                .source_range
                .as_ref()
                .context("original class code requires its actual source range")?;
            ensure!(
                range.start < range.end,
                "original class source range must be nonempty"
            );
            ensure!(
                class_ranges.insert((range.start, range.end)),
                "ambiguous original class source range"
            );
            ensure!(
                node.slots
                    .iter()
                    .all(|slot| slot.kind.is_valid_class_slot()),
                "native class has an unsupported localsplus kind"
            );
        } else {
            ensure!(
                node.symbol_scope != NativeSymbolScopeKind::ClassBlock,
                "native class symbol scope has a non-class compile kind"
            );
        }
    }
    Ok(())
}

fn selected_slot<'a>(
    node: &'a ClassBindingCodeNode,
    class_code: NativeCodeId,
    slot: ClassBindingSlotId,
) -> anyhow::Result<&'a NativeLocalsPlusSlot> {
    ensure!(
        slot.class_code == class_code,
        "foreign class current-slot reference"
    );
    node.slots
        .get(slot.index as usize)
        .context("class current-slot reference is out of bounds")
}

/// Eager comprehensions have their own lexical scope even when CPython inlines
/// them. This is a source-language query, not a native-slot omission receipt.
/// The first iterable is evaluated in the containing scope.
fn in_eager_comprehension_body(module: &ruff_python_ast::ModModule, range: SourceRange) -> bool {
    use ruff_python_ast::{
        self as ast,
        visitor::{walk_expr, Visitor},
    };
    use ruff_text_size::Ranged;
    struct Find {
        range: SourceRange,
        found: bool,
    }
    impl Find {
        fn contains(&self, expr: &ast::Expr) -> bool {
            u32::from(expr.start()) <= self.range.start && self.range.end <= u32::from(expr.end())
        }
        fn generators(&self, generators: &[ast::Comprehension]) -> bool {
            generators.iter().enumerate().any(|(index, generator)| {
                self.contains(&generator.target)
                    || (index != 0 && self.contains(&generator.iter))
                    || generator.ifs.iter().any(|test| self.contains(test))
            })
        }
    }
    impl<'a> Visitor<'a> for Find {
        fn visit_expr(&mut self, expr: &'a ast::Expr) {
            self.found |= match expr {
                ast::Expr::ListComp(comp) => {
                    self.contains(&comp.elt) || self.generators(&comp.generators)
                }
                ast::Expr::SetComp(comp) => {
                    self.contains(&comp.elt) || self.generators(&comp.generators)
                }
                ast::Expr::DictComp(comp) => {
                    comp.key.as_deref().is_some_and(|key| self.contains(key))
                        || self.contains(&comp.value)
                        || self.generators(&comp.generators)
                }
                _ => false,
            };
            if !self.found {
                walk_expr(self, expr);
            }
        }
    }
    if range.start == range.end {
        return false;
    }
    let mut find = Find {
        range,
        found: false,
    };
    for statement in &module.body {
        find.visit_stmt(statement);
    }
    find.found
}

fn validate_recipe(
    source: &str,
    nodes: &[ClassBindingCodeNode],
    recipe: &ClassBindingRecipe,
) -> anyhow::Result<()> {
    let code = nodes
        .get(recipe.class_code.0 as usize)
        .context("class recipe refers to an absent native code node")?;
    ensure!(
        code.compile_scope == NativeCompileScopeKind::Class
            && code.symbol_scope == NativeSymbolScopeKind::ClassBlock,
        "binding recipe requires actual class code"
    );
    let class_range = code
        .source_range
        .as_ref()
        .context("class source range is absent")?;
    let parsed = ruff_python_parser::parse_module(source).context("class lexical source syntax")?;
    let syntax = parsed.syntax();

    let mut initialized = BTreeSet::new();
    let mut required = BTreeSet::new();
    for ordinal in 0..code.freevar_count {
        let index = code.slots.len() - code.freevar_count as usize + ordinal as usize;
        required.insert(ClassBindingSlotId {
            class_code: code.id,
            index: u32::try_from(index)?,
        });
    }
    let mut header_stores = BTreeSet::new();
    let mut header_roles = BTreeSet::new();
    let mut header_started = false;
    for initializer in &recipe.initializers {
        let slot = selected_slot(code, recipe.class_code, initializer.slot)?;
        ensure!(
            slot.kind.is_cell(),
            "class initializer requires actual lexical cell storage"
        );
        match initializer.phase {
            ClassBindingPhase::ClassEntry => {
                ensure!(
                    !header_started,
                    "class-entry initialization follows class header stores"
                );
                ensure!(
                    initialized.insert(initializer.slot),
                    "class cell is initialized twice"
                );
                let valid = match initializer.value {
                    ClassBindingInitialValue::EmptyCell => !slot.kind.is_free(),
                    ClassBindingInitialValue::IncomingFree { ordinal } => {
                        let first = code.slots.len() - code.freevar_count as usize;
                        slot.kind == NativeLocalsPlusKind::FREE
                            && code.freevar_slot(ordinal).is_some()
                            && initializer.slot.index as usize == first + ordinal as usize
                    }
                    ClassBindingInitialValue::NamespaceStore
                    | ClassBindingInitialValue::ConditionalSetStore => false,
                };
                ensure!(
                    valid,
                    "class-entry initializer does not match lexical cell storage"
                );
            }
            ClassBindingPhase::ClassHeaderComplete => {
                header_started = true;
                ensure!(
                    initialized.contains(&initializer.slot)
                        && header_stores.insert(initializer.slot)
                        && header_roles.insert(initializer.value)
                        && !slot.kind.is_free()
                        && matches!(
                            initializer.value,
                            ClassBindingInitialValue::NamespaceStore
                                | ClassBindingInitialValue::ConditionalSetStore
                        ),
                    "class header store requires one initialized class-owned cell"
                );
                required.insert(initializer.slot);
            }
        }
    }

    let mut captures = BTreeMap::<_, BTreeSet<u32>>::new();
    let mut completion_child = None;
    for capture in &recipe.captures {
        let slot = selected_slot(code, recipe.class_code, capture.source)?;
        ensure!(
            slot.kind.is_cell(),
            "child closure capture requires a class cell"
        );
        required.insert(capture.source);
        let child = nodes
            .get(capture.child.0 as usize)
            .context("class capture refers to an absent child")?;
        ensure!(
            child.parent == Some(recipe.class_code),
            "class capture must target an actual direct native child"
        );
        ensure!(
            !child
                .source_range
                .is_some_and(|range| in_eager_comprehension_body(syntax, range)),
            "outlined comprehension capture does not belong to the class scope"
        );
        let child_slot = child
            .freevar_slot(capture.freevar_ordinal)
            .context("class capture free-variable ordinal is out of bounds")?;
        ensure!(
            slot.name == child_slot.name,
            "class capture slot name differs from the child's native free variable"
        );
        capture
            .creation
            .validate(source, code, child)
            .map_err(anyhow::Error::msg)?;
        if matches!(
            &capture.creation,
            ClassBindingCaptureCreation::ClassAnnotationBodyCompletion { .. }
        ) {
            ensure!(
                completion_child.is_none_or(|id| id == capture.child),
                "class has multiple deferred variable annotation providers"
            );
            completion_child = Some(capture.child);
        }
        ensure!(
            captures
                .entry((capture.child, capture.creation.clone()))
                .or_default()
                .insert(capture.freevar_ordinal),
            "duplicate class child capture ordinal at one creation site"
        );
    }
    if let Some(child) = completion_child {
        ensure!(
            recipe
                .captures
                .iter()
                .filter(|capture| capture.child == child)
                .all(|capture| {
                    matches!(
                        &capture.creation,
                        ClassBindingCaptureCreation::ClassAnnotationBodyCompletion { .. }
                    )
                }),
            "class annotation provider has conflicting capture creation phases"
        );
    }
    for ((child, _), ordinals) in &captures {
        ensure!(
            ordinals.len() == nodes[child.0 as usize].freevar_count as usize,
            "class child creation has an incomplete free-variable projection"
        );
    }
    for child in nodes
        .iter()
        .filter(|node| node.parent == Some(recipe.class_code) && node.freevar_count != 0)
    {
        // A lambda/genexpr inside the eager body is created by the ordinary
        // helper and captures that helper's cells. Direct class declarations
        // and the outer iterable still require every authenticated capture.
        if child
            .source_range
            .is_some_and(|range| in_eager_comprehension_body(syntax, range))
        {
            continue;
        }
        ensure!(
            captures.keys().any(|(id, _)| *id == child.id),
            "native class child has no closure capture recipe"
        );
    }

    let mut accesses = BTreeSet::new();
    for access in &recipe.accesses {
        validate_range(source, &access.source_range)?;
        ensure!(
            access.source_range.start < access.source_range.end
                && contains_range(class_range, &access.source_range),
            "native source-name access requires its nonempty original range inside the class"
        );
        ensure!(
            !in_eager_comprehension_body(syntax, access.source_range),
            "outlined comprehension access does not belong to the class scope"
        );
        let slot = selected_slot(code, recipe.class_code, access.source)?;
        let valid = match access.selection {
            soac_core::block_py::ClassBindingAccessSelection::RawSlot => false,
            soac_core::block_py::ClassBindingAccessSelection::CellValue => slot.kind.is_cell(),
            soac_core::block_py::ClassBindingAccessSelection::NamespaceOrCell => {
                access.context == soac_core::block_py::ClassBindingAccessContext::Load
                    && slot.kind.is_cell()
            }
        };
        ensure!(
            valid,
            "native source-name access does not match its selected lexical cell"
        );
        required.insert(access.source);
        ensure!(
            accesses.insert((
                access.source_range.start,
                access.source_range.end,
                access.context
            )),
            "ambiguous native source-name access at one original range/context"
        );
    }
    let mut exports = BTreeSet::new();
    for export in &recipe.exports {
        let slot = selected_slot(code, recipe.class_code, export.source)?;
        ensure!(slot.kind.is_cell(), "class export requires a class cell");
        ensure!(
            exports.insert(export.kind),
            "duplicate native class cell export"
        );
        required.insert(export.source);
    }
    ensure!(
        initialized == required,
        "class cell initialization does not cover its lexical obligations"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_core::block_py::{
        ClassBindingCapture, ClassBindingExport, ClassBindingExportKind, ClassBindingInitializer,
        ModuleNameGen,
    };

    const SOURCE: &str = "def build(outside):\n    class Box:\n        if bool(outside):\n            values = [lambda: outside for outside in (1,)]\n        def method(self):\n            return outside\n    return Box\n";
    const CLASS: NativeCodeId = NativeCodeId(2);

    fn slot(index: u32) -> ClassBindingSlotId {
        ClassBindingSlotId {
            class_code: CLASS,
            index,
        }
    }

    fn range(text: &str) -> SourceRange {
        let start = SOURCE.find(text).unwrap() as u32;
        SourceRange::new(start, start + text.len() as u32)
    }

    fn native_slot(name: &str, kind: u8) -> NativeLocalsPlusSlot {
        NativeLocalsPlusSlot {
            name: name.into(),
            kind: NativeLocalsPlusKind(kind),
        }
    }

    // Value-only lexical-cell data; it does not stand in for the runtime's
    // same-native-tree authentication.
    fn fixture() -> (Vec<ClassBindingCodeNode>, ClassBindingRecipe) {
        let class_range = SourceRange::new(
            SOURCE.find("class Box:").unwrap() as u32,
            SOURCE.rfind("outside").unwrap() as u32 + "outside".len() as u32,
        );
        let lambda_range = range("lambda: outside");
        let method_range =
            SourceRange::new(SOURCE.find("def method").unwrap() as u32, class_range.end);
        let node = |id,
                    parent: Option<u32>,
                    compile_scope,
                    symbol_scope,
                    source_range: Option<SourceRange>,
                    slots,
                    freevar_count| {
            ClassBindingCodeNode {
                id: NativeCodeId(id),
                parent: parent.map(NativeCodeId),
                compile_scope,
                symbol_scope,
                first_line: source_range.map_or(1, |range| {
                    1 + SOURCE[..range.start as usize]
                        .bytes()
                        .filter(|byte| *byte == b'\n')
                        .count() as u32
                }),
                source_range,
                slots,
                freevar_count,
            }
        };
        let nodes = vec![
            node(
                0,
                None,
                NativeCompileScopeKind::Module,
                NativeSymbolScopeKind::ModuleBlock,
                None,
                vec![],
                0,
            ),
            node(
                1,
                Some(0),
                NativeCompileScopeKind::Function,
                NativeSymbolScopeKind::FunctionBlock,
                Some(SourceRange::new(0, SOURCE.len() as u32)),
                vec![native_slot("outside", 0x62)],
                0,
            ),
            node(
                2,
                Some(1),
                NativeCompileScopeKind::Class,
                NativeSymbolScopeKind::ClassBlock,
                Some(class_range),
                vec![
                    native_slot("outside", 0x30),
                    native_slot("outside", 0x40),
                    native_slot("outside", 0x80),
                ],
                1,
            ),
            node(
                3,
                Some(2),
                NativeCompileScopeKind::Lambda,
                NativeSymbolScopeKind::FunctionBlock,
                Some(lambda_range),
                vec![native_slot("outside", 0x80)],
                1,
            ),
            node(
                4,
                Some(2),
                NativeCompileScopeKind::Function,
                NativeSymbolScopeKind::FunctionBlock,
                Some(method_range),
                vec![native_slot("self", 0x22), native_slot("outside", 0x80)],
                1,
            ),
        ];
        let recipe = ClassBindingRecipe {
            class_code: CLASS,
            initializers: vec![ClassBindingInitializer {
                phase: ClassBindingPhase::ClassEntry,
                slot: slot(2),
                value: ClassBindingInitialValue::IncomingFree { ordinal: 0 },
            }],
            // The lambda in the eager body captures the outlined helper's
            // iteration cell; only the direct method captures the class FREE.
            captures: vec![ClassBindingCapture {
                child: NativeCodeId(4),
                creation: ClassBindingCaptureCreation::SourceRange(method_range),
                freevar_ordinal: 0,
                source: slot(2),
            }],
            exports: vec![],
            accesses: vec![],
        };
        (nodes, recipe)
    }

    #[test]
    fn canonical_class_bindings_preserve_explicit_source_name_context_and_slot() {
        use soac_core::block_py::{
            ClassBindingAccess, ClassBindingAccessContext as Context,
            ClassBindingAccessSelection as Selection,
        };
        let (nodes, mut recipe) = fixture();
        let text = range("outside):\n            values");
        let site = SourceRange::new(text.start, text.start + "outside".len() as u32);
        recipe.accesses = vec![
            ClassBindingAccess {
                source_range: site,
                context: Context::Load,
                selection: Selection::NamespaceOrCell,
                source: slot(2),
            },
            ClassBindingAccess {
                source_range: site,
                context: Context::Store,
                selection: Selection::CellValue,
                source: slot(2),
            },
            ClassBindingAccess {
                source_range: site,
                context: Context::Delete,
                selection: Selection::CellValue,
                source: slot(2),
            },
        ];
        let checked = CanonicalClassBindings::from_native_entries(
            SOURCE,
            nodes.clone(),
            vec![recipe.clone()],
        )
        .unwrap();
        assert_eq!(
            checked.class_recipe(CLASS).unwrap().accesses,
            recipe.accesses
        );
        let cases: &[(&str, fn(&mut ClassBindingRecipe))] = &[
            ("namespace fallback on store", |recipe| {
                recipe.accesses[1].selection = Selection::NamespaceOrCell
            }),
            ("raw free access", |recipe| {
                recipe.accesses[1].selection = Selection::RawSlot
            }),
            ("missing source span", |recipe| {
                let at = recipe.accesses[0].source_range.start;
                recipe.accesses[0].source_range = SourceRange::new(at, at);
            }),
            ("foreign selected cell", |recipe| {
                recipe.accesses[0].source.class_code = NativeCodeId(0)
            }),
            ("ambiguous source context", |recipe| {
                recipe.accesses.push(recipe.accesses[0].clone())
            }),
            ("helper iteration access", |recipe| {
                let at = range("outside in").start;
                recipe.accesses[0].source_range = SourceRange::new(at, at + 7);
            }),
        ];
        for &(label, edit) in cases {
            let mut invalid = recipe.clone();
            edit(&mut invalid);
            assert!(
                CanonicalClassBindings::from_native_entries(SOURCE, nodes.clone(), vec![invalid])
                    .is_err(),
                "{label}"
            );
        }
    }

    #[test]
    fn canonical_class_bindings_keep_class_free_cells_separate_from_helper_iteration() {
        let (nodes, recipe) = fixture();
        let checked =
            CanonicalClassBindings::from_native_entries(SOURCE, nodes, vec![recipe.clone()])
                .unwrap();
        assert_eq!(checked.class_recipe(CLASS), Some(&recipe));
        assert_eq!(checked.class_recipes().len(), 1);
        assert_eq!(checked.nodes().len(), 5);
        assert!(checked.matches_source(SOURCE));
        assert!(!checked.matches_source(&SOURCE.replace("outside", "another")));
        let class = checked.node(CLASS).unwrap();
        assert_eq!(class.slots[1].name, class.slots[2].name);
        assert_ne!(class.slots[1].kind, class.slots[2].kind);
        assert_eq!(recipe.initializers.len(), 1);
        assert_eq!(recipe.initializers[0].slot, slot(2));
        assert_eq!(recipe.captures[0].child, NativeCodeId(4));
        assert_eq!(recipe.captures[0].source, slot(2));
        let value = (checked.nodes().to_vec(), recipe);
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&value).unwrap();
        assert_eq!(rkyv::from_bytes::<(Vec<ClassBindingCodeNode>, ClassBindingRecipe), rkyv::rancor::Error>(&bytes).unwrap(), value);
    }

    type Edit = fn(&mut Vec<ClassBindingCodeNode>, &mut ClassBindingRecipe);

    fn rejects(edits: &[(&str, Edit)]) {
        for &(label, edit) in edits {
            let (mut nodes, mut recipe) = fixture();
            edit(&mut nodes, &mut recipe);
            assert!(
                CanonicalClassBindings::from_native_entries(SOURCE, nodes, vec![recipe]).is_err(),
                "invalid native class metadata was accepted: {label}",
            );
        }
    }

    #[test]
    fn canonical_class_bindings_reject_ambiguous_tree_and_foreign_storage() {
        rejects(&[
            ("nonmodule root", |nodes, _| {
                nodes[0].compile_scope = NativeCompileScopeKind::Function
            }),
            ("nondense code id", |nodes, _| nodes[3].id = NativeCodeId(7)),
            ("reopened tree branch", |nodes, _| {
                nodes[3].parent = Some(NativeCodeId(1));
                nodes[4].parent = Some(CLASS);
            }),
            ("class lacks original range", |nodes, _| {
                nodes[2].source_range = None
            }),
            ("class has wrong symbol kind", |nodes, _| {
                nodes[2].symbol_scope = NativeSymbolScopeKind::FunctionBlock
            }),
            ("nonfree suffix", |nodes, _| {
                nodes[2].slots[2].kind = NativeLocalsPlusKind::CELL
            }),
            ("argument class slot", |nodes, _| {
                nodes[2].slots[0].kind = NativeLocalsPlusKind(0x22)
            }),
            ("foreign initializer cell", |_, recipe| {
                recipe.initializers[0].slot.class_code = NativeCodeId(1)
            }),
            ("missing initializer", |_, recipe| {
                recipe.initializers.pop();
            }),
            ("duplicate initializer", |_, recipe| {
                recipe.initializers.push(recipe.initializers[0].clone())
            }),
            ("wrong incoming ordinal", |_, recipe| {
                recipe.initializers[0].value = ClassBindingInitialValue::IncomingFree { ordinal: 1 }
            }),
            ("raw local initialized as cell", |_, recipe| {
                recipe.initializers[0].slot = slot(0);
                recipe.initializers[0].value = ClassBindingInitialValue::EmptyCell;
            }),
            ("unneeded native iteration cell", |_, recipe| {
                recipe.initializers.push(ClassBindingInitializer {
                    phase: ClassBindingPhase::ClassEntry,
                    slot: slot(1),
                    value: ClassBindingInitialValue::EmptyCell,
                })
            }),
        ]);
        let (nodes, recipe) = fixture();
        assert!(
            CanonicalClassBindings::from_native_entries(SOURCE, nodes.clone(), vec![]).is_err()
        );
        assert!(CanonicalClassBindings::from_native_entries(
            SOURCE,
            nodes,
            vec![recipe.clone(), recipe]
        )
        .is_err());
    }

    #[test]
    fn canonical_class_bindings_reject_incomplete_captures_and_invalid_exports() {
        rejects(&[
            ("raw noncell capture", |_, recipe| {
                recipe.captures[0].source = slot(0)
            }),
            ("foreign capture slot", |_, recipe| {
                recipe.captures[0].source.class_code = NativeCodeId(1)
            }),
            ("wrong native child", |_, recipe| {
                recipe.captures[0].child = NativeCodeId(1)
            }),
            ("wrong free ordinal", |_, recipe| {
                recipe.captures[0].freevar_ordinal = 1
            }),
            ("wrong free name", |nodes, _| {
                nodes[4].slots[1].name = "different".into()
            }),
            ("missing child capture", |_, recipe| {
                recipe.captures.pop();
            }),
            ("duplicate child capture", |_, recipe| {
                recipe.captures.push(recipe.captures[0].clone())
            }),
            ("outside creation range", |_, recipe| {
                recipe.captures[0].creation =
                    ClassBindingCaptureCreation::SourceRange(SourceRange::new(0, 3))
            }),
            ("noncell export", |_, recipe| {
                recipe.exports.push(ClassBindingExport {
                    kind: ClassBindingExportKind::ClassCell,
                    source: slot(0),
                })
            }),
            ("duplicate export role", |_, recipe| {
                let export = ClassBindingExport {
                    kind: ClassBindingExportKind::ClassCell,
                    source: slot(1),
                };
                recipe.exports = vec![export.clone(), export];
            }),
        ]);
    }

    #[test]
    fn canonical_class_bindings_preserve_header_store_phase_and_current_slot() {
        let (mut nodes, mut recipe) = fixture();
        nodes[2].slots.insert(2, native_slot("__classdict__", 0x40));
        recipe.initializers[0].slot.index = 3;
        recipe.captures[0].source.index = 3;
        let classdict = slot(2);
        recipe.initializers.push(ClassBindingInitializer {
            phase: ClassBindingPhase::ClassEntry,
            slot: classdict,
            value: ClassBindingInitialValue::EmptyCell,
        });
        recipe.initializers.push(ClassBindingInitializer {
            phase: ClassBindingPhase::ClassHeaderComplete,
            slot: classdict,
            value: ClassBindingInitialValue::NamespaceStore,
        });
        recipe.exports.push(ClassBindingExport {
            kind: ClassBindingExportKind::ClassDictCell,
            source: classdict,
        });
        CanonicalClassBindings::from_native_entries(SOURCE, nodes.clone(), vec![recipe.clone()])
            .unwrap();
        let mut invalid = recipe.clone();
        invalid.initializers.swap(0, 2);
        assert!(
            CanonicalClassBindings::from_native_entries(SOURCE, nodes.clone(), vec![invalid])
                .is_err()
        );
        recipe
            .initializers
            .push(recipe.initializers.last().unwrap().clone());
        assert!(CanonicalClassBindings::from_native_entries(SOURCE, nodes, vec![recipe]).is_err());
    }

    #[test]
    fn canonical_class_bindings_allow_zero_width_synthetic_sites_not_class_declarations() {
        let (mut nodes, mut recipe) = fixture();
        let at = nodes[4].source_range.unwrap().start;
        let zero = SourceRange::new(at, at);
        nodes[4].source_range = Some(zero);
        recipe.captures[0].creation = ClassBindingCaptureCreation::SourceRange(zero);
        CanonicalClassBindings::from_native_entries(SOURCE, nodes.clone(), vec![recipe.clone()])
            .unwrap();
        nodes[2].source_range = Some(zero);
        assert!(CanonicalClassBindings::from_native_entries(SOURCE, nodes, vec![recipe]).is_err());
    }

    #[test]
    fn canonical_class_bindings_reject_non_utf8_boundary_ranges() {
        let source = "class É:\n    pass\n";
        let mut nodes = vec![
            ClassBindingCodeNode {
                id: NativeCodeId(0),
                parent: None,
                compile_scope: NativeCompileScopeKind::Module,
                symbol_scope: NativeSymbolScopeKind::ModuleBlock,
                first_line: 1,
                source_range: None,
                slots: vec![],
                freevar_count: 0,
            },
            ClassBindingCodeNode {
                id: NativeCodeId(1),
                parent: Some(NativeCodeId(0)),
                compile_scope: NativeCompileScopeKind::Class,
                symbol_scope: NativeSymbolScopeKind::ClassBlock,
                first_line: 1,
                source_range: Some(SourceRange::new(0, source.len() as u32)),
                slots: vec![],
                freevar_count: 0,
            },
        ];
        let recipe = ClassBindingRecipe {
            class_code: NativeCodeId(1),
            initializers: vec![],
            captures: vec![],
            exports: vec![],
            accesses: vec![],
        };
        CanonicalClassBindings::from_native_entries(source, nodes.clone(), vec![recipe.clone()])
            .unwrap();
        nodes[1].source_range = Some(SourceRange::new(
            source.find('É').unwrap() as u32 + 1,
            source.len() as u32,
        ));
        assert!(CanonicalClassBindings::from_native_entries(source, nodes, vec![recipe]).is_err());
    }

    #[test]
    fn canonical_class_bindings_driver_rejects_stale_source_without_granting_authority() {
        use soac_core::pass_tracker::RecordingPassTracker;
        use std::sync::Arc;
        let (nodes, recipe) = fixture();
        let canonical = Arc::new(
            CanonicalClassBindings::from_native_entries(SOURCE, nodes, vec![recipe]).unwrap(),
        );
        let options = crate::LoweringOptions {
            canonical_class_bindings: Some(canonical),
            ..Default::default()
        };
        let module = crate::lower_source_to_blockpy_module_with_tracker(
            SOURCE,
            ModuleNameGen::new(0),
            &mut RecordingPassTracker::new(),
            options.clone(),
        )
        .unwrap();
        assert!(
            module.strict_source.is_none(),
            "semantic metadata cannot admit ordinary source"
        );
        let error = crate::lower_source_to_blockpy_module_with_tracker(
            &SOURCE.replace("bool", "bool "),
            ModuleNameGen::new(0),
            &mut RecordingPassTracker::new(),
            options,
        )
        .err()
        .expect("stale native metadata must be rejected");
        assert!(matches!(
            error,
            crate::LoweringError::StrictAuthentication(_)
        ));
    }
}
