//! Board side of the served log (`crate::logbuf`):
//!
//! - ESP-IDF's own C log lines (Wi-Fi, esp-tls, mbedTLS, ...) are captured
//!   with `esp_log_set_vprintf`, still printed to the console, and stored.
//! - The newest ~2 KiB of log text is mirrored into RTC memory, which keeps
//!   its contents across a crash, watchdog or software reset. The next boot
//!   serves it as the previous boot's last words.
//! - A Rust panic logs its message (and so reaches RTC memory) before the
//!   abort.
//!
//! - A C-level crash (bad memory access, stack overflow, abort) is saved by
//!   ESP-IDF as a core dump in the `coredump` partition, if the app has one
//!   and `CONFIG_ESP_COREDUMP_ENABLE_TO_FLASH`. The next boot logs its
//!   summary (task, cause, PC, backtrace) and erases it. Map the addresses
//!   to source with `tools/addr2line.sh`.
use crate::logbuf::{self, parse_idf_line};
use core::ffi::{c_char, c_int};
use esp_idf_svc::sys;
use std::io::Write;
use std::sync::Mutex;

extern "C" {
    fn vsnprintf(buf: *mut c_char, size: usize, format: *const c_char, args: sys::va_list)
        -> c_int;
}

use crate::retained_log::{RetainedLog, WordStorage, WORDS};
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
use std::sync::MutexGuard;

/// RTC is retained RAM, not an MMIO register bank. Atomic words give aligned
/// whole-word accesses without raw pointers or uninitialized references.
/// SeqCst preserves word update order; the lock protects multi-word records.
#[link_section = ".rtc_noinit"]
static RTC: [AtomicU32; WORDS] = [const { AtomicU32::new(0) }; WORDS];
const _: () =
    assert!(core::mem::size_of::<AtomicU32>() == 4 && core::mem::align_of::<AtomicU32>() >= 4);
static RTC_LOCK: Mutex<()> = Mutex::new(());

struct RtcWords {
    _guard: MutexGuard<'static, ()>,
}

impl WordStorage for RtcWords {
    fn read(&self, index: usize) -> u32 {
        RTC[index].load(SeqCst)
    }

    fn write(&mut self, index: usize, value: u32) {
        RTC[index].store(value, SeqCst);
    }
}

fn mirror(level: u8, t: u32, text: &str) {
    let Ok(guard) = RTC_LOCK.try_lock() else {
        return;
    };
    RetainedLog(RtcWords { _guard: guard }).mirror(level, t, text);
}

fn recover() -> Option<String> {
    let guard = RTC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    RetainedLog(RtcWords { _guard: guard }).recover()
}

/// ESP-IDF's log output: format once, print, keep.
unsafe extern "C" fn capture(format: *const c_char, args: sys::va_list) -> c_int {
    let mut buf = [0u8; 256];
    // SAFETY: buf is writable for its length; format/args come from esp_log.
    let n = unsafe { vsnprintf(buf.as_mut_ptr().cast(), buf.len(), format, args) };
    if n <= 0 {
        return n;
    }
    let len = (n as usize).min(buf.len() - 1);
    // The VFS console accepts bytes through std::io; no second variadic call.
    let _ = std::io::stdout().write_all(&buf[..len]);
    let text = match core::str::from_utf8(&buf[..len]) {
        Ok(s) => s,
        Err(e) => core::str::from_utf8(&buf[..e.valid_up_to()]).unwrap_or(""),
    };
    let (level, line) = parse_idf_line(text);
    logbuf::push(level, line);
    n
}

/// Installs the logger, the ring, the RTC mirror, the ESP-IDF hook and the
/// panic hook. Call first thing at boot.
pub fn install(ring_bytes: usize) {
    logbuf::install(ring_bytes);
    let previous = recover();
    logbuf::set_mirror(mirror);
    // SAFETY: `capture` matches vprintf's signature and is re-entrant.
    unsafe { sys::esp_log_set_vprintf(Some(capture)) };
    std::panic::set_hook(Box::new(|info| log::error!("panic: {info}")));
    let reason = super::reset_reason();
    match previous.filter(|t| !t.trim().is_empty()) {
        Some(text) => {
            let lines = text.lines().count();
            logbuf::set_previous(reason, text);
            log::info!("boot: reset reason \"{reason}\"; the previous boot's last {lines} lines are kept (/api/v1/log.txt)");
        }
        None => log::info!("boot: reset reason \"{reason}\""),
    }
    #[cfg(feature = "coredump")]
    report_core_dump();
}

#[cfg(feature = "coredump")]
fn exception_name(cause: u32) -> &'static str {
    match cause {
        0 => "IllegalInstruction",
        2 => "InstructionFetchError",
        3 => "LoadStoreError",
        6 => "IntegerDivideByZero",
        9 => "LoadStoreAlignment",
        20 => "InstFetchProhibited",
        28 => "LoadProhibited",
        29 => "StoreProhibited",
        _ => "other (software panic: abort, watchdog, stack overflow...)",
    }
}

/// Logs (as errors) and erases a core dump left by the previous boot.
#[cfg(feature = "coredump")]
fn report_core_dump() {
    // SAFETY: IDF validates the configured core-dump partition; no pointers.
    if unsafe { sys::esp_core_dump_image_check() } != sys::ESP_OK {
        return;
    }
    let mut s = sys::esp_core_dump_summary_t::default();
    // SAFETY: initialized summary writable for the complete SDK struct.
    if unsafe { sys::esp_core_dump_get_summary(&mut s) } == sys::ESP_OK {
        let task: Vec<u8> = s
            .exc_task
            .iter()
            .map(|&c| c as u8)
            .take_while(|&c| c != 0)
            .collect();
        let depth = (s.exc_bt_info.depth as usize).min(s.exc_bt_info.bt.len());
        let backtrace: Vec<String> = s.exc_bt_info.bt[..depth]
            .iter()
            .map(|pc| format!("{pc:#010x}"))
            .collect();
        log::error!(
            "crash report: task \"{}\", pc {:#010x}, cause {} {}, address {:#010x}",
            String::from_utf8_lossy(&task),
            s.exc_pc,
            s.ex_info.exc_cause,
            exception_name(s.ex_info.exc_cause),
            s.ex_info.exc_vaddr
        );
        log::error!(
            "crash backtrace{}: {}",
            if s.exc_bt_info.corrupted {
                " (corrupted)"
            } else {
                ""
            },
            backtrace.join(" ")
        );
    } else {
        log::warn!("a core dump is present but its summary could not be read");
        return; // Preserve unread forensic data for a later boot/tool.
    }
    // SAFETY: only the configured core-dump partition, after reporting it.
    if unsafe { sys::esp_core_dump_image_erase() } != sys::ESP_OK {
        log::warn!("the reported core dump could not be erased");
    }
}
