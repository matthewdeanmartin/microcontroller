//! gzip (RFC 1952) around miniz_oxide's deflate.
//!
//! Board note: one deflate compressor holds about 230 KiB while it runs
//! (heap, PSRAM on the boards) **and builds a 64 KiB buffer on the stack**
//! (miniz_oxide's `LZOxide`). A board's serving task has a 32 KiB stack, so
//! boards install a [`Runner`] that compresses on a short-lived thread with a
//! big PSRAM stack (`esp::start` does). That is also why dynamic gzip is
//! opt-in per request. Inflate needs about 11 KiB plus the output.
use super::{Error, Result};
use miniz_oxide::deflate::compress_to_vec;
use miniz_oxide::inflate::decompress_to_vec_with_limit;
use std::sync::OnceLock;

/// Runs a compression job somewhere with enough stack. Returns false if it
/// could not (e.g. too little free memory): the response goes uncompressed.
pub type Runner = fn(job: &mut (dyn FnMut() + Send)) -> bool;

static RUNNER: OnceLock<Runner> = OnceLock::new();

/// Installs the board's runner (once; later calls are ignored).
pub fn set_runner(runner: Runner) {
    let _ = RUNNER.set(runner);
}

/// Writes the gzip encoding of `input` into `out`. False means it was not
/// compressed (see [`Runner`]); `out` is then unspecified.
pub fn compress(input: &[u8], level: u8, out: &mut Vec<u8>) -> bool {
    let mut job = || encode(input, level, out);
    match RUNNER.get() {
        Some(run) => run(&mut job),
        None => {
            job();
            true
        }
    }
}

fn encode(input: &[u8], level: u8, out: &mut Vec<u8>) {
    out.clear();
    // Fixed header: magic, deflate, no flags, no mtime, no extra flags, OS unknown.
    out.extend_from_slice(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255]);
    out.extend_from_slice(&compress_to_vec(input, level.clamp(1, 9)));
    out.extend_from_slice(&crc32fast::hash(input).to_le_bytes());
    out.extend_from_slice(&(input.len() as u32).to_le_bytes());
}

/// Decompresses one gzip member, refusing output larger than `limit`.
pub fn decompress(input: &[u8], limit: usize) -> Result<Vec<u8>> {
    if input.len() < 18 || input[0] != 0x1f || input[1] != 0x8b || input[2] != 8 {
        return Err(Error::Invalid("not gzip"));
    }
    let flags = input[3];
    let mut pos = 10;
    if flags & 4 != 0 {
        let extra = u16::from_le_bytes([input[pos], input[pos + 1]]) as usize;
        pos += 2 + extra;
    }
    for bit in [8u8, 16] {
        // FNAME, FCOMMENT: zero-terminated.
        if flags & bit != 0 {
            let end = input
                .get(pos..)
                .and_then(|rest| rest.iter().position(|&b| b == 0))
                .ok_or(Error::Eof)?;
            pos += end + 1;
        }
    }
    if flags & 2 != 0 {
        pos += 2;
    }
    if pos + 8 > input.len() {
        return Err(Error::Eof);
    }
    let body = &input[pos..input.len() - 8];
    let out = decompress_to_vec_with_limit(body, limit).map_err(|e| {
        if matches!(e.status, miniz_oxide::inflate::TINFLStatus::HasMoreOutput) {
            Error::Overflow
        } else {
            Error::Invalid("corrupt gzip body")
        }
    })?;
    let tail = &input[input.len() - 8..];
    let crc = u32::from_le_bytes(tail[..4].try_into().unwrap());
    if crc != crc32fast::hash(&out) {
        return Err(Error::Invalid("gzip checksum mismatch"));
    }
    Ok(out)
}
