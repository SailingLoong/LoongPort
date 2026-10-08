//! `.env` 补丁器（Gemini CLI 的 `~/.gemini/.env`）：按行处理，注释、空行、
//! 认不出的行和其他变量的顺序都原样保留。

use std::collections::HashSet;
use std::path::Path;

use super::{decode_utf8, LivePatch, LiveWriteError};

#[derive(Clone, Default)]
pub struct DotenvPatch {
    /// 命中谓词的变量先全部删掉（关键字段）；`set` 里有的留给它原位改值。
    pub clear: Option<fn(&str) -> bool>,
    /// 目标值：第一处原位改写（保留 `export ` 前缀），重复的其余行删掉；没有就追加。
    /// 值按原样写成 `KEY=value`，和 CC Switch 现有的写法一致。
    pub set: Vec<(String, String)>,
    /// 当前值（去掉引号后）等于其中之一才删除。`set` 里有同名变量时跳过。
    pub remove_if: Vec<(String, Vec<String>)>,
    /// 按名删掉（所有重复的行）。`set` 里有同名变量时跳过。
    pub remove: Vec<String>,
}

#[derive(Debug, Clone)]
struct Line {
    raw: String,
    key: Option<String>,
    ending: &'static str,
}

impl LivePatch for DotenvPatch {
    fn apply(&self, path: &Path, pre: Option<&[u8]>) -> Result<Vec<u8>, LiveWriteError> {
        let text = match pre {
            Some(bytes) => decode_utf8(path, bytes)?,
            None => "",
        };
        let separator = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let trailing_newline = pre.is_none() || text.is_empty() || text.ends_with('\n');
        let (mut lines, quote_error) = logical_lines(text);
        if let Some(line) = quote_error {
            return Err(LiveWriteError::Parse {
                path: path.to_path_buf(),
                line,
                column: 1,
                message: "unterminated or ambiguous quoted dotenv value".into(),
            });
        }

        let targets: HashSet<&str> = self.set.iter().map(|(key, _)| key.as_str()).collect();

        if let Some(is_floor) = self.clear {
            lines.retain(|line| {
                line.key
                    .as_deref()
                    .is_none_or(|key| !is_floor(key) || targets.contains(key))
            });
        }

        for (key, value) in &self.set {
            let mut seen = false;
            lines.retain_mut(|line| {
                if line.key.as_deref() != Some(key.as_str()) {
                    return true;
                }
                if seen {
                    return false;
                }
                seen = true;
                let export = if line.raw.trim_start().starts_with("export ") {
                    "export "
                } else {
                    ""
                };
                line.raw = format!("{export}{key}={value}");
                true
            });
            if !seen {
                if let Some(last) = lines.last_mut() {
                    if last.ending.is_empty() {
                        last.ending = separator;
                    }
                }
                lines.push(Line {
                    raw: format!("{key}={value}"),
                    key: Some(key.clone()),
                    ending: if trailing_newline { separator } else { "" },
                });
            }
        }

        lines.retain(|line| {
            line.key.as_deref().is_none_or(|key| {
                targets.contains(key) || !self.remove.iter().any(|doomed| doomed == key)
            })
        });

        for (key, values) in &self.remove_if {
            if targets.contains(key.as_str()) {
                continue;
            }
            lines.retain(|line| {
                line.key.as_deref() != Some(key.as_str())
                    || !values
                        .iter()
                        .any(|value| unquote(value_of(&line.raw)) == value)
            });
        }

        let mut out = String::new();
        for line in lines {
            out.push_str(&line.raw);
            out.push_str(line.ending);
        }
        Ok(out.into_bytes())
    }
}

/// 文件里的变量和值（值原样，不去引号），按第一次出现的位置排；重复定义时取最后一个
/// 值（和 dotenv 解析的结果一致）。
pub fn entries(text: &str) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = Vec::new();
    // A malformed quoted tail remains one opaque logical assignment; never
    // expose apparent inner assignments as separate editable fields.
    for line in logical_lines(text).0 {
        let Some(key) = line.key else {
            continue;
        };
        let value = value_of(&line.raw).to_string();
        match entries.iter_mut().find(|(existing, _)| *existing == key) {
            Some(entry) => entry.1 = value,
            None => entries.push((key, value)),
        }
    }
    entries
}

