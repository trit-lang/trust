//! The textual image format.
//!
//! TRISC-27 §8 reserves the object format, and an image is defined there only
//! as "the trytes it assembled, in address order". A file is bytes, not
//! trytes, so *something* has to say how one becomes the other — and until §8
//! says it, this is a **tool convention**, not a specification.
//!
//! It is textual for the same reason TIR is (TIR §8): a binary encoding is
//! premature, and a format that diffs, greps and pastes into a bug report is
//! worth more right now than a compact one. An image is a whitespace-separated
//! list of tryte values, written in any of the three radices of Language
//! Ch. 1 §3, with `;` comments:
//!
//! ```text
//! ; a word, least significant tryte first
//! 0hDDD  0hDHE  -13
//! ```

use trit_core::{Literal, literal};

/// Why an image could not be read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ImageError {
    /// 1-based line number.
    pub line: u32,
    /// What went wrong.
    pub message: String,
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ImageError {}

/// Parse a textual image into trytes, in address order.
pub fn parse(src: &str) -> Result<Vec<i16>, ImageError> {
    let mut out = Vec::new();
    for (n, raw) in src.lines().enumerate() {
        let line = n as u32 + 1;
        let text = raw.split(';').next().unwrap_or("");
        for token in text.split_whitespace() {
            let value = match literal::parse_literal(token) {
                Ok(Literal::Int { value, .. }) => value,
                Ok(Literal::Trit(t)) => trit_core::Bt::from(t),
                Err(e) => {
                    return Err(ImageError {
                        line,
                        message: e.to_string(),
                    });
                }
            };
            let v = value.to_i128().filter(|v| (-9841..=9841).contains(v));
            match v {
                Some(v) => out.push(v as i16),
                None => {
                    return Err(ImageError {
                        line,
                        message: format!("`{token}` is not a tryte value"),
                    });
                }
            }
        }
    }
    Ok(out)
}

/// Render trytes as a textual image, one word (three trytes) per line in
/// heptavintimal — the notation in which an instruction's fields are legible.
pub fn render(trytes: &[i16]) -> String {
    let mut s = String::new();
    for (i, chunk) in trytes.chunks(3).enumerate() {
        let cells: Vec<String> = chunk
            .iter()
            .map(|&t| {
                let b = trit_core::Bt::from_i128(t as i128);
                format!("0h{}", b.to_hept_string(9))
            })
            .collect();
        s.push_str(&format!("{:<24}; tryte {}\n", cells.join(" "), i * 3));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_text_form() {
        let trytes: Vec<i16> = vec![0, 1, -1, 9841, -9841, 40, -40];
        let text = render(&trytes);
        assert_eq!(parse(&text).unwrap(), trytes);
    }

    #[test]
    fn accepts_every_radix_and_comments() {
        let src = "
; a comment
6 0t1T0 0hDDE   ; six, six, one
";
        assert_eq!(parse(src).unwrap(), vec![6, 6, 1]);
    }

    #[test]
    fn rejects_a_value_too_large_for_a_tryte() {
        let e = parse("9842").unwrap_err();
        assert!(e.message.contains("not a tryte value"), "{e}");
    }
}
