use super::CompiledFunctionBytes;
use cranelift_codegen::binemit::Reloc;
use cranelift_codegen::ir;
use cranelift_codegen::isa::TargetIsa;
use cranelift_jit::JITModule;
use cranelift_module::{DataId, FuncId, Module, ModuleRelocTarget};
use gimli::LittleEndian;
use gimli::write::{
    Address as DwarfAddress, EhFrame, EndianVec, FrameTable, RelocateWriter, RelocationTarget,
};
use std::collections::HashMap;

pub(super) struct ObjectFunctionDefinition {
    pub(super) func_id: FuncId,
    pub(super) symbol: String,
    pub(super) binding: ElfSymbolBinding,
    pub(super) bytes: CompiledFunctionBytes,
    pub(super) systemv_unwind_info: Option<cranelift_codegen::isa::unwind::systemv::UnwindInfo>,
}

#[derive(Debug)]
pub(super) struct ObjectDataDefinition {
    pub(super) data_id: DataId,
    pub(super) symbol: String,
    pub(super) binding: ElfSymbolBinding,
    pub(super) bytes: Vec<u8>,
    pub(super) align: u64,
    pub(super) writable: bool,
    pub(super) relocations: Vec<ObjectDataRelocation>,
}

#[derive(Debug)]
pub(super) struct ObjectDataRelocation {
    pub(super) offset: u64,
    pub(super) symbol: String,
    pub(super) kind: ElfSymbolKind,
    pub(super) reloc_type: u32,
    pub(super) addend: i64,
}

struct DwarfFunctionEntry<'a> {
    symbol: &'a str,
    symbol_index: u32,
    text_section_symbol_index: u32,
    text_offset: u64,
    code_size: u64,
    systemv_unwind_info: Option<&'a cranelift_codegen::isa::unwind::systemv::UnwindInfo>,
}

struct DwarfSectionBytes {
    bytes: Vec<u8>,
    relocations: Vec<ElfRelocation>,
}

struct PrecompiledDwarfSections {
    debug_info: DwarfSectionBytes,
    debug_abbrev: Vec<u8>,
    debug_line: DwarfSectionBytes,
    debug_str: Vec<u8>,
}

struct RelocatingDwarfWriter {
    writer: EndianVec<LittleEndian>,
    relocations: Vec<gimli::write::Relocation>,
}

impl RelocatingDwarfWriter {
    fn new() -> Self {
        Self {
            writer: EndianVec::new(LittleEndian),
            relocations: Vec::new(),
        }
    }

    fn finish(self) -> Result<DwarfSectionBytes, String> {
        let mut relocations = Vec::with_capacity(self.relocations.len());
        for relocation in self.relocations {
            let RelocationTarget::Symbol(symbol) = relocation.target else {
                return Err(format!(
                    "unsupported DWARF section relocation target: {:?}",
                    relocation.target
                ));
            };
            let symbol_index = u32::try_from(symbol)
                .map_err(|_| format!("DWARF relocation symbol index is too large: {symbol}"))?;
            let offset = u64::try_from(relocation.offset).map_err(|_| {
                format!(
                    "DWARF relocation offset is too large: {}",
                    relocation.offset
                )
            })?;
            relocations.push(ElfRelocation {
                offset,
                symbol_index,
                reloc_type: elf_relocation_type_for_dwarf(relocation.size, relocation.eh_pe)?,
                addend: relocation.addend,
            });
        }
        Ok(DwarfSectionBytes {
            bytes: self.writer.into_vec(),
            relocations,
        })
    }
}

impl RelocateWriter for RelocatingDwarfWriter {
    type Writer = EndianVec<LittleEndian>;

    fn writer(&self) -> &Self::Writer {
        &self.writer
    }

    fn writer_mut(&mut self) -> &mut Self::Writer {
        &mut self.writer
    }

    fn relocate(&mut self, relocation: gimli::write::Relocation) {
        self.relocations.push(relocation);
    }
}

