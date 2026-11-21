//! The shortest string a simple regular-expression pattern accepts (MEDULLA-CONTRACT-R001.3).
//!
//! Covers the pattern shapes the wire schemas use: literals, escapes (`\d`, `\w`, `\.`),
//! character classes with ranges, groups with alternation, and the quantifiers `{n}`,
//! `{n,m}`, `+`, `*` and `?`. Each atom takes its first member and its minimum count. The
//! caller still checks the result against the compiled pattern, so an unsupported shape only
//! means no candidate.

/// The shortest string `pattern` (anchors stripped) accepts, or `None` for a shape the
/// synthesizer does not cover.
pub fn synthesize(pattern: &str) -> Option<String> {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    let body = body.strip_suffix('$').unwrap_or(body);
    let chars: Vec<char> = body.chars().collect();
    sequence(&chars)
}

fn sequence(chars: &[char]) -> Option<String> {
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        let (atom, next) = atom(chars, index)?;
        let (count, next) = quantifier(chars, next);
        out.push_str(&atom.repeat(count));
        index = next;
    }
    Some(out)
}

/// One atom starting at `index` and the index after it.
fn atom(chars: &[char], index: usize) -> Option<(String, usize)> {
    match chars[index] {
        '[' => class(chars, index + 1),
        '(' => group(chars, index + 1),
        '\\' => chars
            .get(index + 1)
            .map(|escaped| (escape(*escaped).to_string(), index + 2)),
        '.' => Some(("a".to_string(), index + 1)),
        '|' | ')' | '^' | '$' => None,
        literal => Some((literal.to_string(), index + 1)),
    }
}

fn escape(escaped: char) -> char {
    match escaped {
        'd' => '0',
        'w' => 'a',
        's' => ' ',
        other => other,
    }
}

/// The first member of a character class; a negated class takes `a`.
fn class(chars: &[char], start: usize) -> Option<(String, usize)> {
    let close = (start..chars.len()).find(|&at| chars[at] == ']' && at > start)?;
    let member = match chars.get(start)? {
        '^' => 'a',
        '\\' => escape(*chars.get(start + 1)?),
        first => *first,
    };
    Some((member.to_string(), close + 1))
}

/// The first alternative of a group, non-capturing prefix stripped.
fn group(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut depth = 0usize;
    let mut first_bar = None;
    for at in start..chars.len() {
        match chars[at] {
            '(' => depth += 1,
            ')' if depth == 0 => {
                let end = first_bar.unwrap_or(at);
                let inner = chars[start..end]
                    .strip_prefix(&['?', ':'])
                    .unwrap_or(&chars[start..end]);
                return Some((sequence(inner)?, at + 1));
            }
            ')' => depth -= 1,
            '|' if depth == 0 && first_bar.is_none() => first_bar = Some(at),
            _ => {}
        }
    }
    None
}

/// The minimum count of the quantifier at `index` (1 when there is none) and the index after it.
fn quantifier(chars: &[char], index: usize) -> (usize, usize) {
    let (count, next) = match chars.get(index) {
        Some('+') => (1, index + 1),
        Some('*') | Some('?') => (0, index + 1),
        Some('{') => braces(chars, index + 1),
        _ => (1, index),
    };
    let lazy = next > index && chars.get(next) == Some(&'?');
    (count, next + usize::from(lazy))
}

fn braces(chars: &[char], start: usize) -> (usize, usize) {
    let close = (start..chars.len()).find(|&at| chars[at] == '}');
    let Some(close) = close else {
        return (1, start - 1);
    };
    let text: String = chars[start..close].iter().collect();
    let minimum = text.split(',').next().and_then(|n| n.trim().parse().ok());
    (minimum.unwrap_or(1), close + 1)
}

#[cfg(test)]
mod tests {
    use super::synthesize;

    // spec: MEDULLA-CONTRACT-R001.3
    #[test]
    fn synthesizes_the_shortest_match_of_wire_patterns() {
        let cases = [
            ("^v1:[0-9a-f]{64}$", Some(format!("v1:{}", "0".repeat(64)))),
            (
                "^sha256:[0-9a-f]{64}$",
                Some(format!("sha256:{}", "0".repeat(64))),
            ),
            ("^(?:ab|cd)-\\d+$", Some("ab-0".to_string())),
            ("^[a-z][a-z0-9_]{0,63}$", Some("a".to_string())),
            ("^x?y*$", Some(String::new())),
            ("^(unclosed$", None),
        ];
        for (pattern, expected) in cases {
            assert_eq!(synthesize(pattern), expected, "{pattern}");
        }
    }
}
