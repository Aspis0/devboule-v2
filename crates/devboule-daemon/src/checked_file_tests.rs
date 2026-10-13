use std::io::Read;

use super::read_capped;

/// A reader that never ends, and counts what it hands out.
struct Endless {
    handed: u64,
}

impl Read for Endless {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        buffer.fill(b'x');
        self.handed += buffer.len() as u64;
        Ok(buffer.len())
    }
}

#[test]
fn a_read_exactly_at_the_cap_comes_back_whole() {
    let mut source: &[u8] = &[7u8; 16];
    let bytes = read_capped(&mut source, 16).expect("the read");
    assert_eq!(bytes, Some(vec![7u8; 16]));
}

/// Growth after the check is not allocated: the read takes one byte past
/// the cap and no more, however long the file goes on.
#[test]
fn a_read_past_the_cap_is_refused_having_taken_one_byte_past_it() {
    let mut source = Endless { handed: 0 };
    let bytes = read_capped(&mut source, 1024).expect("the read");
    assert_eq!(bytes, None, "over the cap is no bytes");
    assert_eq!(source.handed, 1025, "the read stops one byte past the cap");
}
