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
use std::sync::Mutex;

extern "C" {
    fn vsnprintf(buf: *mut c_char, size: usize, format: *const c_char, args: sys::va_list)
        -> c_int;
}

const WORDS: usize = 512;
const MAGIC: u32 = 0x4d46_4c47; // "MFLG"
/// Words 0..3 are magic, write position and the wrapped flag.
const DATA_BYTES: usize = (WORDS - 3) * 4;

/// Not cleared at boot: survives every reset except power loss.
/// Accessed only as whole words (RTC memory on these chips is safest that way).
#[link_section = ".rtc_noinit"]
static mut RTC: [u32; WORDS] = [0; WORDS];

static RTC_LOCK: Mutex<()> = Mutex::new(());

fn word(i: usize) -> *mut u32 {
    // SAFETY: i < WORDS at every call site; only raw pointers are formed.
    unsafe { core::ptr::addr_of_mut!(RTC).cast::<u32>().add(i) }
}

fn get(i: usize) -> u32 {
    // SAFETY: in bounds; RTC words are plain memory.
    unsafe { word(i).read_volatile() }
}

fn set(i: usize, v: u32) {
    // SAFETY: in bounds; writers hold RTC_LOCK.
    unsafe { word(i).write_volatile(v) }
}

fn put(pos: usize, b: u8) {
    let i = 3 + pos / 4;
    let shift = (pos % 4) * 8;
    set(i, (get(i) & !(0xff << shift)) | (u32::from(b) << shift));
}

fn byte(pos: usize) -> u8 {
    (get(3 + pos / 4) >> ((pos % 4) * 8)) as u8
}

/// Appends `L 12.345 text\n` to the RTC copy.
fn mirror(level: u8, t: u32, text: &str) {
    let Ok(_guard) = RTC_LOCK.try_lock() else {
        return;
    };
    if get(0) != MAGIC {
        return;
    }
    let mut pos = get(1) as usize % DATA_BYTES;
    let mut wrapped = get(2) != 0;
    let mut write = |b: u8| {
        put(pos, b);
        pos += 1;
        if pos == DATA_BYTES {
            pos = 0;
            wrapped = true;
        }
    };
    write(logbuf::level_char(level) as u8);
    write(b' ');
    let mut digits = [0u8; 10];
    let mut n = t / 1000;
    let mut k = digits.len();
    loop {
        k -= 1;
        digits[k] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    digits[k..].iter().for_each(|&d| write(d));
    write(b'.');
    let ms = t % 1000;
    write(b'0' + (ms / 100) as u8);
    write(b'0' + (ms / 10 % 10) as u8);
    write(b'0' + (ms % 10) as u8);
    write(b' ');
    text.bytes().take(logbuf::MAX_LINE).for_each(&mut write);
    write(b'\n');
    set(1, pos as u32);
    set(2, wrapped as u32);
}

/// Reads and clears what the previous boot left in RTC memory.
fn recover() -> Option<String> {
    let _guard = RTC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let found = if get(0) == MAGIC {
        let pos = (get(1) as usize).min(DATA_BYTES);
        let wrapped = get(2) != 0;
        let order: Vec<usize> = if wrapped {
            (pos..DATA_BYTES).chain(0..pos).collect()
        } else {
            (0..pos).collect()
        };
        let bytes: Vec<u8> = order.into_iter().map(byte).collect();
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        if wrapped {
            // The oldest line was partly overwritten.
            if let Some(nl) = text.find('\n') {
                text.drain(..=nl);
            }
        }
        Some(text)
    } else {
        None
    };
    set(0, MAGIC);
    set(1, 0);
    set(2, 0);
    found
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
    // SAFETY: "%.*s" with a length and a pointer to that many bytes.
    unsafe { sys::printf(c"%.*s".as_ptr(), len as c_int, buf.as_ptr()) };
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
    // SAFETY: plain IDF calls; the summary is a zeroed POD out-parameter.
    unsafe {
        if sys::esp_core_dump_image_check() != sys::ESP_OK {
            return;
        }
        let mut s: sys::esp_core_dump_summary_t = core::mem::zeroed();
        if sys::esp_core_dump_get_summary(&mut s) == sys::ESP_OK {
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
        }
        sys::esp_core_dump_image_erase();
    }
}
