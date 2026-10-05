//! ECMAScript String.trim used by ZCode's desktop bootstrap and credential keys.
//! Rust's trim differs for BOM and NEL and can select a different account root.
pub(super) fn js_trim(value: &str) -> &str {
    value.trim_matches(|c| {
        matches!(c,
            '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' |
            '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' |
            '\u{205f}' | '\u{3000}' | '\u{feff}')
    })
}
