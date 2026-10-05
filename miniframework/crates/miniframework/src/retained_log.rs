//! Safe ring logic shared by the RTC adapter and hostile desktop tests.
use crate::logbuf;

pub(crate) const WORDS: usize = 512;
const MAGIC: u32 = 0x4d46_4c47;
const DATA_BYTES: usize = (WORDS - 3) * 4;

pub(crate) trait WordStorage {
    fn read(&self, index: usize) -> u32;
    fn write(&mut self, index: usize, value: u32);
}

pub(crate) struct RetainedLog<W>(pub W);

impl<W: WordStorage> RetainedLog<W> {
    fn byte(&self, pos: usize) -> u8 {
        (self.0.read(3 + pos / 4) >> ((pos % 4) * 8)) as u8
    }

    fn put(&mut self, pos: usize, byte: u8) {
        let index = 3 + pos / 4;
        let shift = (pos % 4) * 8;
        let word = self.0.read(index);
        self.0.write(
            index,
            (word & !(0xff << shift)) | (u32::from(byte) << shift),
        );
    }

    fn metadata(&self) -> Option<(usize, bool)> {
        let pos = self.0.read(1) as usize;
        let wrapped = self.0.read(2);
        (self.0.read(0) == MAGIC && pos < DATA_BYTES && wrapped <= 1).then_some((pos, wrapped == 1))
    }

    pub fn mirror(&mut self, level: u8, t: u32, text: &str) {
        let Some((mut pos, mut wrapped)) = self.metadata() else {
            return;
        };
        let mut write = |byte| {
            self.put(pos, byte);
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
        digits[k..].iter().copied().for_each(&mut write);
        write(b'.');
        write(b'0' + (t % 1000 / 100) as u8);
        write(b'0' + (t % 100 / 10) as u8);
        write(b'0' + (t % 10) as u8);
        write(b' ');
        let mut end = text.len().min(logbuf::MAX_LINE);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.as_bytes()[..end].iter().copied().for_each(&mut write);
        write(b'\n');
        self.0.write(1, pos as u32);
        self.0.write(2, u32::from(wrapped));
    }

    pub fn recover(&mut self) -> Option<String> {
        let found = self.metadata().map(|(pos, wrapped)| {
            let mut bytes = Vec::with_capacity(if wrapped { DATA_BYTES } else { pos });
            if wrapped {
                bytes.extend((pos..DATA_BYTES).map(|i| self.byte(i)));
            }
            bytes.extend((0..pos).map(|i| self.byte(i)));
            let mut text = String::from_utf8_lossy(&bytes).into_owned();
            if wrapped {
                if let Some(nl) = text.find('\n') {
                    text.drain(..=nl);
                } else {
                    text.clear();
                }
            }
            text
        });
        self.0.write(0, MAGIC);
        self.0.write(1, 0);
        self.0.write(2, 0);
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl WordStorage for [u32; WORDS] {
        fn read(&self, index: usize) -> u32 {
            self[index]
        }
        fn write(&mut self, index: usize, value: u32) {
            self[index] = value;
        }
    }

    #[test]
    fn arbitrary_metadata_is_bounded_and_reinitialized() {
        for pos in [0, 1, DATA_BYTES - 1, DATA_BYTES, u32::MAX as usize] {
            for flag in [0, 1, 2, u32::MAX] {
                let mut ring = RetainedLog([u32::MAX; WORDS]);
                ring.0[0] = MAGIC;
                ring.0[1] = pos as u32;
                ring.0[2] = flag;
                let text = ring.recover();
                assert_eq!(text.is_some(), pos < DATA_BYTES && flag <= 1);
                assert_eq!(ring.metadata(), Some((0, false)));
                ring.mirror(logbuf::ERROR, u32::MAX, "recovered");
                assert!(ring.recover().unwrap().ends_with("recovered\n"));
            }
        }
    }

    #[test]
    fn wrap_at_every_position_keeps_complete_newest_lines() {
        for pos in 0..DATA_BYTES {
            let mut ring = RetainedLog([0; WORDS]);
            ring.recover();
            ring.0[1] = pos as u32;
            for _ in 0..12 {
                ring.mirror(logbuf::WARN, 1234, &"é".repeat(200));
            }
            ring.mirror(logbuf::INFO, 5678, "newest");
            let text = ring.recover().unwrap();
            assert!(text.ends_with("I 5.678 newest\n"));
            assert!(!text.contains('\u{fffd}'));
            assert!(text.len() <= DATA_BYTES);
            assert_eq!(ring.recover().as_deref(), Some(""));
        }
    }
}
