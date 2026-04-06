#[cfg(target_os = "linux")]
mod imp {
    use std::env;
    use std::fs::{File, OpenOptions};
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::path::{Path, PathBuf};
    use std::ptr;
    use std::slice;
    use std::sync::{Mutex, OnceLock};

    const JITDUMP_MAGIC: u32 = 0x4A69_5444;
    const JITDUMP_VERSION: u32 = 1;
    const PERF_JIT_CODE_LOAD: u32 = 0;

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    struct Header {
        magic: u32,
        version: u32,
        size: u32,
        elf_mach_target: u32,
        reserved: u32,
        process_id: u32,
        time_stamp: u64,
        flags: u64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    struct BaseEvent {
        event: u32,
        size: u32,
        time_stamp: u64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    struct CodeLoadEvent {
        base: BaseEvent,
        process_id: u32,
        thread_id: u32,
        vma: u64,
        code_address: u64,
        code_size: u64,
        code_id: u64,
    }

    #[derive(Debug)]
    pub(crate) struct JitDumpSession {
        file: File,
        mapped_buffer: *mut libc::c_void,
        mapped_size: usize,
        next_code_id: u64,
    }

    // The mapped buffer is process-global state guarded by the outer mutex.
    unsafe impl Send for JitDumpSession {}

    static JITDUMP_SESSION: OnceLock<Result<Option<Mutex<JitDumpSession>>, String>> = OnceLock::new();

    impl JitDumpSession {
        fn new() -> Result<Self, String> {
            let dump_dir = env::var_os("SOAC_JIT_JITDUMP_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/tmp"));
            Self::new_in_dir(&dump_dir)
        }

        fn new_in_dir(dir: &Path) -> Result<Self, String> {
            std::fs::create_dir_all(dir)
                .map_err(|err| format!("failed to create jitdump dir {}: {err}", dir.display()))?;
            let path = dir.join(format!("jit-{}.dump", std::process::id()));
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|err| format!("failed to open jitdump file {}: {err}", path.display()))?;

            let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            if page_size <= 0 {
                return Err("failed to query page size for jitdump".to_string());
            }
            let page_size = page_size as usize;
            file.set_len(page_size as u64)
                .map_err(|err| format!("failed to size jitdump file {}: {err}", path.display()))?;

            let mapped_buffer = unsafe {
                libc::mmap(
                    ptr::null_mut(),
                    page_size,
                    libc::PROT_READ | libc::PROT_EXEC,
                    libc::MAP_PRIVATE,
                    file.as_raw_fd(),
                    0,
                )
            };
            if mapped_buffer == libc::MAP_FAILED {
                return Err(format!(
                    "failed to mmap jitdump marker page {}: {}",
                    path.display(),
                    std::io::Error::last_os_error()
                ));
            }

            let header = Header {
                magic: JITDUMP_MAGIC,
                version: JITDUMP_VERSION,
                size: std::mem::size_of::<Header>() as u32,
                elf_mach_target: elf_machine_architecture(),
                reserved: 0,
                process_id: std::process::id(),
                time_stamp: current_time_microseconds()?,
                flags: 0,
            };
            write_plain(&mut file, &header)?;

            Ok(Self {
                file,
                mapped_buffer,
                mapped_size: page_size,
                next_code_id: 1,
            })
        }

        fn record_code_load(
            &mut self,
            name: &str,
            code_ptr: *const u8,
            code_size: usize,
        ) -> Result<(), String> {
            let code_bytes = unsafe { slice::from_raw_parts(code_ptr, code_size) };
            let name_bytes = name.as_bytes();
            let event = CodeLoadEvent {
                base: BaseEvent {
                    event: PERF_JIT_CODE_LOAD,
                    size: (std::mem::size_of::<CodeLoadEvent>() + name_bytes.len() + 1 + code_size)
                        as u32,
                    time_stamp: current_monotonic_ticks()?,
                },
                process_id: std::process::id(),
                thread_id: current_thread_id()?,
                vma: code_ptr as usize as u64,
                code_address: code_ptr as usize as u64,
                code_size: code_size as u64,
                code_id: self.next_code_id,
            };
            self.next_code_id += 1;
            write_plain(&mut self.file, &event)?;
            self.file
                .write_all(name_bytes)
                .map_err(|err| format!("failed to write jitdump symbol name {name}: {err}"))?;
            self.file
                .write_all(&[0])
                .map_err(|err| format!("failed to terminate jitdump symbol name {name}: {err}"))?;
            self.file
                .write_all(code_bytes)
                .map_err(|err| format!("failed to write jitdump code bytes for {name}: {err}"))?;
            self.file
                .flush()
                .map_err(|err| format!("failed to flush jitdump record for {name}: {err}"))?;
            Ok(())
        }
    }

    impl Drop for JitDumpSession {
        fn drop(&mut self) {
            let _ = self.file.flush();
            if !self.mapped_buffer.is_null() {
                unsafe {
                    libc::munmap(self.mapped_buffer, self.mapped_size);
                }
                self.mapped_buffer = ptr::null_mut();
            }
        }
    }

