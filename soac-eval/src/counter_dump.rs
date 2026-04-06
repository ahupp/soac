#[cfg(not(target_endian = "little"))]
compile_error!("counter dump format currently requires little-endian hosts");

use memmap2::Mmap;
use soac_blockpy::block_py::{BlockLabel, FunctionId, InstrId};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::mem::{align_of, size_of};
use std::path::Path;

pub const COUNTER_DUMP_MAGIC: [u8; 8] = *b"SOACCNTR";
pub const COUNTER_DUMP_VERSION: u16 = 6;
pub const COUNTER_DUMP_NONE_U32: u32 = u32::MAX;
pub const COUNTER_DUMP_NONE_U64: u64 = u64::MAX;
pub const COUNTER_DUMP_NONE_FUNCTION_ID: u64 = 0;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CounterDumpRecordHeader {
    pub magic: [u8; 8],
    pub version: u16,
    pub header_size: u16,
    pub record_len: u32,
    pub row_count: u32,
    pub string_count: u32,
    pub string_bytes_len: u32,
    pub module_name_string_id: u32,
    pub package_name_string_id: u32,
    pub string_offsets_offset: u32,
    pub string_bytes_offset: u32,
    pub counter_id_offset: u32,
    pub scope_offset: u32,
    pub kind_offset: u32,
    pub site_kind_offset: u32,
    pub function_id_offset: u32,
    pub current_function_id_offset: u32,
    pub instr_block_label_offset: u32,
    pub instr_index_in_block_offset: u32,
    pub function_qualname_offset: u32,
    pub block_label_offset: u32,
    pub value_offset: u32,
    pub observed_value_offset: u32,
    pub max_overcount_offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CounterDumpRow {
    pub counter_id: u32,
    pub scope: String,
    pub kind: String,
    pub site_kind: String,
    pub function_id: Option<FunctionId>,
    pub current_function_id: Option<FunctionId>,
    pub instr_id: Option<InstrId>,
    pub function_qualname: Option<String>,
    pub block_label: Option<String>,
    pub value: u64,
    pub observed_value: Option<u64>,
    pub max_overcount: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CounterDumpRecord {
    pub module_name: String,
    pub package_name: Option<String>,
    pub rows: Vec<CounterDumpRow>,
}

pub struct CounterDumpFile {
    mmap: Mmap,
}

#[derive(Clone, Copy)]
pub struct CounterDumpRecordView<'a> {
    header: &'a CounterDumpRecordHeader,
    string_offsets: &'a [u32],
    string_bytes: &'a [u8],
    counter_id: &'a [u32],
    scope: &'a [u32],
    kind: &'a [u32],
    site_kind: &'a [u32],
    function_id: &'a [u64],
    current_function_id: &'a [u64],
    instr_block_label: &'a [u32],
    instr_index_in_block: &'a [u32],
    function_qualname: &'a [u32],
    block_label: &'a [u32],
    value: &'a [u64],
    observed_value: &'a [u64],
    max_overcount: &'a [u64],
}

pub struct CounterDumpRowView<'a> {
    pub counter_id: u32,
    pub scope: &'a str,
    pub kind: &'a str,
    pub site_kind: &'a str,
    pub function_id: Option<FunctionId>,
    pub current_function_id: Option<FunctionId>,
    pub instr_id: Option<InstrId>,
    pub function_qualname: Option<&'a str>,
    pub block_label: Option<&'a str>,
    pub value: u64,
    pub observed_value: Option<u64>,
    pub max_overcount: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CallTargetSpecializationEntry {
    module_name: String,
    site_function_id: FunctionId,
    instr_id: InstrId,
    observed_function_id: FunctionId,
}

#[derive(Default)]
struct StringTable {
    ids: HashMap<String, u32>,
    strings: Vec<String>,
}

impl StringTable {
    fn intern(&mut self, value: &str) -> Result<u32, String> {
        if let Some(id) = self.ids.get(value).copied() {
            return Ok(id);
        }
        let id = u32::try_from(self.strings.len())
            .map_err(|_| "counter dump string table exceeds u32 capacity".to_string())?;
        let owned = value.to_string();
        self.ids.insert(owned.clone(), id);
        self.strings.push(owned);
        Ok(id)
    }
}

impl CounterDumpRecord {
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let row_count = u32::try_from(self.rows.len())
            .map_err(|_| "counter dump row count exceeds u32 capacity".to_string())?;
        let mut strings = StringTable::default();
        let module_name_string_id = strings.intern(self.module_name.as_str())?;
        let package_name_string_id = match self.package_name.as_deref() {
            Some(package_name) if !package_name.is_empty() => strings.intern(package_name)?,
            _ => COUNTER_DUMP_NONE_U32,
        };

        let mut counter_id = Vec::with_capacity(self.rows.len());
        let mut scope = Vec::with_capacity(self.rows.len());
        let mut kind = Vec::with_capacity(self.rows.len());
        let mut site_kind = Vec::with_capacity(self.rows.len());
        let mut function_id = Vec::with_capacity(self.rows.len());
        let mut current_function_id = Vec::with_capacity(self.rows.len());
        let mut instr_block_label = Vec::with_capacity(self.rows.len());
        let mut instr_index_in_block = Vec::with_capacity(self.rows.len());
        let mut function_qualname = Vec::with_capacity(self.rows.len());
        let mut block_label = Vec::with_capacity(self.rows.len());
        let mut value = Vec::with_capacity(self.rows.len());
        let mut observed_value = Vec::with_capacity(self.rows.len());
        let mut max_overcount = Vec::with_capacity(self.rows.len());

        for row in &self.rows {
            counter_id.push(row.counter_id);
            scope.push(strings.intern(row.scope.as_str())?);
            kind.push(strings.intern(row.kind.as_str())?);
            site_kind.push(strings.intern(row.site_kind.as_str())?);
            function_id.push(
                row.function_id
                    .map(FunctionId::packed)
                    .unwrap_or(COUNTER_DUMP_NONE_FUNCTION_ID),
            );
            current_function_id.push(
                row.current_function_id
                    .map(FunctionId::packed)
                    .unwrap_or(COUNTER_DUMP_NONE_FUNCTION_ID),
            );
            instr_block_label.push(
                row.instr_id
                    .map(|instr_id| instr_id.block_label().as_u32())
                    .unwrap_or(COUNTER_DUMP_NONE_U32),
            );
            instr_index_in_block.push(
                row.instr_id
                    .map(|instr_id| instr_id.instr_index_in_block())
                    .unwrap_or(COUNTER_DUMP_NONE_U32),
            );
            function_qualname.push(match row.function_qualname.as_deref() {
                Some(qualname) => strings.intern(qualname)?,
                None => COUNTER_DUMP_NONE_U32,
            });
            block_label.push(match row.block_label.as_deref() {
                Some(block) => strings.intern(block)?,
                None => COUNTER_DUMP_NONE_U32,
            });
            value.push(row.value);
            observed_value.push(row.observed_value.unwrap_or(COUNTER_DUMP_NONE_U64));
            max_overcount.push(row.max_overcount.unwrap_or(COUNTER_DUMP_NONE_U64));
        }

        let string_count = u32::try_from(strings.strings.len())
            .map_err(|_| "counter dump string count exceeds u32 capacity".to_string())?;
        let mut string_offsets = Vec::with_capacity(strings.strings.len() + 1);
        let mut string_bytes = Vec::new();
        string_offsets.push(0u32);
        for string in &strings.strings {
            string_bytes.extend_from_slice(string.as_bytes());
            string_offsets.push(
                u32::try_from(string_bytes.len())
                    .map_err(|_| "counter dump string bytes exceed u32 capacity".to_string())?,
            );
        }

        let header_size = size_of::<CounterDumpRecordHeader>();
        let string_offsets_offset = align_up(header_size, 4);
        let string_bytes_offset = string_offsets_offset + string_offsets.len() * size_of::<u32>();
        let counter_id_offset = align_up(string_bytes_offset + string_bytes.len(), 4);
        let scope_offset = counter_id_offset + counter_id.len() * size_of::<u32>();
        let kind_offset = scope_offset + scope.len() * size_of::<u32>();
        let site_kind_offset = kind_offset + kind.len() * size_of::<u32>();
        let function_id_offset = align_up(site_kind_offset + site_kind.len() * size_of::<u32>(), 8);
        let current_function_id_offset = function_id_offset + function_id.len() * size_of::<u64>();
        let instr_block_label_offset =
            current_function_id_offset + current_function_id.len() * size_of::<u64>();
        let instr_index_in_block_offset =
            instr_block_label_offset + instr_block_label.len() * size_of::<u32>();
        let function_qualname_offset =
            instr_index_in_block_offset + instr_index_in_block.len() * size_of::<u32>();
        let block_label_offset =
            function_qualname_offset + function_qualname.len() * size_of::<u32>();
        let value_offset = align_up(block_label_offset + block_label.len() * size_of::<u32>(), 8);
        let observed_value_offset = value_offset + value.len() * size_of::<u64>();
        let max_overcount_offset = observed_value_offset + observed_value.len() * size_of::<u64>();
        let record_len = align_up(max_overcount_offset + max_overcount.len() * size_of::<u64>(), 8);

        let header = CounterDumpRecordHeader {
            magic: COUNTER_DUMP_MAGIC,
            version: COUNTER_DUMP_VERSION,
            header_size: u16::try_from(header_size)
                .map_err(|_| "counter dump header size exceeds u16 capacity".to_string())?,
            record_len: u32::try_from(record_len)
                .map_err(|_| "counter dump record length exceeds u32 capacity".to_string())?,
            row_count,
            string_count,
            string_bytes_len: u32::try_from(string_bytes.len())
                .map_err(|_| "counter dump string bytes exceed u32 capacity".to_string())?,
            module_name_string_id,
            package_name_string_id,
            string_offsets_offset: u32::try_from(string_offsets_offset)
                .map_err(|_| "counter dump offset exceeds u32 capacity".to_string())?,
            string_bytes_offset: u32::try_from(string_bytes_offset)
                .map_err(|_| "counter dump offset exceeds u32 capacity".to_string())?,
            counter_id_offset: u32::try_from(counter_id_offset)
                .map_err(|_| "counter dump offset exceeds u32 capacity".to_string())?,
            scope_offset: u32::try_from(scope_offset)
                .map_err(|_| "counter dump offset exceeds u32 capacity".to_string())?,
            kind_offset: u32::try_from(kind_offset)
                .map_err(|_| "counter dump offset exceeds u32 capacity".to_string())?,
            site_kind_offset: u32::try_from(site_kind_offset)
                .map_err(|_| "counter dump offset exceeds u32 capacity".to_string())?,
            function_id_offset: u32::try_from(function_id_offset)
                .map_err(|_| "counter dump offset exceeds u32 capacity".to_string())?,
            current_function_id_offset: u32::try_from(current_function_id_offset)
                .map_err(|_| "counter dump offset exceeds u32 capacity".to_string())?,
            instr_block_label_offset: u32::try_from(instr_block_label_offset)
                .map_err(|_| "counter dump offset exceeds u32 capacity".to_string())?,
            instr_index_in_block_offset: u32::try_from(instr_index_in_block_offset)
                .map_err(|_| "counter dump offset exceeds u32 capacity".to_string())?,
            function_qualname_offset: u32::try_from(function_qualname_offset)
                .map_err(|_| "counter dump offset exceeds u32 capacity".to_string())?,
            block_label_offset: u32::try_from(block_label_offset)
                .map_err(|_| "counter dump offset exceeds u32 capacity".to_string())?,
            value_offset: u32::try_from(value_offset)
                .map_err(|_| "counter dump offset exceeds u32 capacity".to_string())?,
            observed_value_offset: u32::try_from(observed_value_offset)
                .map_err(|_| "counter dump offset exceeds u32 capacity".to_string())?,
            max_overcount_offset: u32::try_from(max_overcount_offset)
                .map_err(|_| "counter dump offset exceeds u32 capacity".to_string())?,
        };

        let mut bytes = vec![0u8; record_len];
        write_bytes(&mut bytes, 0, bytes_of(&header))?;
        write_bytes(
            &mut bytes,
            string_offsets_offset,
            bytes_of_slice(string_offsets.as_slice()),
        )?;
        write_bytes(&mut bytes, string_bytes_offset, string_bytes.as_slice())?;
        write_bytes(
            &mut bytes,
            counter_id_offset,
            bytes_of_slice(counter_id.as_slice()),
        )?;
        write_bytes(&mut bytes, scope_offset, bytes_of_slice(scope.as_slice()))?;
        write_bytes(&mut bytes, kind_offset, bytes_of_slice(kind.as_slice()))?;
        write_bytes(
            &mut bytes,
            site_kind_offset,
            bytes_of_slice(site_kind.as_slice()),
        )?;
        write_bytes(
            &mut bytes,
            function_id_offset,
            bytes_of_slice(function_id.as_slice()),
        )?;
        write_bytes(
            &mut bytes,
            current_function_id_offset,
            bytes_of_slice(current_function_id.as_slice()),
        )?;
        write_bytes(
            &mut bytes,
            instr_block_label_offset,
            bytes_of_slice(instr_block_label.as_slice()),
        )?;
        write_bytes(
            &mut bytes,
            instr_index_in_block_offset,
            bytes_of_slice(instr_index_in_block.as_slice()),
        )?;
        write_bytes(
            &mut bytes,
            function_qualname_offset,
            bytes_of_slice(function_qualname.as_slice()),
        )?;
        write_bytes(
            &mut bytes,
            block_label_offset,
            bytes_of_slice(block_label.as_slice()),
        )?;
        write_bytes(&mut bytes, value_offset, bytes_of_slice(value.as_slice()))?;
        write_bytes(
            &mut bytes,
            observed_value_offset,
            bytes_of_slice(observed_value.as_slice()),
        )?;
        write_bytes(
            &mut bytes,
            max_overcount_offset,
            bytes_of_slice(max_overcount.as_slice()),
        )?;
        Ok(bytes)
    }
}