fn quoted_values() -> &'static regex::Regex {
    static QUOTED: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    QUOTED.get_or_init(|| {
        regex::Regex::new(r#"(?s)^(?:'(?:\\'|[^'])*'|"(?:\\"|[^"])*"|`(?:\\`|[^`])*`)"#)
            .expect("constant dotenv quote pattern")
    })
}

/// Read only literal values of caller-owned assignments. The editing parser's
/// broader grouping remains authoritative; unsupported assignment spelling,
/// escapes or expansion produce an unknown value rather than a false fact.
pub(crate) fn literal_owned_entries(
    text: &str,
    owns: fn(&str) -> bool,
) -> Option<Vec<(String, Option<String>)>> {
    let (lines, error) = logical_lines(text);
    if error.is_some() {
        return None;
    }
    let mut result: Vec<(String, Option<String>)> = Vec::new();
    for line in lines {
        let Some((key, offset)) = assignment_parts(&line.raw) else {
            continue;
        };
        if !owns(key) {
            continue;
        }
        let value = if parse_key(&line.raw) == Some(key) {
            literal_value(&line.raw[offset..])
        } else {
            None
        };
        if let Some(entry) = result.iter_mut().find(|(existing, _)| existing == key) {
            entry.1 = value;
        } else {
            result.push((key.to_owned(), value));
        }
    }
    Some(result)
}

fn literal_value(raw: &str) -> Option<String> {
    if raw
        .trim_start_matches([' ', '\t'])
        .starts_with(['\n', '\r'])
    {
        return None;
    }
    let value = raw.trim();
    if value.starts_with(['\'', '"', '`']) {
        let quoted = quoted_values().find(value)?;
        let tail = value[quoted.end()..].trim();
        if !tail.is_empty() && !tail.starts_with('#') {
            return None;
        }
        let literal = &value[1..quoted.end() - 1];
        if literal.contains(['\\', '$']) {
            return None;
        }
        Some(literal.to_owned())
    } else if value.contains(['\\', '$', '#']) {
        None
    } else {
        Some(value.to_owned())
    }
}

/// Preserve logical quoted assignments as indivisible blocks. The quote grammar
/// follows dotenv's single/double/backtick alternatives; raw bytes are never decoded.
fn logical_lines(text: &str) -> (Vec<Line>, Option<usize>) {
    let quoted = quoted_values();
    let mut lines = Vec::new();
    let mut cursor = 0;
    let mut error = None;
    while cursor < text.len() {
        let remaining = &text[cursor..];
        let physical_end = remaining.find('\n').map_or(text.len(), |i| cursor + i + 1);
        let first = &text[cursor..physical_end];
        let mut end = physical_end;
        if let Some(offset) = assignment_value_offset(first) {
            let value = text[cursor + offset..].trim_start();
            if value.starts_with(['\'', '"', '`']) {
                if let Some(found) = quoted.find(value) {
                    let close = text.len() - value.len() + found.end();
                    end = text[close..]
                        .find('\n')
                        .map_or(text.len(), |i| close + i + 1);
                    let tail = text[close..end].trim();
                    if !tail.is_empty() && !tail.starts_with('#') {
                        error = Some(text[..cursor].bytes().filter(|b| *b == b'\n').count() + 1);
                    }
                } else {
                    error = Some(text[..cursor].bytes().filter(|b| *b == b'\n').count() + 1);
                }
                if error.is_some() {
                    end = text.len();
                }
            }
        }
        let block = &text[cursor..end];
        let (raw, ending) = if let Some(raw) = block.strip_suffix("\r\n") {
            (raw, "\r\n")
        } else if let Some(raw) = block.strip_suffix('\n') {
            (raw, "\n")
        } else {
            (block, "")
        };
        lines.push(Line {
            raw: raw.into(),
            key: parse_key(first).map(str::to_string),
            ending,
        });
        cursor = end;
    }
    (lines, error)
}

