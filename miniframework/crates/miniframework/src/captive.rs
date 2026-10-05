//! The captive-portal DNS answer for a board's setup network (see
//! `esp::WifiSetup`): every name points at the board, so a phone that joins
//! the open setup network shows the board's page. Pure, so it is tested on
//! the desktop. Which paths the app redirects to its setup page is the
//! app's business.

use std::net::Ipv4Addr;

/// The board's address on its own setup network (ESP-IDF's default).
pub const AP_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);

/// A reply to a single-question query: the question, plus one `A` answer
/// pointing at the board when the question asks for `A`/`ANY`.
pub fn dns_reply(query: &[u8]) -> Option<Vec<u8>> {
    if query.len() < 12 || query[2] & 0x80 != 0 || u16::from_be_bytes([query[4], query[5]]) != 1 {
        return None;
    }
    let mut end = 12;
    while end < query.len() && query[end] != 0 {
        end += 1 + query[end] as usize;
    }
    let question_end = end + 5; // zero byte, type, class
    if question_end > query.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([query[end + 1], query[end + 2]]);
    let answer = qtype == 1 || qtype == 255;
    let mut out = Vec::with_capacity(question_end + 16);
    out.extend_from_slice(&query[..2]); // id
    out.extend_from_slice(&[0x81, 0x80]); // response, recursion available
    out.extend_from_slice(&[0, 1, 0, u8::from(answer), 0, 0, 0, 0]);
    out.extend_from_slice(&query[12..question_end]);
    if answer {
        out.extend_from_slice(&[0xC0, 0x0C, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
        out.extend_from_slice(&AP_IP.octets());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(name: &str, qtype: u16) -> Vec<u8> {
        let mut q = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
        for label in name.split('.') {
            q.push(label.len() as u8);
            q.extend_from_slice(label.as_bytes());
        }
        q.push(0);
        q.extend_from_slice(&qtype.to_be_bytes());
        q.extend_from_slice(&[0, 1]);
        q
    }

    #[test]
    fn a_queries_point_at_the_board() {
        let q = query("captive.apple.com", 1);
        let r = dns_reply(&q).unwrap();
        assert_eq!(&r[..2], &[0x12, 0x34]);
        assert_eq!(r[2] & 0x80, 0x80, "is a response");
        assert_eq!(&r[6..8], &[0, 1], "one answer");
        assert_eq!(&r[r.len() - 4..], &AP_IP.octets());
        assert_eq!(&r[12..q.len()], &q[12..]);
    }

    #[test]
    fn other_queries_get_no_answer_and_junk_is_ignored() {
        let r = dns_reply(&query("example.com", 28)).unwrap();
        assert_eq!(&r[6..8], &[0, 0]);
        assert!(dns_reply(&[0; 5]).is_none());
        let mut response = query("x.y", 1);
        response[2] |= 0x80;
        assert!(dns_reply(&response).is_none(), "never answer a response");
        let mut truncated = query("x.y", 1);
        truncated.truncate(15);
        assert!(dns_reply(&truncated).is_none());
    }
}
