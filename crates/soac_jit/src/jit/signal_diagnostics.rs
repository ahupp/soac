#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
use libc::{c_int, c_void};
use soac_core::block_py::RuntimeFunctionId;
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};

const MAX_JIT_CODE_RANGES: usize = 8192;

struct JitCodeRangeSlot {
    start: AtomicUsize,
    end: AtomicUsize,
    symbol_ptr: AtomicPtr<u8>,
    symbol_len: AtomicUsize,
    qualname_ptr: AtomicPtr<u8>,
    qualname_len: AtomicUsize,
    entry_kind_ptr: AtomicPtr<u8>,
    entry_kind_len: AtomicUsize,
    function_id: AtomicU64,
    bb_offsets_ptr: AtomicPtr<usize>,
    bb_offsets_len: AtomicUsize,
}

impl JitCodeRangeSlot {
    const fn empty() -> Self {
        Self {
            start: AtomicUsize::new(0),
            end: AtomicUsize::new(0),
            symbol_ptr: AtomicPtr::new(ptr::null_mut()),
            symbol_len: AtomicUsize::new(0),
            qualname_ptr: AtomicPtr::new(ptr::null_mut()),
            qualname_len: AtomicUsize::new(0),
            entry_kind_ptr: AtomicPtr::new(ptr::null_mut()),
            entry_kind_len: AtomicUsize::new(0),
            function_id: AtomicU64::new(0),
            bb_offsets_ptr: AtomicPtr::new(ptr::null_mut()),
            bb_offsets_len: AtomicUsize::new(0),
        }
    }
}

static INSTALL_SIGILL_HANDLER: OnceLock<Result<(), &'static str>> = OnceLock::new();
static NEXT_JIT_CODE_RANGE: AtomicUsize = AtomicUsize::new(0);
static JIT_CODE_RANGES: [JitCodeRangeSlot; MAX_JIT_CODE_RANGES] =
    [const { JitCodeRangeSlot::empty() }; MAX_JIT_CODE_RANGES];

pub(crate) fn install_sigill_diagnostics() -> Result<(), String> {
    INSTALL_SIGILL_HANDLER
        .get_or_init(install_sigill_diagnostics_once)
        .map_err(|err| (*err).to_string())
}

fn install_sigill_diagnostics_once() -> Result<(), &'static str> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = sigill_handler as *const () as usize;
        action.sa_flags = libc::SA_SIGINFO;
        if libc::sigemptyset(&mut action.sa_mask) != 0 {
            return Err("failed to initialize SIGILL diagnostic signal mask");
        }
        if libc::sigaction(libc::SIGILL, &action, ptr::null_mut()) != 0 {
            return Err("failed to install SIGILL diagnostic handler");
        }
        Ok(())
    }

    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        Ok(())
    }
}

pub(crate) fn register_jit_code_range(
    symbol: &str,
    code_ptr: *const u8,
    code_size: usize,
    function_id: RuntimeFunctionId,
    function_qualname: &str,
    entry_kind: &str,
    bb_offsets: &[usize],
) {
    if code_ptr.is_null() || code_size == 0 {
        return;
    }
    let index = NEXT_JIT_CODE_RANGE.fetch_add(1, Ordering::AcqRel);
    let Some(slot) = JIT_CODE_RANGES.get(index) else {
        return;
    };

    let symbol = leak_str(symbol);
    let function_qualname = leak_str(function_qualname);
    let entry_kind = leak_str(entry_kind);
    let bb_offsets = bb_offsets.to_vec().into_boxed_slice();
    let bb_offsets_len = bb_offsets.len();
    let bb_offsets_ptr = Box::leak(bb_offsets).as_mut_ptr();

    let start = code_ptr as usize;
    let end = start.saturating_add(code_size);
    slot.end.store(end, Ordering::Relaxed);
    slot.symbol_ptr
        .store(symbol.as_ptr() as *mut u8, Ordering::Relaxed);
    slot.symbol_len.store(symbol.len(), Ordering::Relaxed);
    slot.qualname_ptr
        .store(function_qualname.as_ptr() as *mut u8, Ordering::Relaxed);
    slot.qualname_len
        .store(function_qualname.len(), Ordering::Relaxed);
    slot.entry_kind_ptr
        .store(entry_kind.as_ptr() as *mut u8, Ordering::Relaxed);
    slot.entry_kind_len
        .store(entry_kind.len(), Ordering::Relaxed);
    slot.function_id
        .store(function_id.to_packed_runtime_u64(), Ordering::Relaxed);
    slot.bb_offsets_ptr.store(bb_offsets_ptr, Ordering::Relaxed);
    slot.bb_offsets_len.store(bb_offsets_len, Ordering::Relaxed);
    // Publish the entry last so the signal handler never observes partially initialized metadata.
    slot.start.store(start, Ordering::Release);
}