impl CounterDumpFile {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file =
            File::open(path).map_err(|err| format!("failed to open {}: {err}", path.display()))?;
        let mmap = unsafe { Mmap::map(&file) }
            .map_err(|err| format!("failed to map {}: {err}", path.display()))?;
        Ok(Self { mmap })
    }

    pub fn records(&self) -> Result<Vec<CounterDumpRecordView<'_>>, String> {
        parse_counter_dump_records(self.mmap.as_ref())
    }
}

impl<'a> CounterDumpRecordView<'a> {
    pub fn module_name(&self) -> Result<&'a str, String> {
        self.resolve_string_id(self.header.module_name_string_id)
    }

    pub fn package_name(&self) -> Result<Option<&'a str>, String> {
        if self.header.package_name_string_id == COUNTER_DUMP_NONE_U32 {
            Ok(None)
        } else {
            self.resolve_string_id(self.header.package_name_string_id)
                .map(Some)
        }
    }

    pub fn row_count(&self) -> usize {
        self.counter_id.len()
    }

    pub fn row(&self, index: usize) -> Result<CounterDumpRowView<'a>, String> {
        if index >= self.row_count() {
            return Err(format!(
                "counter dump row {index} is out of bounds for {} rows",
                self.row_count()
            ));
        }
        Ok(CounterDumpRowView {
            counter_id: self.counter_id[index],
            scope: self.resolve_string_id(self.scope[index])?,
            kind: self.resolve_string_id(self.kind[index])?,
            site_kind: self.resolve_string_id(self.site_kind[index])?,
            function_id: (self.function_id[index] != COUNTER_DUMP_NONE_FUNCTION_ID)
                .then_some(FunctionId::from_packed(self.function_id[index])),
            current_function_id: (self.current_function_id[index] != COUNTER_DUMP_NONE_FUNCTION_ID)
                .then_some(FunctionId::from_packed(self.current_function_id[index])),
            instr_id: if self.instr_block_label[index] == COUNTER_DUMP_NONE_U32
                || self.instr_index_in_block[index] == COUNTER_DUMP_NONE_U32
            {
                None
            } else {
                Some(InstrId::new(
                    BlockLabel::from_index(self.instr_block_label[index] as usize),
                    self.instr_index_in_block[index],
                ))
            },
            function_qualname: self.resolve_optional_string_id(self.function_qualname[index])?,
            block_label: self.resolve_optional_string_id(self.block_label[index])?,
            value: self.value[index],
            observed_value: (self.observed_value[index] != COUNTER_DUMP_NONE_U64)
                .then_some(self.observed_value[index]),
            max_overcount: (self.max_overcount[index] != COUNTER_DUMP_NONE_U64)
                .then_some(self.max_overcount[index]),
        })
    }

    fn resolve_optional_string_id(&self, string_id: u32) -> Result<Option<&'a str>, String> {
        if string_id == COUNTER_DUMP_NONE_U32 {
            Ok(None)
        } else {
            self.resolve_string_id(string_id).map(Some)
        }
    }

    fn resolve_string_id(&self, string_id: u32) -> Result<&'a str, String> {
        let string_index = usize::try_from(string_id)
            .map_err(|_| format!("string id {string_id} does not fit in usize"))?;
        let Some(start) = self.string_offsets.get(string_index).copied() else {
            return Err(format!(
                "string id {string_id} is out of bounds for {} strings",
                self.string_offsets.len().saturating_sub(1)
            ));
        };
        let Some(end) = self.string_offsets.get(string_index + 1).copied() else {
            return Err(format!(
                "string id {string_id} is missing its terminal offset"
            ));
        };
        let start = usize::try_from(start)
            .map_err(|_| format!("string start offset {start} does not fit in usize"))?;
        let end = usize::try_from(end)
            .map_err(|_| format!("string end offset {end} does not fit in usize"))?;
        let Some(bytes) = self.string_bytes.get(start..end) else {
            return Err(format!(
                "string id {string_id} range {start}..{end} is out of bounds"
            ));
        };
        std::str::from_utf8(bytes)
            .map_err(|err| format!("counter dump string id {string_id} is not utf-8: {err}"))
    }
}

