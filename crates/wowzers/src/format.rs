//! Text helpers for the UI.

/// `1234567` -> `1.2M`
pub fn compact_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}K", n as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{:.1}B", n as f64 / 1e9),
    }
    .replace(".0", "")
}

/// Cuts `s` to at most `max` characters, ending in `…` when cut.
pub fn truncate(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= max {
        return s;
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_compact() {
        assert_eq!(compact_count(999), "999");
        assert_eq!(compact_count(1_000), "1K");
        assert_eq!(compact_count(1_250), "1.2K");
        assert_eq!(compact_count(12_345_678), "12.3M");
    }

    #[test]
    fn truncate_collapses_whitespace_and_marks_the_cut() {
        assert_eq!(truncate("a\n  b", 10), "a b");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("héllo wörld", 5), "héll…");
    }
}