fn leak_str(value: &str) -> &'static str {
    Box::leak(value.to_string().into_boxed_str())
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe extern "C" fn sigill_handler(
    signal: c_int,
    _info: *mut libc::siginfo_t,
    context: *mut c_void,
) {
    let pc = signal_context_pc(context);
    write_lit(b"\nSOAC: caught SIGILL at pc=");
    write_hex_usize(pc);
    write_lit(b"\n");
    if let Some(range) = find_jit_code_range(pc) {
        let offset = pc.saturating_sub(range.start);
        write_lit(b"SOAC: JIT symbol ");
        write_bytes(range.symbol_ptr, range.symbol_len);
        write_lit(b"+");
        write_hex_usize(offset);
        write_lit(b"\nSOAC: function_id=");
        write_dec_u64(range.function_id >> 32);
        write_lit(b":");
        write_dec_u64(range.function_id & 0xffff_ffff);
        write_lit(b" qualname=");
        write_bytes(range.qualname_ptr, range.qualname_len);
        write_lit(b" entry=");
        write_bytes(range.entry_kind_ptr, range.entry_kind_len);
        write_lit(b"\n");
        if let Some(block) = block_for_offset(&range, offset) {
            write_lit(b"SOAC: approximate machine block=block");
            write_dec_usize(block.index);
            write_lit(b" range=");
            write_hex_usize(block.start);
            write_lit(b"..");
            write_hex_usize(block.end);
            write_lit(b"\n");
        }
    } else {
        write_lit(b"SOAC: pc is not in a registered SOAC JIT code range\n");
    }
    write_lit(b"SOAC: re-raising SIGILL for normal crash handling\n");
    restore_default_sigill_and_reraise(signal);
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn signal_context_pc(context: *mut c_void) -> usize {
    if context.is_null() {
        return 0;
    }
    let context = &*(context as *const libc::ucontext_t);
    context.uc_mcontext.gregs[libc::REG_RIP as usize] as usize
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
struct HandlerRange {
    start: usize,
    end: usize,
    symbol_ptr: *const u8,
    symbol_len: usize,
    qualname_ptr: *const u8,
    qualname_len: usize,
    entry_kind_ptr: *const u8,
    entry_kind_len: usize,
    function_id: u64,
    bb_offsets_ptr: *const usize,
    bb_offsets_len: usize,
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
struct HandlerBlock {
    index: usize,
    start: usize,
    end: usize,
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn find_jit_code_range(pc: usize) -> Option<HandlerRange> {
    let count = NEXT_JIT_CODE_RANGE
        .load(Ordering::Acquire)
        .min(MAX_JIT_CODE_RANGES);
    for slot in &JIT_CODE_RANGES[..count] {
        let start = slot.start.load(Ordering::Acquire);
        if start == 0 || pc < start {
            continue;
        }
        let end = slot.end.load(Ordering::Relaxed);
        if pc >= end {
            continue;
        }
        return Some(HandlerRange {
            start,
            end,
            symbol_ptr: slot.symbol_ptr.load(Ordering::Relaxed) as *const u8,
            symbol_len: slot.symbol_len.load(Ordering::Relaxed),
            qualname_ptr: slot.qualname_ptr.load(Ordering::Relaxed) as *const u8,
            qualname_len: slot.qualname_len.load(Ordering::Relaxed),
            entry_kind_ptr: slot.entry_kind_ptr.load(Ordering::Relaxed) as *const u8,
            entry_kind_len: slot.entry_kind_len.load(Ordering::Relaxed),
            function_id: slot.function_id.load(Ordering::Relaxed),
            bb_offsets_ptr: slot.bb_offsets_ptr.load(Ordering::Relaxed) as *const usize,
            bb_offsets_len: slot.bb_offsets_len.load(Ordering::Relaxed),
        });
    }
    None
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn block_for_offset(range: &HandlerRange, offset: usize) -> Option<HandlerBlock> {
    if range.bb_offsets_ptr.is_null() || range.bb_offsets_len == 0 {
        return None;
    }
    let mut index = 0;
    for candidate in 0..range.bb_offsets_len {
        let block_start = *range.bb_offsets_ptr.add(candidate);
        if block_start > offset {
            break;
        }
        index = candidate;
    }
    let start = *range.bb_offsets_ptr.add(index);
    let mut end = range.end.saturating_sub(range.start);
    for candidate in (index + 1)..range.bb_offsets_len {
        let candidate_start = *range.bb_offsets_ptr.add(candidate);
        if candidate_start > start {
            end = candidate_start;
            break;
        }
    }
    Some(HandlerBlock { index, start, end })
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn restore_default_sigill_and_reraise(signal: c_int) -> ! {
    let mut action: libc::sigaction = std::mem::zeroed();
    action.sa_sigaction = libc::SIG_DFL;
    action.sa_flags = 0;
    let _ = libc::sigemptyset(&mut action.sa_mask);
    let _ = libc::sigaction(signal, &action, ptr::null_mut());
    let _ = libc::raise(signal);
    libc::_exit(128 + signal);
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn write_lit(bytes: &'static [u8]) {
    write_raw(bytes.as_ptr(), bytes.len());
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn write_bytes(ptr: *const u8, len: usize) {
    if ptr.is_null() || len == 0 {
        write_lit(b"<unknown>");
        return;
    }
    write_raw(ptr, len);
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn write_raw(ptr: *const u8, len: usize) {
    let mut written = 0;
    while written < len {
        let result = libc::write(
            libc::STDERR_FILENO,
            ptr.add(written).cast::<c_void>(),
            len - written,
        );
        if result <= 0 {
            return;
        }
        written += result as usize;
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn write_hex_usize(value: usize) {
    let mut buf = [0_u8; 2 + usize::BITS as usize / 4];
    buf[0] = b'0';
    buf[1] = b'x';
    let mut started = false;
    let mut out = 2;
    for shift in (0..usize::BITS).step_by(4).rev() {
        let digit = ((value >> shift) & 0xf) as u8;
        if digit != 0 || started || shift == 0 {
            started = true;
            buf[out] = hex_digit(digit);
            out += 1;
        }
    }
    write_raw(buf.as_ptr(), out);
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn write_dec_usize(value: usize) {
    write_dec_u64(value as u64);
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn write_dec_u64(mut value: u64) {
    let mut buf = [0_u8; 20];
    let mut out = buf.len();
    loop {
        out -= 1;
        buf[out] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    write_raw(buf.as_ptr().add(out), buf.len() - out);
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const fn hex_digit(value: u8) -> u8 {
    match value {
        0..=9 => b'0' + value,
        _ => b'a' + (value - 10),
    }
}