pub fn parse_counter_dump_records(bytes: &[u8]) -> Result<Vec<CounterDumpRecordView<'_>>, String> {
    let mut records = Vec::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        let header = unsafe { cast_ref::<CounterDumpRecordHeader>(remaining, 0) }?;
        if header.magic != COUNTER_DUMP_MAGIC {
            return Err(format!(
                "counter dump record at byte offset {offset} has invalid magic {:?}",
                header.magic
            ));
        }
        if header.version != COUNTER_DUMP_VERSION {
            return Err(format!(
                "counter dump record at byte offset {offset} uses unsupported version {}",
                header.version
            ));
        }
        if usize::from(header.header_size) != size_of::<CounterDumpRecordHeader>() {
            return Err(format!(
                "counter dump record at byte offset {offset} has unexpected header size {}",
                header.header_size
            ));
        }

        let record_len = usize::try_from(header.record_len)
            .map_err(|_| format!("counter dump record at byte offset {offset} is too large"))?;
        if record_len == 0 || record_len % 8 != 0 {
            return Err(format!(
                "counter dump record at byte offset {offset} has invalid length {record_len}"
            ));
        }
        let Some(record_bytes) = remaining.get(..record_len) else {
            return Err(format!(
                "counter dump record at byte offset {offset} extends past end of file"
            ));
        };

        let row_count = usize::try_from(header.row_count)
            .map_err(|_| format!("counter dump row count at byte offset {offset} is too large"))?;
        let string_count = usize::try_from(header.string_count).map_err(|_| {
            format!("counter dump string count at byte offset {offset} is too large")
        })?;

        let string_offsets_offset =
            usize::try_from(header.string_offsets_offset).map_err(|_| {
                format!("counter dump string offsets offset at byte offset {offset} is too large")
            })?;
        let string_bytes_offset = usize::try_from(header.string_bytes_offset).map_err(|_| {
            format!("counter dump string bytes offset at byte offset {offset} is too large")
        })?;
        let string_bytes_len = usize::try_from(header.string_bytes_len).map_err(|_| {
            format!("counter dump string byte length at byte offset {offset} is too large")
        })?;
        let counter_id_offset = usize::try_from(header.counter_id_offset).map_err(|_| {
            format!("counter dump counter_id offset at byte offset {offset} is too large")
        })?;
        let scope_offset = usize::try_from(header.scope_offset).map_err(|_| {
            format!("counter dump scope offset at byte offset {offset} is too large")
        })?;
        let kind_offset = usize::try_from(header.kind_offset).map_err(|_| {
            format!("counter dump kind offset at byte offset {offset} is too large")
        })?;
        let site_kind_offset = usize::try_from(header.site_kind_offset).map_err(|_| {
            format!("counter dump site_kind offset at byte offset {offset} is too large")
        })?;
        let function_id_offset = usize::try_from(header.function_id_offset).map_err(|_| {
            format!("counter dump function_id offset at byte offset {offset} is too large")
        })?;
        let current_function_id_offset = usize::try_from(header.current_function_id_offset)
            .map_err(|_| {
                format!(
                    "counter dump current_function_id offset at byte offset {offset} is too large"
                )
            })?;
        let instr_block_label_offset =
            usize::try_from(header.instr_block_label_offset).map_err(|_| {
                format!(
                    "counter dump instr_block_label offset at byte offset {offset} is too large"
                )
            })?;
        let instr_index_in_block_offset = usize::try_from(header.instr_index_in_block_offset)
            .map_err(|_| {
                format!(
                    "counter dump instr_index_in_block offset at byte offset {offset} is too large"
                )
            })?;
        let function_qualname_offset =
            usize::try_from(header.function_qualname_offset).map_err(|_| {
                format!(
                    "counter dump function_qualname offset at byte offset {offset} is too large"
                )
            })?;
        let block_label_offset = usize::try_from(header.block_label_offset).map_err(|_| {
            format!("counter dump block_label offset at byte offset {offset} is too large")
        })?;
        let value_offset = usize::try_from(header.value_offset).map_err(|_| {
            format!("counter dump value offset at byte offset {offset} is too large")
        })?;
        let observed_value_offset = usize::try_from(header.observed_value_offset).map_err(|_| {
            format!("counter dump observed_value offset at byte offset {offset} is too large")
        })?;
        let max_overcount_offset = usize::try_from(header.max_overcount_offset).map_err(|_| {
            format!("counter dump max_overcount offset at byte offset {offset} is too large")
        })?;

        if !is_nondecreasing(&[
            usize::from(header.header_size),
            string_offsets_offset,
            string_bytes_offset,
            counter_id_offset,
            scope_offset,
            kind_offset,
            site_kind_offset,
            function_id_offset,
            current_function_id_offset,
            instr_block_label_offset,
            instr_index_in_block_offset,
            function_qualname_offset,
            block_label_offset,
            value_offset,
            observed_value_offset,
            max_overcount_offset,
            record_len,
        ]) {
            return Err(format!(
                "counter dump record at byte offset {offset} has overlapping sections"
            ));
        }

        let string_offsets =
            unsafe { cast_slice::<u32>(record_bytes, string_offsets_offset, string_count + 1) }?;
        let Some(string_bytes) =
            record_bytes.get(string_bytes_offset..string_bytes_offset + string_bytes_len)
        else {
            return Err(format!(
                "counter dump string bytes at byte offset {offset} are out of bounds"
            ));
        };
        let counter_id = unsafe { cast_slice::<u32>(record_bytes, counter_id_offset, row_count) }?;
        let scope = unsafe { cast_slice::<u32>(record_bytes, scope_offset, row_count) }?;
        let kind = unsafe { cast_slice::<u32>(record_bytes, kind_offset, row_count) }?;
        let site_kind = unsafe { cast_slice::<u32>(record_bytes, site_kind_offset, row_count) }?;
        let function_id =
            unsafe { cast_slice::<u64>(record_bytes, function_id_offset, row_count) }?;
        let current_function_id =
            unsafe { cast_slice::<u64>(record_bytes, current_function_id_offset, row_count) }?;
        let instr_block_label =
            unsafe { cast_slice::<u32>(record_bytes, instr_block_label_offset, row_count) }?;
        let instr_index_in_block =
            unsafe { cast_slice::<u32>(record_bytes, instr_index_in_block_offset, row_count) }?;
        let function_qualname =
            unsafe { cast_slice::<u32>(record_bytes, function_qualname_offset, row_count) }?;
        let block_label =
            unsafe { cast_slice::<u32>(record_bytes, block_label_offset, row_count) }?;
        let value = unsafe { cast_slice::<u64>(record_bytes, value_offset, row_count) }?;
        let observed_value =
            unsafe { cast_slice::<u64>(record_bytes, observed_value_offset, row_count) }?;
        let max_overcount =
            unsafe { cast_slice::<u64>(record_bytes, max_overcount_offset, row_count) }?;

        if string_offsets.first().copied().unwrap_or(0) != 0 {
            return Err(format!(
                "counter dump record at byte offset {offset} has a non-zero first string offset"
            ));
        }
        let total_string_bytes = u32::try_from(string_bytes.len()).map_err(|_| {
            format!("counter dump string bytes at byte offset {offset} exceed u32 capacity")
        })?;
        if string_offsets.last().copied().unwrap_or(0) != total_string_bytes {
            return Err(format!(
                "counter dump record at byte offset {offset} has mismatched string byte length"
            ));
        }

        records.push(CounterDumpRecordView {
            header,
            string_offsets,
            string_bytes,
            counter_id,
            scope,
            kind,
            site_kind,
            function_id,
            current_function_id,
            instr_block_label,
            instr_index_in_block,
            function_qualname,
            block_label,
            value,
            observed_value,
            max_overcount,
        });
        offset += record_len;
    }
    Ok(records)
}