fn build_eh_frame_section(
    isa: &dyn TargetIsa,
    functions: &[DwarfFunctionEntry<'_>],
) -> Result<Option<DwarfSectionBytes>, String> {
    let Some(mut cie) = isa.create_systemv_cie() else {
        return Ok(None);
    };
    cie.fde_address_encoding = gimli::constants::DW_EH_PE_pcrel | gimli::constants::DW_EH_PE_sdata4;

    let mut frame_table = FrameTable::default();
    let cie_id = frame_table.add_cie(cie);
    for function in functions {
        let Some(unwind_info) = function.systemv_unwind_info else {
            continue;
        };
        frame_table.add_fde(
            cie_id,
            unwind_info.to_fde(DwarfAddress::Symbol {
                symbol: function.text_section_symbol_index as usize,
                addend: i64::try_from(function.text_offset).map_err(|_| {
                    format!(
                        "function text offset for {} does not fit .eh_frame addend",
                        function.symbol
                    )
                })?,
            }),
        );
    }
    if frame_table.fde_count() == 0 {
        return Ok(None);
    }

    let mut eh_frame = EhFrame(RelocatingDwarfWriter::new());
    frame_table
        .write_eh_frame(&mut eh_frame)
        .map_err(|err| format!("failed to serialize precompiled .eh_frame: {err}"))?;
    let mut section = eh_frame.0.finish()?;
    section.bytes.extend_from_slice(&0_u32.to_le_bytes());
    Ok(Some(section))
}

fn build_precompiled_dwarf_sections(
    functions: &[DwarfFunctionEntry<'_>],
) -> Result<PrecompiledDwarfSections, String> {
    let mut debug_str = Vec::new();
    let producer = push_string_table(&mut debug_str, "soac precompiled object")?;
    let unit_name = push_string_table(&mut debug_str, "soac-precompiled")?;
    let comp_dir = push_string_table(&mut debug_str, "")?;
    let mut function_names = Vec::with_capacity(functions.len());
    for function in functions {
        function_names.push(push_string_table(&mut debug_str, function.symbol)?);
    }

    let debug_abbrev = build_precompiled_debug_abbrev();
    let debug_info =
        build_precompiled_debug_info(functions, producer, unit_name, comp_dir, &function_names)?;
    let debug_line = build_precompiled_debug_line(functions)?;

    Ok(PrecompiledDwarfSections {
        debug_info,
        debug_abbrev,
        debug_line,
        debug_str,
    })
}

fn build_precompiled_debug_abbrev() -> Vec<u8> {
    let mut out = Vec::new();

    push_uleb128(&mut out, 1);
    push_uleb128(&mut out, DW_TAG_COMPILE_UNIT);
    out.push(DW_CHILDREN_YES);
    push_uleb128(&mut out, DW_AT_PRODUCER);
    push_uleb128(&mut out, DW_FORM_STRP);
    push_uleb128(&mut out, DW_AT_LANGUAGE);
    push_uleb128(&mut out, DW_FORM_DATA2);
    push_uleb128(&mut out, DW_AT_NAME);
    push_uleb128(&mut out, DW_FORM_STRP);
    push_uleb128(&mut out, DW_AT_COMP_DIR);
    push_uleb128(&mut out, DW_FORM_STRP);
    push_uleb128(&mut out, DW_AT_STMT_LIST);
    push_uleb128(&mut out, DW_FORM_SEC_OFFSET);
    push_uleb128(&mut out, 0);
    push_uleb128(&mut out, 0);

    push_uleb128(&mut out, 2);
    push_uleb128(&mut out, DW_TAG_SUBPROGRAM);
    out.push(DW_CHILDREN_NO);
    push_uleb128(&mut out, DW_AT_NAME);
    push_uleb128(&mut out, DW_FORM_STRP);
    push_uleb128(&mut out, DW_AT_LOW_PC);
    push_uleb128(&mut out, DW_FORM_ADDR);
    push_uleb128(&mut out, DW_AT_HIGH_PC);
    push_uleb128(&mut out, DW_FORM_DATA8);
    push_uleb128(&mut out, DW_AT_EXTERNAL);
    push_uleb128(&mut out, DW_FORM_FLAG_PRESENT);
    push_uleb128(&mut out, 0);
    push_uleb128(&mut out, 0);

    push_uleb128(&mut out, 0);
    out
}

fn build_precompiled_debug_info(
    functions: &[DwarfFunctionEntry<'_>],
    producer: u32,
    unit_name: u32,
    comp_dir: u32,
    function_names: &[u32],
) -> Result<DwarfSectionBytes, String> {
    let mut bytes = Vec::new();
    let mut relocations = Vec::new();

    push_u32(&mut bytes, 0);
    push_u16(&mut bytes, 4);
    push_u32(&mut bytes, 0);
    bytes.push(8);

    push_uleb128(&mut bytes, 1);
    push_u32(&mut bytes, producer);
    push_u16(&mut bytes, DW_LANG_PYTHON);
    push_u32(&mut bytes, unit_name);
    push_u32(&mut bytes, comp_dir);
    push_u32(&mut bytes, 0);

    for (function, name_offset) in functions.iter().zip(function_names.iter().copied()) {
        push_uleb128(&mut bytes, 2);
        push_u32(&mut bytes, name_offset);
        let reloc_offset = bytes.len() as u64;
        push_u64(&mut bytes, 0);
        relocations.push(ElfRelocation {
            offset: reloc_offset,
            symbol_index: function.symbol_index,
            reloc_type: R_X86_64_64,
            addend: 0,
        });
        push_u64(&mut bytes, function.code_size);
    }
    bytes.push(0);

    let unit_len = u32::try_from(bytes.len() - 4)
        .map_err(|_| "precompiled .debug_info unit exceeded DWARF32 size".to_string())?;
    put_u32(&mut bytes, 0, unit_len);

    Ok(DwarfSectionBytes { bytes, relocations })
}

fn build_precompiled_debug_line(
    functions: &[DwarfFunctionEntry<'_>],
) -> Result<DwarfSectionBytes, String> {
    let mut bytes = Vec::new();
    let mut relocations = Vec::new();

    push_u32(&mut bytes, 0);
    push_u16(&mut bytes, 4);
    let header_length_offset = bytes.len();
    push_u32(&mut bytes, 0);
    let header_start = bytes.len();
    bytes.push(1);
    bytes.push(1);
    bytes.push(1);
    bytes.push((-5_i8) as u8);
    bytes.push(14);
    bytes.push(13);
    bytes.extend_from_slice(&[0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1]);
    bytes.push(0);
    bytes.extend_from_slice(b"soac-precompiled\0");
    push_uleb128(&mut bytes, 0);
    push_uleb128(&mut bytes, 0);
    push_uleb128(&mut bytes, 0);
    bytes.push(0);
    let header_len = u32::try_from(bytes.len() - header_start)
        .map_err(|_| "precompiled .debug_line header exceeded DWARF32 size".to_string())?;
    put_u32(&mut bytes, header_length_offset, header_len);

    for (index, function) in functions.iter().enumerate() {
        bytes.push(DW_LNE_EXTENDED_OPCODE);
        push_uleb128(&mut bytes, 1 + 8);
        bytes.push(DW_LNE_SET_ADDRESS);
        let reloc_offset = bytes.len() as u64;
        push_u64(&mut bytes, 0);
        relocations.push(ElfRelocation {
            offset: reloc_offset,
            symbol_index: function.symbol_index,
            reloc_type: R_X86_64_64,
            addend: 0,
        });

        bytes.push(DW_LNS_ADVANCE_LINE);
        push_sleb128(&mut bytes, index as i64);
        bytes.push(DW_LNS_COPY);
        bytes.push(DW_LNS_ADVANCE_PC);
        push_uleb128(&mut bytes, function.code_size);
        bytes.push(DW_LNE_EXTENDED_OPCODE);
        push_uleb128(&mut bytes, 1);
        bytes.push(DW_LNE_END_SEQUENCE);
    }

    let unit_len = u32::try_from(bytes.len() - 4)
        .map_err(|_| "precompiled .debug_line unit exceeded DWARF32 size".to_string())?;
    put_u32(&mut bytes, 0, unit_len);

    Ok(DwarfSectionBytes { bytes, relocations })
}

pub(super) fn write_precompiled_object(
    jit_module: &JITModule,
    isa: &dyn TargetIsa,
    function_definitions: &[ObjectFunctionDefinition],
    data_definitions: &[ObjectDataDefinition],
) -> Result<Vec<u8>, String> {
    if !(cfg!(target_os = "linux")
        && cfg!(target_arch = "x86_64")
        && cfg!(target_endian = "little"))
    {
        return Err(format!(
            "precompile object output currently supports only little-endian linux/x86_64 hosts, got {}",
            std::env::consts::ARCH
        ));
    }

    let mut object = ElfObjectBuilder::default();
    let mut function_symbols = HashMap::new();
    let mut data_symbols = HashMap::new();
    let text_section_symbol_index = object.add_defined_symbol(
        "",
        ElfSymbolBinding::Local,
        ElfSymbolKind::Section,
        ElfSectionIndex::Text,
        0,
        0,
    );

    for function in function_definitions {
        let offset =
            object.append_text(function.bytes.code.as_slice(), function.bytes.alignment)?;
        let symbol_index = object.add_defined_symbol(
            function.symbol.as_str(),
            function.binding,
            ElfSymbolKind::Func,
            ElfSectionIndex::Text,
            offset,
            function.bytes.code.len() as u64,
        );
        function_symbols.insert(function.func_id, (symbol_index, offset));
    }

    for data in data_definitions {
        let section = if data.writable {
            ElfSectionIndex::Data
        } else if data.relocations.is_empty() {
            ElfSectionIndex::Rodata
        } else {
            ElfSectionIndex::DataRelRo
        };
        let offset = object.append_data(section, data.bytes.as_slice(), data.align)?;
        let symbol_index = object.add_defined_symbol(
            data.symbol.as_str(),
            data.binding,
            ElfSymbolKind::Object,
            section,
            offset,
            data.bytes.len() as u64,
        );
        for relocation in &data.relocations {
            let relocation_offset = offset
                .checked_add(relocation.offset)
                .ok_or_else(|| format!("data relocation offset overflow in {}", data.symbol))?;
            let symbol_index =
                object.add_global_undefined_symbol(relocation.symbol.as_str(), relocation.kind);
            object.add_section_relocation(
                section,
                ElfRelocation {
                    offset: relocation_offset,
                    symbol_index,
                    reloc_type: relocation.reloc_type,
                    addend: relocation.addend,
                },
            )?;
        }
        data_symbols.insert(data.data_id, symbol_index);
    }

    let debug_functions = function_definitions
        .iter()
        .map(|function| {
            let (symbol_index, text_offset) = function_symbols[&function.func_id];
            DwarfFunctionEntry {
                symbol: function.symbol.as_str(),
                symbol_index,
                text_section_symbol_index,
                text_offset,
                code_size: function.bytes.code.len() as u64,
                systemv_unwind_info: function.systemv_unwind_info.as_ref(),
            }
        })
        .collect::<Vec<_>>();
    if let Some(eh_frame) = build_eh_frame_section(isa, debug_functions.as_slice())? {
        object.set_section_bytes(ElfSectionIndex::EhFrame, eh_frame.bytes)?;
        for relocation in eh_frame.relocations {
            object.add_section_relocation(ElfSectionIndex::EhFrame, relocation)?;
        }
    }
    let dwarf = build_precompiled_dwarf_sections(debug_functions.as_slice())?;
    object.set_section_bytes(ElfSectionIndex::DebugInfo, dwarf.debug_info.bytes)?;
    for relocation in dwarf.debug_info.relocations {
        object.add_section_relocation(ElfSectionIndex::DebugInfo, relocation)?;
    }
    object.set_section_bytes(ElfSectionIndex::DebugAbbrev, dwarf.debug_abbrev)?;
    object.set_section_bytes(ElfSectionIndex::DebugLine, dwarf.debug_line.bytes)?;
    for relocation in dwarf.debug_line.relocations {
        object.add_section_relocation(ElfSectionIndex::DebugLine, relocation)?;
    }
    object.set_section_bytes(ElfSectionIndex::DebugStr, dwarf.debug_str)?;

    for function in function_definitions {
        let (_, function_offset) = function_symbols[&function.func_id];
        for reloc in &function.bytes.relocs {
            let (target_symbol, addend) = elf_symbol_for_reloc_target(
                jit_module,
                &mut object,
                &function_symbols,
                &data_symbols,
                &reloc.name,
                reloc.addend,
            )?;
            let offset = function_offset
                .checked_add(u64::from(reloc.offset))
                .ok_or_else(|| format!("relocation offset overflow in {}", function.symbol))?;
            object.add_text_relocation(ElfRelocation {
                offset,
                symbol_index: target_symbol,
                reloc_type: elf_relocation_type(reloc.kind)?,
                addend,
            });
        }
    }

    object.finish()
}

#[derive(Default)]
struct ElfObjectBuilder {
    text: Vec<u8>,
    data: Vec<u8>,
    data_rel_ro: Vec<u8>,
    rodata: Vec<u8>,
    eh_frame: Vec<u8>,
    debug_info: Vec<u8>,
    debug_abbrev: Vec<u8>,
    debug_line: Vec<u8>,
    debug_str: Vec<u8>,
    symbols: Vec<ElfSymbol>,
    global_symbols_by_name: HashMap<String, u32>,
    text_relocations: Vec<ElfRelocation>,
    data_relocations: Vec<ElfRelocation>,
    data_rel_ro_relocations: Vec<ElfRelocation>,
    rodata_relocations: Vec<ElfRelocation>,
    eh_frame_relocations: Vec<ElfRelocation>,
    debug_info_relocations: Vec<ElfRelocation>,
    debug_line_relocations: Vec<ElfRelocation>,
}

impl ElfObjectBuilder {
    fn append_text(&mut self, bytes: &[u8], align: u64) -> Result<u64, String> {
        let offset = append_aligned(&mut self.text, bytes, align.max(1))?;
        Ok(offset)
    }

    fn append_data(
        &mut self,
        section: ElfSectionIndex,
        bytes: &[u8],
        align: u64,
    ) -> Result<u64, String> {
        let target = match section {
            ElfSectionIndex::Data => &mut self.data,
            ElfSectionIndex::DataRelRo => &mut self.data_rel_ro,
            ElfSectionIndex::Rodata => &mut self.rodata,
            ElfSectionIndex::Text
            | ElfSectionIndex::EhFrame
            | ElfSectionIndex::DebugInfo
            | ElfSectionIndex::DebugAbbrev
            | ElfSectionIndex::DebugLine
            | ElfSectionIndex::DebugStr
            | ElfSectionIndex::Undefined => {
                return Err(format!("cannot append data to ELF section {section:?}"));
            }
        };
        append_aligned(target, bytes, align.max(1))
    }

    fn add_defined_symbol(
        &mut self,
        name: &str,
        binding: ElfSymbolBinding,
        kind: ElfSymbolKind,
        section: ElfSectionIndex,
        value: u64,
        size: u64,
    ) -> u32 {
        let index = (self.symbols.len() + 1) as u32;
        self.symbols.push(ElfSymbol {
            name: name.to_string(),
            binding,
            kind,
            section,
            value,
            size,
        });
        index
    }

    fn add_global_undefined_symbol(&mut self, name: &str, kind: ElfSymbolKind) -> u32 {
        if let Some(index) = self.global_symbols_by_name.get(name).copied() {
            return index;
        }
        let index = (self.symbols.len() + 1) as u32;
        self.symbols.push(ElfSymbol {
            name: name.to_string(),
            binding: ElfSymbolBinding::Global,
            kind,
            section: ElfSectionIndex::Undefined,
            value: 0,
            size: 0,
        });
        self.global_symbols_by_name.insert(name.to_string(), index);
        index
    }

    fn set_section_bytes(
        &mut self,
        section: ElfSectionIndex,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        let target = match section {
            ElfSectionIndex::EhFrame => &mut self.eh_frame,
            ElfSectionIndex::DebugInfo => &mut self.debug_info,
            ElfSectionIndex::DebugAbbrev => &mut self.debug_abbrev,
            ElfSectionIndex::DebugLine => &mut self.debug_line,
            ElfSectionIndex::DebugStr => &mut self.debug_str,
            ElfSectionIndex::Text
            | ElfSectionIndex::Data
            | ElfSectionIndex::DataRelRo
            | ElfSectionIndex::Rodata
            | ElfSectionIndex::Undefined => {
                return Err(format!("cannot replace ELF section {section:?} bytes"));
            }
        };
        *target = bytes;
        Ok(())
    }

    fn add_text_relocation(&mut self, relocation: ElfRelocation) {
        self.text_relocations.push(relocation);
    }

    fn add_section_relocation(
        &mut self,
        section: ElfSectionIndex,
        relocation: ElfRelocation,
    ) -> Result<(), String> {
        match section {
            ElfSectionIndex::Data => {
                self.data_relocations.push(relocation);
                Ok(())
            }
            ElfSectionIndex::DataRelRo => {
                self.data_rel_ro_relocations.push(relocation);
                Ok(())
            }
            ElfSectionIndex::Rodata => {
                self.rodata_relocations.push(relocation);
                Ok(())
            }
            ElfSectionIndex::EhFrame => {
                self.eh_frame_relocations.push(relocation);
                Ok(())
            }
            ElfSectionIndex::DebugInfo => {
                self.debug_info_relocations.push(relocation);
                Ok(())
            }
            ElfSectionIndex::DebugLine => {
                self.debug_line_relocations.push(relocation);
                Ok(())
            }
            ElfSectionIndex::Text
            | ElfSectionIndex::DebugAbbrev
            | ElfSectionIndex::DebugStr
            | ElfSectionIndex::Undefined => {
                Err(format!("cannot add relocation to ELF section {section:?}"))
            }
        }
    }

    fn finish(self) -> Result<Vec<u8>, String> {
        let mut strtab = Vec::from([0]);
        let mut symbol_names = Vec::with_capacity(self.symbols.len());
        for symbol in &self.symbols {
            symbol_names.push(push_string_table(&mut strtab, symbol.name.as_str())?);
        }

        let first_global_symbol = self
            .symbols
            .iter()
            .position(|symbol| symbol.binding == ElfSymbolBinding::Global)
            .map(|index| index + 1)
            .unwrap_or(self.symbols.len() + 1) as u32;

        let mut symtab = vec![0; ELF64_SYM_SIZE];
        for (symbol, name_offset) in self.symbols.iter().zip(symbol_names) {
            push_elf_symbol(&mut symtab, name_offset, symbol);
        }

        let mut rela_text = Vec::with_capacity(self.text_relocations.len() * ELF64_RELA_SIZE);
        for relocation in &self.text_relocations {
            push_u64(&mut rela_text, relocation.offset);
            push_u64(
                &mut rela_text,
                (u64::from(relocation.symbol_index) << 32) | u64::from(relocation.reloc_type),
            );
            push_i64(&mut rela_text, relocation.addend);
        }
        let mut rela_data = Vec::with_capacity(self.data_relocations.len() * ELF64_RELA_SIZE);
        for relocation in &self.data_relocations {
            push_u64(&mut rela_data, relocation.offset);
            push_u64(
                &mut rela_data,
                (u64::from(relocation.symbol_index) << 32) | u64::from(relocation.reloc_type),
            );
            push_i64(&mut rela_data, relocation.addend);
        }
        let mut rela_data_rel_ro =
            Vec::with_capacity(self.data_rel_ro_relocations.len() * ELF64_RELA_SIZE);
        for relocation in &self.data_rel_ro_relocations {
            push_u64(&mut rela_data_rel_ro, relocation.offset);
            push_u64(
                &mut rela_data_rel_ro,
                (u64::from(relocation.symbol_index) << 32) | u64::from(relocation.reloc_type),
            );
            push_i64(&mut rela_data_rel_ro, relocation.addend);
        }
        let mut rela_rodata = Vec::with_capacity(self.rodata_relocations.len() * ELF64_RELA_SIZE);
        for relocation in &self.rodata_relocations {
            push_u64(&mut rela_rodata, relocation.offset);
            push_u64(
                &mut rela_rodata,
                (u64::from(relocation.symbol_index) << 32) | u64::from(relocation.reloc_type),
            );
            push_i64(&mut rela_rodata, relocation.addend);
        }
        let mut rela_eh_frame =
            Vec::with_capacity(self.eh_frame_relocations.len() * ELF64_RELA_SIZE);
        for relocation in &self.eh_frame_relocations {
            push_u64(&mut rela_eh_frame, relocation.offset);
            push_u64(
                &mut rela_eh_frame,
                (u64::from(relocation.symbol_index) << 32) | u64::from(relocation.reloc_type),
            );
            push_i64(&mut rela_eh_frame, relocation.addend);
        }
        let mut rela_debug_info =
            Vec::with_capacity(self.debug_info_relocations.len() * ELF64_RELA_SIZE);
        for relocation in &self.debug_info_relocations {
            push_u64(&mut rela_debug_info, relocation.offset);
            push_u64(
                &mut rela_debug_info,
                (u64::from(relocation.symbol_index) << 32) | u64::from(relocation.reloc_type),
            );
            push_i64(&mut rela_debug_info, relocation.addend);
        }
        let mut rela_debug_line =
            Vec::with_capacity(self.debug_line_relocations.len() * ELF64_RELA_SIZE);
        for relocation in &self.debug_line_relocations {
            push_u64(&mut rela_debug_line, relocation.offset);
            push_u64(
                &mut rela_debug_line,
                (u64::from(relocation.symbol_index) << 32) | u64::from(relocation.reloc_type),
            );
            push_i64(&mut rela_debug_line, relocation.addend);
        }

        let mut shstrtab = Vec::from([0]);
        let text_name = push_string_table(&mut shstrtab, ".text")?;
        let data_name = push_string_table(&mut shstrtab, ".data")?;
        let data_rel_ro_name = push_string_table(&mut shstrtab, ".data.rel.ro")?;
        let rodata_name = push_string_table(&mut shstrtab, ".rodata")?;
        let eh_frame_name = push_string_table(&mut shstrtab, ".eh_frame")?;
        let debug_info_name = push_string_table(&mut shstrtab, ".debug_info")?;
        let debug_abbrev_name = push_string_table(&mut shstrtab, ".debug_abbrev")?;
        let debug_line_name = push_string_table(&mut shstrtab, ".debug_line")?;
        let debug_str_name = push_string_table(&mut shstrtab, ".debug_str")?;
        let rela_text_name = push_string_table(&mut shstrtab, ".rela.text")?;
        let rela_data_name = push_string_table(&mut shstrtab, ".rela.data")?;
        let rela_data_rel_ro_name = push_string_table(&mut shstrtab, ".rela.data.rel.ro")?;
        let rela_rodata_name = push_string_table(&mut shstrtab, ".rela.rodata")?;
        let rela_eh_frame_name = push_string_table(&mut shstrtab, ".rela.eh_frame")?;
        let rela_debug_info_name = push_string_table(&mut shstrtab, ".rela.debug_info")?;
        let rela_debug_line_name = push_string_table(&mut shstrtab, ".rela.debug_line")?;
        let symtab_name = push_string_table(&mut shstrtab, ".symtab")?;
        let strtab_name = push_string_table(&mut shstrtab, ".strtab")?;
        let shstrtab_name = push_string_table(&mut shstrtab, ".shstrtab")?;
        let gnu_stack_name = push_string_table(&mut shstrtab, ".note.GNU-stack")?;

        let mut file = vec![0; ELF64_EHDR_SIZE];
        let text_header = append_section_bytes(&mut file, self.text.as_slice(), 16)?;
        let data_header = append_section_bytes(&mut file, self.data.as_slice(), 8)?;
        let data_rel_ro_header = append_section_bytes(&mut file, self.data_rel_ro.as_slice(), 8)?;
        let rodata_header = append_section_bytes(&mut file, self.rodata.as_slice(), 8)?;
        let eh_frame_header = append_section_bytes(&mut file, self.eh_frame.as_slice(), 8)?;
        let debug_info_header = append_section_bytes(&mut file, self.debug_info.as_slice(), 1)?;
        let debug_abbrev_header = append_section_bytes(&mut file, self.debug_abbrev.as_slice(), 1)?;
        let debug_line_header = append_section_bytes(&mut file, self.debug_line.as_slice(), 1)?;
        let debug_str_header = append_section_bytes(&mut file, self.debug_str.as_slice(), 1)?;
        let rela_text_header = append_section_bytes(&mut file, rela_text.as_slice(), 8)?;
        let rela_data_header = append_section_bytes(&mut file, rela_data.as_slice(), 8)?;
        let rela_data_rel_ro_header =
            append_section_bytes(&mut file, rela_data_rel_ro.as_slice(), 8)?;
        let rela_rodata_header = append_section_bytes(&mut file, rela_rodata.as_slice(), 8)?;
        let rela_eh_frame_header = append_section_bytes(&mut file, rela_eh_frame.as_slice(), 8)?;
        let rela_debug_info_header =
            append_section_bytes(&mut file, rela_debug_info.as_slice(), 8)?;
        let rela_debug_line_header =
            append_section_bytes(&mut file, rela_debug_line.as_slice(), 8)?;
        let symtab_header = append_section_bytes(&mut file, symtab.as_slice(), 8)?;
        let strtab_header = append_section_bytes(&mut file, strtab.as_slice(), 1)?;
        let shstrtab_header = append_section_bytes(&mut file, shstrtab.as_slice(), 1)?;
        let section_header_offset = align_vec(&mut file, 8)?;

        let mut section_headers = Vec::with_capacity(ELF_SECTION_COUNT * ELF64_SHDR_SIZE);
        section_headers.resize(ELF64_SHDR_SIZE, 0);
        push_elf_section_header(
            &mut section_headers,
            text_name,
            SHT_PROGBITS,
            SHF_ALLOC | SHF_EXECINSTR,
            text_header,
            0,
            0,
            16,
            0,
        );
        push_elf_section_header(
            &mut section_headers,
            data_name,
            SHT_PROGBITS,
            SHF_ALLOC | SHF_WRITE,
            data_header,
            0,
            0,
            8,
            0,
        );
        push_elf_section_header(
            &mut section_headers,
            data_rel_ro_name,
            SHT_PROGBITS,
            SHF_ALLOC | SHF_WRITE,
            data_rel_ro_header,
            0,
            0,
            8,
            0,
        );
        push_elf_section_header(
            &mut section_headers,
            rodata_name,
            SHT_PROGBITS,
            SHF_ALLOC,
            rodata_header,
            0,
            0,
            8,
            0,
        );
        push_elf_section_header(
            &mut section_headers,
            eh_frame_name,
            SHT_PROGBITS,
            SHF_ALLOC,
            eh_frame_header,
            0,
            0,
            8,
            0,
        );
        push_elf_section_header(
            &mut section_headers,
            debug_info_name,
            SHT_PROGBITS,
            0,
            debug_info_header,
            0,
            0,
            1,
            0,
        );
        push_elf_section_header(
            &mut section_headers,
            debug_abbrev_name,
            SHT_PROGBITS,
            0,
            debug_abbrev_header,
            0,
            0,
            1,
            0,
        );
        push_elf_section_header(
            &mut section_headers,
            debug_line_name,
            SHT_PROGBITS,
            0,
            debug_line_header,
            0,
            0,
            1,
            0,
        );
        push_elf_section_header(
            &mut section_headers,
            debug_str_name,
            SHT_PROGBITS,
            0,
            debug_str_header,
            0,
            0,
            1,
            0,
        );
        push_elf_section_header(
            &mut section_headers,
            rela_text_name,
            SHT_RELA,
            0,
            rela_text_header,
            ELF_SECTION_SYMTAB_INDEX,
            ELF_SECTION_TEXT_INDEX,
            8,
            ELF64_RELA_SIZE as u64,
        );
        push_elf_section_header(
            &mut section_headers,
            rela_data_name,
            SHT_RELA,
            0,
            rela_data_header,
            ELF_SECTION_SYMTAB_INDEX,
            ELF_SECTION_DATA_INDEX,
            8,
            ELF64_RELA_SIZE as u64,
        );
        push_elf_section_header(
            &mut section_headers,
            rela_data_rel_ro_name,
            SHT_RELA,
            0,
            rela_data_rel_ro_header,
            ELF_SECTION_SYMTAB_INDEX,
            ELF_SECTION_DATA_REL_RO_INDEX,
            8,
            ELF64_RELA_SIZE as u64,
        );
        push_elf_section_header(
            &mut section_headers,
            rela_rodata_name,
            SHT_RELA,
            0,
            rela_rodata_header,
            ELF_SECTION_SYMTAB_INDEX,
            ELF_SECTION_RODATA_INDEX,
            8,
            ELF64_RELA_SIZE as u64,
        );
        push_elf_section_header(
            &mut section_headers,
            rela_eh_frame_name,
            SHT_RELA,
            0,
            rela_eh_frame_header,
            ELF_SECTION_SYMTAB_INDEX,
            ELF_SECTION_EH_FRAME_INDEX,
            8,
            ELF64_RELA_SIZE as u64,
        );
        push_elf_section_header(
            &mut section_headers,
            rela_debug_info_name,
            SHT_RELA,
            0,
            rela_debug_info_header,
            ELF_SECTION_SYMTAB_INDEX,
            ELF_SECTION_DEBUG_INFO_INDEX,
            8,
            ELF64_RELA_SIZE as u64,
        );
        push_elf_section_header(
            &mut section_headers,
            rela_debug_line_name,
            SHT_RELA,
            0,
            rela_debug_line_header,
            ELF_SECTION_SYMTAB_INDEX,
            ELF_SECTION_DEBUG_LINE_INDEX,
            8,
            ELF64_RELA_SIZE as u64,
        );
        push_elf_section_header(
            &mut section_headers,
            symtab_name,
            SHT_SYMTAB,
            0,
            symtab_header,
            ELF_SECTION_STRTAB_INDEX,
            first_global_symbol,
            8,
            ELF64_SYM_SIZE as u64,
        );
        push_elf_section_header(
            &mut section_headers,
            strtab_name,
            SHT_STRTAB,
            0,
            strtab_header,
            0,
            0,
            1,
            0,
        );
        push_elf_section_header(
            &mut section_headers,
            shstrtab_name,
            SHT_STRTAB,
            0,
            shstrtab_header,
            0,
            0,
            1,
            0,
        );
        push_elf_section_header(
            &mut section_headers,
            gnu_stack_name,
            SHT_PROGBITS,
            0,
            ElfSectionHeaderInput { offset: 0, size: 0 },
            0,
            0,
            1,
            0,
        );
        file.extend_from_slice(section_headers.as_slice());
        write_elf_header(&mut file[..ELF64_EHDR_SIZE], section_header_offset)?;
        Ok(file)
    }
}

fn elf_symbol_for_reloc_target(
    jit_module: &JITModule,
    object: &mut ElfObjectBuilder,
    function_symbols: &HashMap<FuncId, (u32, u64)>,
    data_symbols: &HashMap<DataId, u32>,
    target: &ModuleRelocTarget,
    addend: i64,
) -> Result<(u32, i64), String> {
    match target {
        ModuleRelocTarget::User { namespace: 0, .. } => {
            let func_id = FuncId::from_name(target);
            if let Some((symbol_id, _offset)) = function_symbols.get(&func_id).copied() {
                return Ok((symbol_id, addend));
            }
            let decl = jit_module.declarations().get_function_decl(func_id);
            if decl.linkage.requires_definition() {
                return Err(format!(
                    "relocation references local function {} that was not emitted into the object",
                    decl.linkage_name(func_id)
                ));
            }
            let symbol = decl.linkage_name(func_id).into_owned();
            Ok((
                object.add_global_undefined_symbol(symbol.as_str(), ElfSymbolKind::Func),
                addend,
            ))
        }
        ModuleRelocTarget::User { namespace: 1, .. } => {
            let data_id = DataId::from_name(target);
            if let Some(symbol_id) = data_symbols.get(&data_id).copied() {
                return Ok((symbol_id, addend));
            }
            let decl = jit_module.declarations().get_data_decl(data_id);
            if decl.linkage.requires_definition() {
                return Err(format!(
                    "relocation references local data object {} that was not emitted into the object",
                    decl.linkage_name(data_id)
                ));
            }
            let symbol = decl.linkage_name(data_id).into_owned();
            Ok((
                object.add_global_undefined_symbol(symbol.as_str(), ElfSymbolKind::Object),
                addend,
            ))
        }
        ModuleRelocTarget::User { namespace, index } => Err(format!(
            "unsupported Cranelift user relocation namespace {namespace}:{index}"
        )),
        ModuleRelocTarget::LibCall(libcall) => {
            let libcall_names = cranelift_module::default_libcall_names();
            let symbol = libcall_names(*libcall);
            Ok((
                object.add_global_undefined_symbol(symbol.as_str(), ElfSymbolKind::Func),
                addend,
            ))
        }
        ModuleRelocTarget::KnownSymbol(symbol) => {
            let symbol = match symbol {
                ir::KnownSymbol::ElfGlobalOffsetTable => "_GLOBAL_OFFSET_TABLE_",
                ir::KnownSymbol::CoffTlsIndex => "__tls_index",
            };
            Ok((
                object.add_global_undefined_symbol(symbol, ElfSymbolKind::Object),
                addend,
            ))
        }
        ModuleRelocTarget::FunctionOffset(func_id, offset) => {
            let (symbol_id, _target_offset) =
                function_symbols.get(func_id).copied().ok_or_else(|| {
                    format!("relocation references undefined function offset target {func_id}")
                })?;
            Ok((symbol_id, addend + i64::from(*offset)))
        }
    }
}

fn elf_relocation_type(reloc: Reloc) -> Result<u32, String> {
    match reloc {
        Reloc::Abs4 => Ok(R_X86_64_32),
        Reloc::Abs8 => Ok(R_X86_64_64),
        Reloc::X86PCRel4 => Ok(R_X86_64_PC32),
        Reloc::X86CallPCRel4 | Reloc::X86CallPLTRel4 => Ok(R_X86_64_PLT32),
        Reloc::X86GOTPCRel4 => Ok(R_X86_64_GOTPCREL),
        other => Err(format!(
            "unsupported Cranelift relocation for precompiled object: {other:?}"
        )),
    }
}

fn elf_relocation_type_for_dwarf(
    size: u8,
    eh_pe: Option<gimli::constants::DwEhPe>,
) -> Result<u32, String> {
    match eh_pe {
        Some(encoding)
            if encoding.application() == gimli::constants::DW_EH_PE_pcrel
                && encoding.format() == gimli::constants::DW_EH_PE_sdata4 =>
        {
            Ok(R_X86_64_PC32)
        }
        Some(encoding)
            if encoding.application() == gimli::constants::DW_EH_PE_absptr && size == 8 =>
        {
            Ok(R_X86_64_64)
        }
        Some(encoding) => Err(format!(
            "unsupported DWARF relocation pointer encoding for precompiled object: {encoding:?}"
        )),
        None if size == 8 => Ok(R_X86_64_64),
        None if size == 4 => Ok(R_X86_64_32),
        None => Err(format!(
            "unsupported DWARF relocation size for precompiled object: {size}"
        )),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ElfSectionIndex {
    Undefined,
    Text,
    Data,
    DataRelRo,
    Rodata,
    EhFrame,
    DebugInfo,
    DebugAbbrev,
    DebugLine,
    DebugStr,
}

impl ElfSectionIndex {
    fn as_u16(self) -> u16 {
        match self {
            Self::Undefined => 0,
            Self::Text => ELF_SECTION_TEXT_INDEX as u16,
            Self::Data => ELF_SECTION_DATA_INDEX as u16,
            Self::DataRelRo => ELF_SECTION_DATA_REL_RO_INDEX as u16,
            Self::Rodata => ELF_SECTION_RODATA_INDEX as u16,
            Self::EhFrame => ELF_SECTION_EH_FRAME_INDEX as u16,
            Self::DebugInfo => ELF_SECTION_DEBUG_INFO_INDEX as u16,
            Self::DebugAbbrev => ELF_SECTION_DEBUG_ABBREV_INDEX as u16,
            Self::DebugLine => ELF_SECTION_DEBUG_LINE_INDEX as u16,
            Self::DebugStr => ELF_SECTION_DEBUG_STR_INDEX as u16,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ElfSymbolKind {
    Object,
    Func,
    Section,
}

impl ElfSymbolKind {
    fn as_u8(self) -> u8 {
        match self {
            Self::Object => 1,
            Self::Func => 2,
            Self::Section => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ElfSymbolBinding {
    Local,
    Global,
}

impl ElfSymbolBinding {
    fn as_u8(self) -> u8 {
        match self {
            Self::Local => 0,
            Self::Global => 1,
        }
    }
}

struct ElfSymbol {
    name: String,
    binding: ElfSymbolBinding,
    kind: ElfSymbolKind,
    section: ElfSectionIndex,
    value: u64,
    size: u64,
}

struct ElfRelocation {
    offset: u64,
    symbol_index: u32,
    reloc_type: u32,
    addend: i64,
}

#[derive(Clone, Copy)]
struct ElfSectionHeaderInput {
    offset: u64,
    size: u64,
}

const ELF64_EHDR_SIZE: usize = 64;
const ELF64_SHDR_SIZE: usize = 64;
const ELF64_SYM_SIZE: usize = 24;
const ELF64_RELA_SIZE: usize = 24;
const ELF_SECTION_COUNT: usize = 21;
const ELF_SECTION_TEXT_INDEX: u32 = 1;
const ELF_SECTION_DATA_INDEX: u32 = 2;
const ELF_SECTION_DATA_REL_RO_INDEX: u32 = 3;
const ELF_SECTION_RODATA_INDEX: u32 = 4;
const ELF_SECTION_EH_FRAME_INDEX: u32 = 5;
const ELF_SECTION_DEBUG_INFO_INDEX: u32 = 6;
const ELF_SECTION_DEBUG_ABBREV_INDEX: u32 = 7;
const ELF_SECTION_DEBUG_LINE_INDEX: u32 = 8;
const ELF_SECTION_DEBUG_STR_INDEX: u32 = 9;
const ELF_SECTION_SYMTAB_INDEX: u32 = 17;
const ELF_SECTION_STRTAB_INDEX: u32 = 18;
const ELF_SECTION_SHSTRTAB_INDEX: u16 = 19;
const SHT_PROGBITS: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_RELA: u32 = 4;
const SHF_WRITE: u64 = 0x1;
const SHF_ALLOC: u64 = 0x2;
const SHF_EXECINSTR: u64 = 0x4;
const EM_X86_64: u16 = 62;
const ET_REL: u16 = 1;
pub(super) const R_X86_64_64: u32 = 1;
const R_X86_64_PC32: u32 = 2;
const R_X86_64_PLT32: u32 = 4;
const R_X86_64_GOTPCREL: u32 = 9;
const R_X86_64_32: u32 = 10;
const DW_TAG_COMPILE_UNIT: u64 = 0x11;
const DW_TAG_SUBPROGRAM: u64 = 0x2e;
const DW_CHILDREN_NO: u8 = 0;
const DW_CHILDREN_YES: u8 = 1;
const DW_AT_NAME: u64 = 0x03;
const DW_AT_STMT_LIST: u64 = 0x10;
const DW_AT_LOW_PC: u64 = 0x11;
const DW_AT_HIGH_PC: u64 = 0x12;
const DW_AT_LANGUAGE: u64 = 0x13;
const DW_AT_COMP_DIR: u64 = 0x1b;
const DW_AT_PRODUCER: u64 = 0x25;
const DW_AT_EXTERNAL: u64 = 0x3f;
const DW_FORM_ADDR: u64 = 0x01;
const DW_FORM_DATA2: u64 = 0x05;
const DW_FORM_DATA8: u64 = 0x07;
const DW_FORM_STRP: u64 = 0x0e;
const DW_FORM_SEC_OFFSET: u64 = 0x17;
const DW_FORM_FLAG_PRESENT: u64 = 0x19;
const DW_LANG_PYTHON: u16 = 0x0014;
const DW_LNS_COPY: u8 = 1;
const DW_LNS_ADVANCE_PC: u8 = 2;
const DW_LNS_ADVANCE_LINE: u8 = 3;
const DW_LNE_EXTENDED_OPCODE: u8 = 0;
const DW_LNE_END_SEQUENCE: u8 = 1;
const DW_LNE_SET_ADDRESS: u8 = 2;

fn append_aligned(out: &mut Vec<u8>, bytes: &[u8], align: u64) -> Result<u64, String> {
    let offset = align_vec(out, align)?;
    out.extend_from_slice(bytes);
    Ok(offset)
}

fn append_section_bytes(
    out: &mut Vec<u8>,
    bytes: &[u8],
    align: u64,
) -> Result<ElfSectionHeaderInput, String> {
    let offset = append_aligned(out, bytes, align)?;
    Ok(ElfSectionHeaderInput {
        offset,
        size: bytes.len() as u64,
    })
}

fn align_vec(out: &mut Vec<u8>, align: u64) -> Result<u64, String> {
    if !align.is_power_of_two() {
        return Err(format!(
            "ELF section alignment must be a power of two, got {align}"
        ));
    }
    let align = usize::try_from(align)
        .map_err(|_| format!("ELF section alignment is too large: {align}"))?;
    let padding = (align - (out.len() % align)) % align;
    out.resize(out.len() + padding, 0);
    Ok(out.len() as u64)
}

fn push_string_table(table: &mut Vec<u8>, value: &str) -> Result<u32, String> {
    let offset = u32::try_from(table.len())
        .map_err(|_| "ELF string table exceeds u32 offsets".to_string())?;
    table.extend_from_slice(value.as_bytes());
    table.push(0);
    Ok(offset)
}

fn write_elf_header(header: &mut [u8], section_header_offset: u64) -> Result<(), String> {
    if header.len() != ELF64_EHDR_SIZE {
        return Err("internal error: ELF header buffer has wrong size".to_string());
    }
    header[0..4].copy_from_slice(b"\x7fELF");
    header[4] = 2; // 64-bit
    header[5] = 1; // little endian
    header[6] = 1; // ELF version
    put_u16(header, 16, ET_REL);
    put_u16(header, 18, EM_X86_64);
    put_u32(header, 20, 1);
    put_u64(header, 40, section_header_offset);
    put_u16(header, 52, ELF64_EHDR_SIZE as u16);
    put_u16(header, 58, ELF64_SHDR_SIZE as u16);
    put_u16(header, 60, ELF_SECTION_COUNT as u16);
    put_u16(header, 62, ELF_SECTION_SHSTRTAB_INDEX);
    Ok(())
}

fn push_elf_section_header(
    out: &mut Vec<u8>,
    name_offset: u32,
    section_type: u32,
    flags: u64,
    input: ElfSectionHeaderInput,
    link: u32,
    info: u32,
    align: u64,
    entry_size: u64,
) {
    push_u32(out, name_offset);
    push_u32(out, section_type);
    push_u64(out, flags);
    push_u64(out, 0);
    push_u64(out, input.offset);
    push_u64(out, input.size);
    push_u32(out, link);
    push_u32(out, info);
    push_u64(out, align);
    push_u64(out, entry_size);
}

fn push_elf_symbol(out: &mut Vec<u8>, name_offset: u32, symbol: &ElfSymbol) {
    push_u32(out, name_offset);
    out.push((symbol.binding.as_u8() << 4) | symbol.kind.as_u8());
    out.push(0);
    push_u16(out, symbol.section.as_u16());
    push_u64(out, symbol.value);
    push_u64(out, symbol.size);
}

fn put_u16(out: &mut [u8], offset: usize, value: u16) {
    out[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut [u8], offset: usize, value: u32) {
    out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut [u8], offset: usize, value: u64) {
    out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_uleb128(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn push_sleb128(out: &mut Vec<u8>, mut value: i64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        let sign_bit_set = (byte & 0x40) != 0;
        let done = (value == 0 && !sign_bit_set) || (value == -1 && sign_bit_set);
        if done {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}
