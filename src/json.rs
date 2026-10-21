//! Minimal JSON string escaping shared by all CLI/report writers.

pub fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch < ' ' => escaped.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_windows_paths_and_controls() {
        assert_eq!(escape("C:\\"), "C:\\\\");
        assert_eq!(escape("a\"b"), "a\\\"b");
        assert_eq!(escape("a\nb\tc"), "a\\nb\\tc");
        assert_eq!(escape("\u{0001}"), "\\u0001");
    }
}