fn call_target_specialization_entries(
    records: &[CounterDumpRecordView<'_>],
) -> Result<Vec<CallTargetSpecializationEntry>, String> {
    let mut entries = Vec::new();
    for record in records {
        let module_name = record.module_name()?.to_string();
        for row_index in 0..record.row_count() {
            let row = record.row(row_index)?;
            if row.kind != "call_hot_targets" {
                continue;
            }
            let Some(site_function_id) = row.function_id else {
                continue;
            };
            let Some(instr_id) = row.instr_id else {
                continue;
            };
            let Some(observed_value) = row.observed_value else {
                continue;
            };
            if observed_value == 0 {
                continue;
            }
            let observed_function_id = FunctionId::from_packed(observed_value);
            if observed_function_id == FunctionId::global() {
                continue;
            }
            entries.push(CallTargetSpecializationEntry {
                module_name: module_name.clone(),
                site_function_id,
                instr_id,
                observed_function_id,
            });
        }
    }
    Ok(entries)
}

pub fn render_call_target_specializations(
    records: &[CounterDumpRecordView<'_>],
) -> Result<String, String> {
    let mut ordered_keys = Vec::new();
    let mut seen_targets = HashSet::new();
    let mut targets = HashMap::<String, Vec<u64>>::new();
    for entry in call_target_specialization_entries(records)? {
        let key = format!(
            "{}|{}|{}|{}",
            entry.module_name,
            entry.site_function_id.packed(),
            entry.instr_id.block_label().as_u32(),
            entry.instr_id.instr_index_in_block(),
        );
        let target_key = format!("{key}|{}", entry.observed_function_id.packed());
        if seen_targets.insert(target_key) {
            if !targets.contains_key(&key) {
                ordered_keys.push(key.clone());
            }
            targets
                .entry(key)
                .or_default()
                .push(entry.observed_function_id.packed());
        }
    }
    let mut out = String::new();
    for (index, key) in ordered_keys.iter().enumerate() {
        if index > 0 {
            out.push(';');
        }
        out.push_str(key);
        out.push('=');
        let Some(values) = targets.get(key) else {
            return Err(format!("missing specialization targets for key {key}"));
        };
        for (value_index, value) in values.iter().enumerate() {
            if value_index > 0 {
                out.push(',');
            }
            out.push_str(&value.to_string());
        }
    }
    Ok(out)
}

pub fn collect_call_target_specializations_for_function(
    records: &[CounterDumpRecordView<'_>],
    module_name: &str,
    function_id: FunctionId,
) -> Result<HashMap<InstrId, Vec<FunctionId>>, String> {
    let mut out = HashMap::<InstrId, Vec<FunctionId>>::new();
    let mut seen_targets = HashSet::<(InstrId, FunctionId)>::new();
    for entry in call_target_specialization_entries(records)? {
        if entry.module_name != module_name || entry.site_function_id != function_id {
            continue;
        }
        if seen_targets.insert((entry.instr_id, entry.observed_function_id)) {
            out.entry(entry.instr_id)
                .or_default()
                .push(entry.observed_function_id);
        }
    }
    Ok(out)
}

pub fn read_call_target_specializations_from_file(
    path: &Path,
    module_name: &str,
    function_id: FunctionId,
) -> Result<HashMap<InstrId, Vec<FunctionId>>, String> {
    let dump = CounterDumpFile::open(path)?;
    let records = dump.records()?;
    collect_call_target_specializations_for_function(records.as_slice(), module_name, function_id)
}

fn align_up(offset: usize, align: usize) -> usize {
    let remainder = offset % align;
    if remainder == 0 {
        offset
    } else {
        offset + (align - remainder)
    }
}

fn write_bytes(dst: &mut [u8], offset: usize, src: &[u8]) -> Result<(), String> {
    let end = offset
        .checked_add(src.len())
        .ok_or_else(|| "counter dump byte range overflowed".to_string())?;
    let Some(target) = dst.get_mut(offset..end) else {
        return Err("counter dump byte range is out of bounds".to_string());
    };
    target.copy_from_slice(src);
    Ok(())
}

fn bytes_of<T>(value: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>()) }
}

fn bytes_of_slice<T>(values: &[T]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), std::mem::size_of_val(values))
    }
}

