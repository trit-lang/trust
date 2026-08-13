//! Target descriptions (TIR §7).
//!
//! A declarative record consumed by legalization and codegen. Every field is
//! target-supplied data, never an assumption baked into a pass — `addr_unit`
//! need not be 9 and `word` need not be a power of three.

use std::collections::BTreeSet;

/// A parsed target description.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TargetDesc {
    /// The target's name, as written in a module header.
    pub name: String,
    /// Trits per addressable unit.
    pub addr_unit: u32,
    /// Trits in an address.
    pub ptr_width: u32,
    /// Widths with native operation support — the legal set.
    pub legal: Vec<u32>,
    /// Preferred/register width.
    pub word: u32,
    /// Symbolic calling convention name.
    pub call_conv: String,
}

impl TargetDesc {
    /// The reference target: the Tritium VM implementing TRISC-27 (TIR §7).
    pub fn tritium() -> TargetDesc {
        TargetDesc {
            name: "tritium".into(),
            addr_unit: 9,
            ptr_width: 27,
            legal: vec![1, 9, 27],
            word: 27,
            call_conv: "tritium0".into(),
        }
    }

    /// The smallest legal width at least `width`, if any.
    pub fn legal_at_least(&self, width: u32) -> Option<u32> {
        self.legal.iter().copied().filter(|&w| w >= width).min()
    }

    /// The largest legal width.
    pub fn widest_legal(&self) -> u32 {
        self.legal.iter().copied().max().unwrap_or(self.word)
    }

    /// Is this width natively supported?
    pub fn is_legal(&self, width: u32) -> bool {
        self.legal.contains(&width)
    }

    /// Check the §7 constraints. Returns the problems found.
    pub fn check(&self) -> Vec<String> {
        let mut errs = Vec::new();
        if self.addr_unit == 0 {
            errs.push("`addr_unit` must be at least 1".into());
        }
        if self.legal.is_empty() {
            errs.push("`legal` must not be empty".into());
        }
        if !self.is_legal(self.word) {
            errs.push(format!(
                "`legal` must contain `word` ({}), but is {:?}",
                self.word, self.legal
            ));
        }
        if !self.legal.iter().any(|&w| w >= self.ptr_width) {
            errs.push(format!(
                "`legal` must contain at least one width >= `ptr_width` ({})",
                self.ptr_width
            ));
        }
        let unique: BTreeSet<u32> = self.legal.iter().copied().collect();
        if unique.len() != self.legal.len() {
            errs.push("`legal` lists a width more than once".into());
        }
        errs
    }
}

/// Parse a target description in the §7 syntax.
pub fn parse_target(src: &str) -> Result<TargetDesc, String> {
    let mut name = None;
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut in_body = false;

    for (n, raw) in src.lines().enumerate() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let at = |m: &str| format!("line {}: {m}", n + 1);
        if !in_body {
            let rest = line
                .strip_prefix("target")
                .ok_or_else(|| at("expected `target \"name\" {`"))?
                .trim();
            let rest = rest
                .strip_prefix('"')
                .ok_or_else(|| at("target name must be quoted"))?;
            let (n2, rest) = rest
                .split_once('"')
                .ok_or_else(|| at("unterminated target name"))?;
            if rest.trim() != "{" {
                return Err(at("expected `{` after the target name"));
            }
            name = Some(n2.to_string());
            in_body = true;
            continue;
        }
        if line == "}" {
            in_body = false;
            continue;
        }
        let (k, v) = line
            .split_once('=')
            .ok_or_else(|| at("expected `field = value`"))?;
        fields.push((k.trim().to_string(), v.trim().to_string()));
    }
    if in_body {
        return Err("unterminated target description".into());
    }
    let name = name.ok_or("no target description found")?;

    let get = |k: &str| -> Result<&str, String> {
        fields
            .iter()
            .find(|(f, _)| f == k)
            .map(|(_, v)| v.as_str())
            .ok_or_else(|| format!("target description is missing `{k}`"))
    };
    let num = |k: &str| -> Result<u32, String> {
        get(k)?
            .parse::<u32>()
            .map_err(|_| format!("`{k}` must be a non-negative integer"))
    };

    let legal_src = get("legal")?;
    let legal_body = legal_src
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or("`legal` must be a bracketed list, e.g. [1, 9, 27]")?;
    let legal = legal_body
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<u32>()
                .map_err(|_| format!("`{s}` is not a width"))
        })
        .collect::<Result<Vec<u32>, String>>()?;

    let call_conv = get("call_conv")?.trim_matches('"').to_string();

    let desc = TargetDesc {
        name,
        addr_unit: num("addr_unit")?,
        ptr_width: num("ptr_width")?,
        legal,
        word: num("word")?,
        call_conv,
    };
    match desc.check().as_slice() {
        [] => Ok(desc),
        problems => Err(problems.join("; ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_spec_example() {
        let src = r#"
target "tritium" {
    addr_unit   = 9          ; trits per addressable unit (tryte)
    ptr_width   = 27         ; trits in an address
    legal       = [1, 9, 27] ; widths with native operation support
    word        = 27         ; preferred/register width
    call_conv   = "tritium0" ; symbolic; defined in the target's own doc
}
"#;
        assert_eq!(parse_target(src).unwrap(), TargetDesc::tritium());
    }

    #[test]
    fn word_must_be_legal() {
        let src = "target \"x\" {\n addr_unit = 9\n ptr_width = 27\n legal = [9, 27]\n word = 12\n call_conv = \"c\"\n}";
        assert!(
            parse_target(src)
                .unwrap_err()
                .contains("must contain `word`")
        );
    }

    #[test]
    fn a_twelve_trit_word_is_expressible() {
        // TIR §7: `word` need *not* be a power of three — the SBTCVM Gen 3
        // lesson, learned before writing line one of a backend.
        let src = "target \"sbtcvm\" {\n addr_unit = 6\n ptr_width = 12\n legal = [1, 6, 12]\n word = 12\n call_conv = \"sbtcvm0\"\n}";
        let t = parse_target(src).unwrap();
        assert_eq!(t.word, 12);
        assert!(t.check().is_empty());
    }
}
