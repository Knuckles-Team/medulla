//! Comment and literal masks that keep every byte offset and newline of the source.

/// Blank comments and string/char literals (length and newlines preserved).
pub fn code_mask(source: &str) -> Result<String, String> {
    mask(source, true)
}

/// Blank comments only; literals survive.
pub fn comments_mask(source: &str) -> Result<String, String> {
    mask(source, false)
}

fn mask(source: &str, literals: bool) -> Result<String, String> {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let mut index = 0;
    while index < bytes.len() {
        if !source.is_char_boundary(index) {
            index += 1;
            continue;
        }
        match lexical_span(source, index)? {
            Some((end, is_literal)) => {
                if literals || !is_literal {
                    for byte in &mut out[index..end] {
                        if *byte != b'\n' {
                            *byte = b' ';
                        }
                    }
                }
                index = end;
            }
            None => index += 1,
        }
    }
    String::from_utf8(out).map_err(|e| format!("mask produced invalid UTF-8: {e}"))
}

fn lexical_span(source: &str, index: usize) -> Result<Option<(usize, bool)>, String> {
    let bytes = source.as_bytes();
    let rest = &source[index..];
    if rest.starts_with("//") {
        let end = source[index + 2..]
            .find('\n')
            .map_or(source.len(), |n| index + 2 + n);
        return Ok(Some((end, false)));
    }
    if rest.starts_with("/*") {
        return block_comment_end(source, index).map(|end| Some((end, false)));
    }
    // Raw string: (b|c)? r #* "
    let mut cursor = index;
    if matches!(bytes[cursor], b'b' | b'c') {
        cursor += 1;
    }
    if bytes.get(cursor) == Some(&b'r') {
        let mut hashes = 0;
        let mut at = cursor + 1;
        while bytes.get(at) == Some(&b'#') {
            hashes += 1;
            at += 1;
        }
        if bytes.get(at) == Some(&b'"') {
            let terminator = format!("\"{}", "#".repeat(hashes));
            return match source[at + 1..].find(&terminator) {
                Some(n) => Ok(Some((at + 1 + n + terminator.len(), true))),
                None => Err("unterminated Rust raw string".into()),
            };
        }
    }
    if let Some(end) = char_literal_end(source, index) {
        return Ok(Some((end, true)));
    }
    let mut quote = index;
    if matches!(bytes[quote], b'b' | b'c') {
        quote += 1;
    }
    if bytes.get(quote) == Some(&b'"') {
        let mut end = quote + 1;
        while end < bytes.len() {
            match bytes[end] {
                b'\\' => end += 2,
                b'"' => return Ok(Some((end + 1, true))),
                _ => end += 1,
            }
        }
        return Err("unterminated Rust literal".into());
    }
    Ok(None)
}

fn block_comment_end(source: &str, start: usize) -> Result<usize, String> {
    let mut depth = 1;
    let mut cursor = start + 2;
    while cursor < source.len() && depth > 0 {
        let rest = &source[cursor..];
        if rest.starts_with("/*") {
            depth += 1;
            cursor += 2;
        } else if rest.starts_with("*/") {
            depth -= 1;
            cursor += 2;
        } else {
            cursor += 1;
        }
    }
    if depth == 0 {
        Ok(cursor)
    } else {
        Err("unterminated Rust block comment".into())
    }
}

/// End of a char/byte literal opening at `index` (`'a'`, `b'\n'`, `'\u{1F}'`), if one does.
fn char_literal_end(source: &str, index: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut at = index;
    if bytes[at] == b'b' {
        at += 1;
    }
    if bytes.get(at) != Some(&b'\'') {
        return None;
    }
    at += 1;
    let first = source[at..].chars().next()?;
    if first == '\\' {
        at += 1;
        let escaped = source[at..].chars().next()?;
        if escaped == 'u' && bytes.get(at + 1) == Some(&b'{') {
            let close = source[at..].find('}')?;
            at += close + 1;
        } else if escaped == 'x'
            && source
                .get(at + 1..at + 3)
                .is_some_and(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            at += 3;
        } else {
            at += escaped.len_utf8();
        }
    } else if first == '\'' || first == '\n' {
        return None;
    } else {
        at += first.len_utf8();
    }
    (bytes.get(at) == Some(&b'\'')).then_some(at + 1)
}

/// Offset of the closer matching the `opener` at `start`, over an already masked source.
pub fn balanced_span_from(
    masked: &str,
    start: usize,
    opener: u8,
    closer: u8,
) -> Result<usize, String> {
    let mut depth = 0usize;
    for (offset, byte) in masked.as_bytes()[start..].iter().enumerate() {
        if *byte == opener {
            depth += 1;
        } else if *byte == closer {
            depth -= 1;
            if depth == 0 {
                return Ok(start + offset);
            }
        }
    }
    Err(format!(
        "unterminated balanced block starting at position {start}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_keep_offsets_and_newlines() {
        let src = "let a = \"x//y\"; // c\nlet b = 'z'; /* /* n */ */ let c: &'a str;";
        let code = code_mask(src).unwrap();
        assert_eq!(code.len(), src.len());
        assert!(!code.contains("x//y") && !code.contains("// c") && !code.contains("'z'"));
        assert!(
            code.contains("&'a str"),
            "a lifetime is not a char literal: {code}"
        );
        let comments = comments_mask(src).unwrap();
        assert!(
            comments.contains("\"x//y\"") && comments.contains("'z'") && !comments.contains("// c")
        );
        assert_eq!(code.matches('\n').count(), src.matches('\n').count());
    }

    #[test]
    fn raw_strings_and_escapes() {
        let src = "r#\"a \" b\"# 'x' b'\\n' '\\u{1F}' \"q\\\"r\"";
        let code = code_mask(src).unwrap();
        assert!(code.trim().is_empty(), "{code:?}");
    }

    #[test]
    fn unterminated_literals_fail_closed() {
        assert!(code_mask("\"abc").is_err());
        assert!(code_mask("/* abc").is_err());
    }

    #[test]
    fn balanced_span_finds_matching_brace() {
        let src = "{ a { b } c } d";
        assert_eq!(balanced_span_from(src, 0, b'{', b'}').unwrap(), 12);
        assert!(balanced_span_from("{ {", 0, b'{', b'}').is_err());
    }
}