fn is_nondecreasing(values: &[usize]) -> bool {
    values.windows(2).all(|window| window[0] <= window[1])
}

unsafe fn cast_ref<'a, T>(bytes: &'a [u8], offset: usize) -> Result<&'a T, String> {
    let Some(tail) = bytes.get(offset..) else {
        return Err(format!("counter dump offset {offset} is out of bounds"));
    };
    if tail.len() < size_of::<T>() {
        return Err(format!(
            "counter dump tail at offset {offset} is too short for {} bytes",
            size_of::<T>()
        ));
    }
    let ptr = tail.as_ptr();
    if !(ptr as usize).is_multiple_of(align_of::<T>()) {
        return Err(format!(
            "counter dump offset {offset} is not aligned for {}-byte values",
            align_of::<T>()
        ));
    }
    Ok(unsafe { &*ptr.cast::<T>() })
}

unsafe fn cast_slice<'a, T>(bytes: &'a [u8], offset: usize, len: usize) -> Result<&'a [T], String> {
    let byte_len = len
        .checked_mul(size_of::<T>())
        .ok_or_else(|| "counter dump slice length overflowed".to_string())?;
    let Some(slice_bytes) = bytes.get(offset..offset + byte_len) else {
        return Err(format!(
            "counter dump slice at offset {offset} with len {len} is out of bounds"
        ));
    };
    if !(slice_bytes.as_ptr() as usize).is_multiple_of(align_of::<T>()) {
        return Err(format!(
            "counter dump slice at offset {offset} is not aligned for {}-byte values",
            align_of::<T>()
        ));
    }
    Ok(unsafe { std::slice::from_raw_parts(slice_bytes.as_ptr().cast::<T>(), len) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_blockpy::block_py::BlockLabel;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_COUNTER_DUMP_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    fn temp_path() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "soac_counter_dump_{nonce}_{}_{}.bin",
            std::process::id(),
            NEXT_COUNTER_DUMP_TEST_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn read_u16(bytes: &[u8], offset: usize) -> u16 {
        let raw = bytes[offset..offset + 2]
            .try_into()
            .expect("u16 slice should have exact width");
        u16::from_le_bytes(raw)
    }

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        let raw = bytes[offset..offset + 4]
            .try_into()
            .expect("u32 slice should have exact width");
        u32::from_le_bytes(raw)
    }

    fn read_u64(bytes: &[u8], offset: usize) -> u64 {
        let raw = bytes[offset..offset + 8]
            .try_into()
            .expect("u64 slice should have exact width");
        u64::from_le_bytes(raw)
    }

    #[test]
    fn encodes_columnar_record_layout() {
        let record = CounterDumpRecord {
            module_name: "counter_test".to_string(),
            package_name: Some("pkg".to_string()),
            rows: vec![
                CounterDumpRow {
                    counter_id: 3,
                    scope: "this".to_string(),
                    kind: "block_entry".to_string(),
                    site_kind: "block_entry".to_string(),
                    function_id: Some(FunctionId::new(1, 7)),
                    current_function_id: Some(FunctionId::new(1, 7)),
                    instr_id: None,
                    function_qualname: Some("f".to_string()),
                    block_label: Some("bb0".to_string()),
                    value: 11,
                    observed_value: None,
                    max_overcount: None,
                },
                CounterDumpRow {
                    counter_id: 4,
                    scope: "global".to_string(),
                    kind: "runtime_incref".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(FunctionId::global()),
                    current_function_id: Some(FunctionId::global()),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(5), 3)),
                    function_qualname: None,
                    block_label: None,
                    value: 19,
                    observed_value: Some(42),
                    max_overcount: Some(7),
                },
            ],
        };

        let bytes = record.encode().expect("counter dump should encode");
        assert_eq!(&bytes[..8], COUNTER_DUMP_MAGIC.as_slice());
        assert_eq!(read_u16(&bytes, 8), COUNTER_DUMP_VERSION);
        let header_size = usize::from(read_u16(&bytes, 10));
        let record_len = read_u32(&bytes, 12) as usize;
        let row_count = read_u32(&bytes, 16);
        let string_count = read_u32(&bytes, 20);
        let string_bytes_len = read_u32(&bytes, 24) as usize;
        let string_offsets_offset = read_u32(&bytes, 36) as usize;
        let string_bytes_offset = read_u32(&bytes, 40) as usize;
        let counter_id_offset = read_u32(&bytes, 44) as usize;
        let scope_offset = read_u32(&bytes, 48) as usize;
        let kind_offset = read_u32(&bytes, 52) as usize;
        let site_kind_offset = read_u32(&bytes, 56) as usize;
        let function_id_offset = read_u32(&bytes, 60) as usize;
        let current_function_id_offset = read_u32(&bytes, 64) as usize;
        let instr_block_label_offset = read_u32(&bytes, 68) as usize;
        let instr_index_in_block_offset = read_u32(&bytes, 72) as usize;
        let function_qualname_offset = read_u32(&bytes, 76) as usize;
        let block_label_offset = read_u32(&bytes, 80) as usize;
        let value_offset = read_u32(&bytes, 84) as usize;
        let observed_value_offset = read_u32(&bytes, 88) as usize;
        let max_overcount_offset = read_u32(&bytes, 92) as usize;

        assert_eq!(header_size, size_of::<CounterDumpRecordHeader>());
        assert_eq!(record_len, bytes.len());
        assert_eq!(row_count, 2);
        assert!(string_count >= 7);
        assert!(counter_id_offset >= string_bytes_offset);
        assert!(scope_offset > counter_id_offset);
        assert!(kind_offset > scope_offset);
        assert!(site_kind_offset > kind_offset);
        assert!(function_id_offset > site_kind_offset);
        assert!(current_function_id_offset > function_id_offset);
        assert!(instr_block_label_offset > current_function_id_offset);
        assert!(instr_index_in_block_offset > instr_block_label_offset);
        assert!(function_qualname_offset > instr_index_in_block_offset);
        assert!(block_label_offset > function_qualname_offset);
        assert!(value_offset > block_label_offset);
        assert!(observed_value_offset > value_offset);
        assert!(max_overcount_offset > observed_value_offset);
        assert_eq!(value_offset % 8, 0);

        let string_offsets_len = (string_count as usize + 1) * size_of::<u32>();
        let first_string_start = string_bytes_offset;
        let first_string_end = first_string_start
            + read_u32(&bytes, string_offsets_offset + size_of::<u32>()) as usize;
        let first_string = std::str::from_utf8(&bytes[first_string_start..first_string_end])
            .expect("module name should be utf-8");
        assert_eq!(first_string, "counter_test");

        assert_eq!(read_u32(&bytes, counter_id_offset), 3);
        assert_eq!(read_u32(&bytes, counter_id_offset + 4), 4);
        assert_eq!(
            read_u64(&bytes, function_id_offset),
            FunctionId::new(1, 7).packed()
        );
        assert_eq!(
            read_u64(&bytes, function_id_offset + 8),
            FunctionId::global().packed()
        );
        assert_eq!(
            read_u64(&bytes, current_function_id_offset),
            FunctionId::new(1, 7).packed()
        );
        assert_eq!(
            read_u64(&bytes, current_function_id_offset + 8),
            FunctionId::global().packed()
        );
        assert_eq!(
            read_u32(&bytes, instr_block_label_offset),
            COUNTER_DUMP_NONE_U32
        );
        assert_eq!(
            read_u32(&bytes, instr_block_label_offset + 4),
            BlockLabel::from_index(5).as_u32()
        );
        assert_eq!(
            read_u32(&bytes, instr_index_in_block_offset),
            COUNTER_DUMP_NONE_U32
        );
        assert_eq!(read_u32(&bytes, instr_index_in_block_offset + 4), 3);
        assert_eq!(read_u64(&bytes, value_offset), 11);
        assert_eq!(read_u64(&bytes, value_offset + 8), 19);
        assert_eq!(read_u64(&bytes, observed_value_offset), COUNTER_DUMP_NONE_U64);
        assert_eq!(read_u64(&bytes, observed_value_offset + 8), 42);
        assert_eq!(read_u64(&bytes, max_overcount_offset), COUNTER_DUMP_NONE_U64);
        assert_eq!(read_u64(&bytes, max_overcount_offset + 8), 7);

        let string_offsets_end = string_offsets_offset + string_offsets_len;
        assert!(string_bytes_offset + string_bytes_len <= counter_id_offset);
        assert!(string_offsets_end <= string_bytes_offset);
    }

    #[test]
    fn parses_appended_counter_dump_records_from_mmap() {
        let first = CounterDumpRecord {
            module_name: "alpha".to_string(),
            package_name: Some("pkg".to_string()),
            rows: vec![CounterDumpRow {
                counter_id: 1,
                scope: "this".to_string(),
                kind: "block_entry".to_string(),
                site_kind: "block_entry".to_string(),
                function_id: Some(FunctionId::new(1, 7)),
                current_function_id: Some(FunctionId::new(1, 7)),
                instr_id: None,
                function_qualname: Some("f".to_string()),
                block_label: Some("bb0".to_string()),
                value: 5,
                observed_value: None,
                max_overcount: None,
            }],
        };
        let second = CounterDumpRecord {
            module_name: "beta".to_string(),
            package_name: None,
            rows: vec![CounterDumpRow {
                counter_id: 3,
                scope: "global".to_string(),
                kind: "runtime_incref".to_string(),
                site_kind: "runtime".to_string(),
                function_id: Some(FunctionId::global()),
                current_function_id: Some(FunctionId::global()),
                instr_id: Some(InstrId::new(BlockLabel::from_index(5), 3)),
                function_qualname: None,
                block_label: None,
                value: 11,
                observed_value: Some(7),
                max_overcount: Some(2),
            }],
        };

        let path = temp_path();
        let mut bytes = first.encode().expect("first record should encode");
        bytes.extend_from_slice(
            second
                .encode()
                .expect("second record should encode")
                .as_slice(),
        );
        fs::write(&path, bytes).expect("counter dump file should be writable");

        let dump = CounterDumpFile::open(path.as_path()).expect("counter dump file should map");
        let records = dump.records().expect("mapped counter dump should parse");
        assert_eq!(records.len(), 2);

        let first_record = records[0];
        assert_eq!(first_record.module_name().expect("module name"), "alpha");
        assert_eq!(
            first_record.package_name().expect("package name"),
            Some("pkg")
        );
        let first_row = first_record.row(0).expect("first row should resolve");
        assert_eq!(first_row.counter_id, 1);
        assert_eq!(first_row.scope, "this");
        assert_eq!(first_row.kind, "block_entry");
        assert_eq!(first_row.site_kind, "block_entry");
        assert_eq!(first_row.function_id, Some(FunctionId::new(1, 7)));
        assert_eq!(first_row.current_function_id, Some(FunctionId::new(1, 7)));
        assert_eq!(first_row.instr_id, None);
        assert_eq!(first_row.function_qualname, Some("f"));
        assert_eq!(first_row.block_label, Some("bb0"));
        assert_eq!(first_row.value, 5);

        let second_record = records[1];
        assert_eq!(second_record.module_name().expect("module name"), "beta");
        assert_eq!(second_record.package_name().expect("package name"), None);
        let second_row = second_record.row(0).expect("second row should resolve");
        assert_eq!(second_row.counter_id, 3);
        assert_eq!(second_row.scope, "global");
        assert_eq!(second_row.kind, "runtime_incref");
        assert_eq!(second_row.site_kind, "runtime");
        assert_eq!(second_row.function_id, Some(FunctionId::global()));
        assert_eq!(second_row.current_function_id, Some(FunctionId::global()));
        assert_eq!(
            second_row.instr_id,
            Some(InstrId::new(BlockLabel::from_index(5), 3))
        );
        assert_eq!(second_row.function_qualname, None);
        assert_eq!(second_row.block_label, None);
        assert_eq!(second_row.value, 11);

        fs::remove_file(&path).expect("temp counter dump file should be removable");
    }

    #[test]
    fn renders_and_collects_call_target_specializations() {
        let record = CounterDumpRecord {
            module_name: "mod".to_string(),
            package_name: None,
            rows: vec![
                CounterDumpRow {
                    counter_id: 1,
                    scope: "this".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(FunctionId::new(1, 7)),
                    current_function_id: Some(FunctionId::new(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 11,
                    observed_value: Some(FunctionId::new(1, 9).packed()),
                    max_overcount: Some(1),
                },
                CounterDumpRow {
                    counter_id: 2,
                    scope: "this".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(FunctionId::new(1, 7)),
                    current_function_id: Some(FunctionId::new(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 5,
                    observed_value: Some(FunctionId::new(1, 10).packed()),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 3,
                    scope: "this".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(FunctionId::new(1, 8)),
                    current_function_id: Some(FunctionId::new(1, 8)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(3), 1)),
                    function_qualname: Some("pkg.mod.g".to_string()),
                    block_label: None,
                    value: 4,
                    observed_value: Some(FunctionId::global().packed()),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 4,
                    scope: "this".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(FunctionId::new(1, 8)),
                    current_function_id: Some(FunctionId::new(1, 8)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(3), 2)),
                    function_qualname: Some("pkg.mod.h".to_string()),
                    block_label: None,
                    value: 4,
                    observed_value: Some(0),
                    max_overcount: Some(0),
                },
            ],
        };

        let bytes = record.encode().expect("counter dump should encode");
        let records = parse_counter_dump_records(bytes.as_slice()).expect("counter dump should parse");
        let rendered = render_call_target_specializations(&records)
            .expect("specializations should render");
        assert_eq!(rendered, "mod|4294967303|2|4=4294967305,4294967306");

        let collected = collect_call_target_specializations_for_function(
            &records,
            "mod",
            FunctionId::new(1, 7),
        )
        .expect("specializations should collect");
        assert_eq!(
            collected.get(&InstrId::new(BlockLabel::from_index(2), 4)),
            Some(&vec![FunctionId::new(1, 9), FunctionId::new(1, 10)])
        );
    }
}
