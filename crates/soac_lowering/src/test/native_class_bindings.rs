//! Test fixtures use the selected native compiler's original code tree.
//!
//! The child process compiles but never executes the source. The production
//! validator checks the pointer-free class-only projection; this creates no
//! runtime authority. The native schema is v7, while recipe rows below encode
//! the unchanged ClassBindingRecipe value model, not the native scope wire.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use anyhow::{ensure, Context};
use serde_json::Value;
use soac_contracts::{Fingerprint, SourceRange};
use soac_core::block_py::*;

use crate::CanonicalClassBindings;

pub(crate) fn for_source(source: &str) -> anyhow::Result<Arc<CanonicalClassBindings>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let interpreter = std::env::var_os("CPYTHON_BIN")
        .map(std::path::PathBuf::from)
        .context("native class fixtures require the selected CPYTHON_BIN; run through just --command cargo test")?;
    let mut child = Command::new(&interpreter)
        .args([
            "-I",
            "-S",
            "-B",
            "-c",
            include_str!("native_class_bindings.py"),
        ])
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("start selected native compiler {}", interpreter.display()))?;
    child
        .stdin
        .take()
        .context("native compiler stdin")?
        .write_all(source.as_bytes())?;
    let output = child.wait_with_output()?;
    ensure!(
        output.status.success(),
        "selected native class fixture compilation failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let packet: Value = serde_json::from_slice(&output.stdout)?;
    ensure!(
        unsigned(&packet["native_schema"])? == CLASS_BINDINGS_SCHEMA_VERSION,
        "native fixture schema"
    );
    ensure!(
        packet["source_sha256"].as_str()
            == Some(Fingerprint::digest(source.as_bytes()).to_hex().as_str()),
        "native fixture source digest"
    );
    let nodes = values(&packet["nodes"])?
        .iter()
        .map(node)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let recipes = values(&packet["class_projection"])?
        .iter()
        .map(|value| recipe(value, source, &nodes))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(Arc::new(CanonicalClassBindings::from_native_entries(
        source, nodes, recipes,
    )?))
}

fn values(value: &Value) -> anyhow::Result<&Vec<Value>> {
    value.as_array().context("native fixture array")
}

fn row<const N: usize>(value: &Value) -> anyhow::Result<&[Value; N]> {
    values(value)?
        .as_slice()
        .try_into()
        .context("native fixture row length")
}

fn unsigned(value: &Value) -> anyhow::Result<u32> {
    u32::try_from(value.as_u64().context("native fixture unsigned integer")?)
        .context("native fixture integer range")
}

fn optional_unsigned(value: &Value) -> anyhow::Result<Option<u32>> {
    if value.is_null() {
        Ok(None)
    } else {
        unsigned(value).map(Some)
    }
}

fn tag<T>(value: &Value, decode: fn(u32) -> Option<T>) -> anyhow::Result<T> {
    decode(unsigned(value)?).context("native fixture enum tag")
}

fn range(value: &Value) -> anyhow::Result<SourceRange> {
    let [start, end] = row(value)?;
    Ok(SourceRange::new(unsigned(start)?, unsigned(end)?))
}

fn optional_range(value: &Value) -> anyhow::Result<Option<SourceRange>> {
    if value.is_null() {
        Ok(None)
    } else {
        range(value).map(Some)
    }
}

fn native_kind(value: &Value) -> anyhow::Result<NativeLocalsPlusKind> {
    Ok(NativeLocalsPlusKind(u8::try_from(unsigned(value)?)?))
}

fn node(value: &Value) -> anyhow::Result<ClassBindingCodeNode> {
    let [id, parent, compile_scope, symbol_scope, source_range, slots, freevar_count, first_line] =
        row(value)?;
    Ok(ClassBindingCodeNode {
        id: NativeCodeId(unsigned(id)?),
        parent: optional_unsigned(parent)?.map(NativeCodeId),
        compile_scope: tag(compile_scope, NativeCompileScopeKind::from_wire)?,
        symbol_scope: tag(symbol_scope, NativeSymbolScopeKind::from_wire)?,
        first_line: unsigned(first_line)?,
        source_range: optional_range(source_range)?,
        slots: values(slots)?
            .iter()
            .map(|value| {
                let [name, kind] = row(value)?;
                Ok(NativeLocalsPlusSlot {
                    name: name
                        .as_str()
                        .context("native fixture slot name")?
                        .to_owned(),
                    kind: native_kind(kind)?,
                })
            })
            .collect::<anyhow::Result<_>>()?,
        freevar_count: unsigned(freevar_count)?,
    })
}

fn recipe(
    value: &Value,
    source_text: &str,
    nodes: &[ClassBindingCodeNode],
) -> anyhow::Result<ClassBindingRecipe> {
    let [code, initializers, captures, exports, accesses] = row(value)?;
    let class_code = NativeCodeId(unsigned(code)?);
    let parent = nodes
        .get(class_code.0 as usize)
        .context("native fixture class node")?;
    let slot_id = |index| ClassBindingSlotId { class_code, index };
    let current_slot = |value| -> anyhow::Result<ClassBindingSlotId> {
        let [kind, index] = row(value)?;
        ensure!(unsigned(kind)? == 0, "native fixture current slot tag");
        Ok(slot_id(unsigned(index)?))
    };
    Ok(ClassBindingRecipe {
        class_code,
        initializers: values(initializers)?
            .iter()
            .map(|value| {
                let [phase, slot, kind, operand] = row(value)?;
                Ok(ClassBindingInitializer {
                    phase: tag(phase, ClassBindingPhase::from_wire)?,
                    slot: slot_id(unsigned(slot)?),
                    value: ClassBindingInitialValue::from_wire(
                        unsigned(kind)?,
                        optional_unsigned(operand)?,
                    )
                    .context("native fixture initializer")?,
                })
            })
            .collect::<anyhow::Result<_>>()?,
        captures: values(captures)?
            .iter()
            .map(|value| {
                let [child, creation_range, ordinal, source] = row(value)?;
                let child = NativeCodeId(unsigned(child)?);
                let child_node = nodes
                    .get(child.0 as usize)
                    .context("native fixture capture child")?;
                let creation = ClassBindingCaptureCreation::from_native_marker(
                    source_text,
                    parent,
                    child_node,
                    optional_range(creation_range)?,
                )
                .map_err(anyhow::Error::msg)?;
                Ok(ClassBindingCapture {
                    child,
                    creation,
                    freevar_ordinal: unsigned(ordinal)?,
                    source: current_slot(source)?,
                })
            })
            .collect::<anyhow::Result<_>>()?,
        exports: values(exports)?
            .iter()
            .map(|value| {
                let [kind, source] = row(value)?;
                Ok(ClassBindingExport {
                    kind: tag(kind, ClassBindingExportKind::from_wire)?,
                    source: current_slot(source)?,
                })
            })
            .collect::<anyhow::Result<_>>()?,
        accesses: values(accesses)?
            .iter()
            .map(|value| {
                let [source_range, context, selection, source] = row(value)?;
                Ok(ClassBindingAccess {
                    source_range: range(source_range)?,
                    context: tag(context, ClassBindingAccessContext::from_wire)?,
                    selection: tag(selection, ClassBindingAccessSelection::from_wire)?,
                    source: current_slot(source)?,
                })
            })
            .collect::<anyhow::Result<_>>()?,
    })
}