    pub(crate) fn record_code_load(
        name: &str,
        code_ptr: *const u8,
        code_size: usize,
    ) -> Result<(), String> {
        let session = JITDUMP_SESSION.get_or_init(|| {
            if env::var_os("PERF_BUILDID_DIR").is_none() {
                return Ok(None);
            }
            Ok(Some(Mutex::new(JitDumpSession::new()?)))
        });
        let Some(session) = session.as_ref().map_err(|err| err.clone())? else {
            return Ok(());
        };
        let mut session = session
            .lock()
            .map_err(|_| "jitdump session lock poisoned".to_string())?;
        session.record_code_load(name, code_ptr, code_size)
    }

    fn current_monotonic_ticks() -> Result<u64, String> {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
        if rc != 0 {
            return Err(format!(
                "clock_gettime(CLOCK_MONOTONIC) failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok((ts.tv_sec as u64) * 1_000_000_000 + ts.tv_nsec as u64)
    }

    fn current_time_microseconds() -> Result<u64, String> {
        let mut tv = libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        };
        let rc = unsafe { libc::gettimeofday(&mut tv, ptr::null_mut()) };
        if rc != 0 {
            return Err(format!(
                "gettimeofday failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok((tv.tv_sec as u64) * 1_000_000 + tv.tv_usec as u64)
    }

    fn current_thread_id() -> Result<u32, String> {
        let tid = unsafe { libc::syscall(libc::SYS_gettid) };
        if tid < 0 {
            return Err(format!(
                "syscall(SYS_gettid) failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(tid as u32)
    }

    fn elf_machine_architecture() -> u32 {
        #[cfg(target_arch = "x86_64")]
        {
            62
        }
        #[cfg(target_arch = "x86")]
        {
            3
        }
        #[cfg(target_arch = "aarch64")]
        {
            183
        }
        #[cfg(target_arch = "arm")]
        {
            40
        }
        #[cfg(target_arch = "riscv64")]
        {
            243
        }
    }

    fn write_plain<T>(file: &mut File, value: &T) -> Result<(), String> {
        let bytes = unsafe {
            slice::from_raw_parts(
                (value as *const T).cast::<u8>(),
                std::mem::size_of::<T>(),
            )
        };
        file.write_all(bytes)
            .map_err(|err| format!("failed to write jitdump bytes: {err}"))
    }

    #[cfg(test)]
    mod tests {
        use super::{CodeLoadEvent, Header, JITDUMP_MAGIC, JITDUMP_VERSION, JitDumpSession};
        use std::fs;
        use std::path::PathBuf;
        use std::time::{SystemTime, UNIX_EPOCH};

        fn unique_test_dir() -> PathBuf {
            let mut path = std::env::temp_dir();
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos();
            path.push(format!("soac-jitdump-test-{now}"));
            path
        }

        #[test]
        fn jitdump_session_writes_header_and_code_load_event() {
            let dir = unique_test_dir();
            let mut session = JitDumpSession::new_in_dir(&dir).expect("jitdump session should init");
            let code = [0x90_u8, 0xC3_u8];
            session
                .record_code_load("py:d:test", code.as_ptr(), code.len())
                .expect("jitdump code load should write");

            let dump_path = dir.join(format!("jit-{}.dump", std::process::id()));
            let bytes = fs::read(&dump_path).expect("jitdump file should exist");
            assert!(bytes.len() >= std::mem::size_of::<Header>() + std::mem::size_of::<CodeLoadEvent>() + code.len());

            let header = unsafe { &*(bytes.as_ptr().cast::<Header>()) };
            assert_eq!(header.magic, JITDUMP_MAGIC);
            assert_eq!(header.version, JITDUMP_VERSION);

            let event_offset = std::mem::size_of::<Header>();
            let event = unsafe { &*(bytes[event_offset..].as_ptr().cast::<CodeLoadEvent>()) };
            assert_eq!(event.base.event, 0);
            assert_eq!(event.code_size, code.len() as u64);

            let name_offset = event_offset + std::mem::size_of::<CodeLoadEvent>();
            let name_end = bytes[name_offset..]
                .iter()
                .position(|byte| *byte == 0)
                .expect("jitdump symbol should be null terminated");
            assert_eq!(&bytes[name_offset..name_offset + name_end], b"py:d:test");
            let code_offset = name_offset + name_end + 1;
            assert_eq!(&bytes[code_offset..code_offset + code.len()], &code);

            let _ = fs::remove_file(&dump_path);
            let _ = fs::remove_dir(&dir);
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use imp::record_code_load;

#[cfg(not(target_os = "linux"))]
pub(crate) fn record_code_load(
    _name: &str,
    _code_ptr: *const u8,
    _code_size: usize,
) -> Result<(), String> {
    Ok(())
}
