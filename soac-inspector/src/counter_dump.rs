use memmap2::Mmap;
use soac_blockpy::block_py::{BlockLabel, FunctionId, InstrId};
use soac_eval::counter_dump::{
    COUNTER_DUMP_MAGIC, COUNTER_DUMP_NONE_U32, COUNTER_DUMP_NONE_U64, COUNTER_DUMP_VERSION,
    CounterDumpRecordHeader,
};
#[cfg(test)]
use soac_eval::counter_dump::{CounterDumpRecord, CounterDumpRow};
use std::fs::File;
use std::mem::{align_of, size_of};
use std::path::Path;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

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
            function_id: (self.function_id[index] != COUNTER_DUMP_NONE_U64)
                .then_some(FunctionId::from_packed(self.function_id[index])),
            current_function_id: (self.current_function_id[index] != COUNTER_DUMP_NONE_U64)
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
mod test {
    use super::*;
    use std::fs;

    static NEXT_COUNTER_DUMP_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    fn temp_path() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "soac_counter_dump_inspector_{nonce}_{}_{}.bin",
            std::process::id(),
            NEXT_COUNTER_DUMP_TEST_ID.fetch_add(1, Ordering::Relaxed)
        ))
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
}