/// Accept the broader dotenv key spelling for grouping only, so an unowned
/// dotted/dashed or colon-style assignment cannot expose inner apparent keys.
fn assignment_value_offset(line: &str) -> Option<usize> {
    assignment_parts(line).map(|(_, offset)| offset)
}

fn assignment_parts(line: &str) -> Option<(&str, usize)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return None;
    }
    let body = trimmed
        .strip_prefix("export")
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .map(str::trim_start)
        .unwrap_or(trimmed);
    let separator = body.find(['=', ':'])?;
    let key = body[..separator].trim_end();
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return None;
    }
    let value = &body[separator + 1..];
    if body.as_bytes()[separator] == b':' && !value.starts_with(char::is_whitespace) {
        return None;
    }
    Some((key, line.len() - value.len()))
}

/// `KEY=...` 或 `export KEY=...` 里的变量名；认不出的行返回 `None`，原样保留。
fn parse_key(raw: &str) -> Option<&str> {
    let line = raw.trim_start();
    if line.starts_with('#') {
        return None;
    }
    let line = line.strip_prefix("export ").unwrap_or(line);
    let (key, _) = line.split_once('=')?;
    let key = key.trim();
    (!key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).then_some(key)
}

fn value_of(raw: &str) -> &str {
    raw.split_once('=').map_or("", |(_, value)| value.trim())
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::floor;

    fn apply(patch: &DotenvPatch, pre: Option<&str>) -> String {
        let out = patch
            .apply(Path::new(".env"), pre.map(str::as_bytes))
            .expect("apply");
        String::from_utf8(out).expect("utf8")
    }

    #[test]
    fn key_fields_change_and_other_lines_stay_in_order() {
        let patch = DotenvPatch {
            clear: Some(floor::gemini_floor_env),
            set: vec![("GEMINI_API_KEY".into(), "key-b".into())],
            ..DotenvPatch::default()
        };
        let pre = "# sandbox\nGEMINI_SANDBOX=docker\nexport GEMINI_API_KEY=\"key-a\"\nGOOGLE_GEMINI_BASE_URL=https://a.example\nDEBUG=1\nGEMINI_API_KEY=dup\n";
        assert_eq!(
            apply(&patch, Some(pre)),
            "# sandbox\nGEMINI_SANDBOX=docker\nexport GEMINI_API_KEY=key-b\nDEBUG=1\n"
        );
    }

    #[test]
    fn missing_keys_are_appended_and_crlf_is_kept() {
        let patch = DotenvPatch {
            set: vec![("GEMINI_MODEL".into(), "m".into())],
            ..DotenvPatch::default()
        };
        assert_eq!(
            apply(&patch, Some("DEBUG=1\r\n")),
            "DEBUG=1\r\nGEMINI_MODEL=m\r\n"
        );
        assert_eq!(apply(&patch, Some("DEBUG=1")), "DEBUG=1\nGEMINI_MODEL=m");
        assert_eq!(apply(&patch, None), "GEMINI_MODEL=m\n");
    }

    #[test]
    fn remove_drops_every_line_of_the_key_and_entries_take_the_last_value() {
        let patch = DotenvPatch {
            remove: vec!["X".into()],
            ..DotenvPatch::default()
        };
        assert_eq!(apply(&patch, Some("X=1\nY=2\nexport X=3\n")), "Y=2\n");
        assert_eq!(
            entries("# c\nX=1\nY = 2\nexport X=\"3\"\n"),
            vec![
                ("X".to_string(), "\"3\"".to_string()),
                ("Y".to_string(), "2".to_string())
            ]
        );
    }

    #[test]
    fn remove_if_compares_unquoted_values() {
        let patch = DotenvPatch {
            remove_if: vec![("X".into(), vec!["1".into()])],
            ..DotenvPatch::default()
        };
        assert_eq!(apply(&patch, Some("X=\"1\"\nY=2\n")), "Y=2\n");
        assert_eq!(apply(&patch, Some("X=0\n")), "X=0\n");
    }
}
