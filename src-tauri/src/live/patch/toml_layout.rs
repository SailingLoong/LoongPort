//! Compatibility seam for the byte-preservation contract of the upstream TOML owner.
//!
//! toml_edit 0.25 discards statement terminators while parsing and strips CR from
//! raw decoration while displaying. Keep its parser, mutation rules and serializer:
//! source spans identify statement endings; temporary post-mutation markers carry
//! those endings and raw CR through its serializer. No marker reaches patch logic.
//!
//! This seam supports field edits that preserve existing table paths and positions,
//! not arbitrary table moves/reordering or formatting-only CR changes. Such consumers
//! need explicit edit provenance rather than this post-mutation interface.
//! Standard tables have unique logical paths. Array-table members additionally need
//! their parser positions, which custom patches can overwrite. Until an actual
//! projection/editor consumer supplies stronger provenance, only unchanged existing
//! array-table subtrees, removal, and new unpositioned members are supported. An
//! ambiguous edit is refused, never normalized silently. Evaluate this boundary
//! before activating a consumer that edits/repositions existing array members.
//! Moving positioned tables to unknown source paths likewise requires explicit
//! provenance; new tables must be unpositioned rather than impersonating source rows.
//!
//! toml_edit also groups some interleaved dotted statements. A no-op returns the
//! input verbatim. Changed documents require an exact source round-trip through
//! this seam; layouts the existing owner cannot represent are refused. Integrating
//! an editor must surface that limitation rather than silently rewriting the file.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use toml_edit::{Decor, Document, DocumentMut, Item, RawString, Table, TableLike, Value};

use super::{decode_utf8, line_column, LiveWriteError};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Segment {
    Key(String),
    Array(Option<isize>),
}
type Scope = Vec<Segment>;
type LayoutResult<T> = Result<T, &'static str>;

#[derive(Clone, Copy)]
enum Ending {
    Lf,
    CrLf,
    Eof,
}

impl Ending {
    fn after(text: &str, end: usize) -> LayoutResult<Self> {
        let tail = text.get(end..).ok_or("invalid source span")?;
        Ok(match tail.find('\n') {
            None => Self::Eof,
            Some(n) if n > 0 && tail.as_bytes()[n - 1] == b'\r' => Self::CrLf,
            Some(_) => Self::Lf,
        })
    }
}

#[derive(Default)]
struct TableLayout {
    header: Option<Ending>,
    position: Option<isize>,
    values: HashMap<Vec<String>, Ending>,
}

#[derive(Default)]
struct SourceLayout {
    tables: HashMap<Scope, TableLayout>,
    arrays: HashMap<Scope, HashMap<isize, String>>,
}

impl SourceLayout {
    fn collect(
        &mut self,
        source: &Table,
        editable: &Table,
        text: &str,
        scope: &mut Scope,
    ) -> LayoutResult<()> {
        if !source.is_dotted() {
            let mut layout = TableLayout {
                position: source.position(),
                ..Default::default()
            };
            if !scope.is_empty() && !source.is_implicit() {
                if let Some(span) = source.span() {
                    layout.header = Some(Ending::after(text, span.end)?);
                }
            }
            for (keys, value) in source.get_values() {
                let span = value.span().ok_or("missing source value span")?;
                layout.values.insert(
                    keys.iter().map(|key| key.get().to_owned()).collect(),
                    Ending::after(text, span.end)?,
                );
            }
            self.tables.insert(scope.clone(), layout);
        }
        for (key, item) in source.iter() {
            scope.push(Segment::Key(key.to_owned()));
            match item {
                Item::Table(table) => {
                    let other = editable
                        .get(key)
                        .and_then(Item::as_table)
                        .ok_or("source table shape changed before mutation")?;
                    self.collect(table, other, text, scope)?;
                }
                Item::ArrayOfTables(array) => {
                    let other = editable
                        .get(key)
                        .and_then(Item::as_array_of_tables)
                        .ok_or("source array shape changed before mutation")?;
                    for (table, other) in array.iter().zip(other.iter()) {
                        let position = table.position().ok_or("missing source array position")?;
                        self.arrays
                            .entry(scope.clone())
                            .or_default()
                            .insert(position, subtree_text(other));
                        scope.push(Segment::Array(Some(position)));
                        self.collect(table, other, text, scope)?;
                        scope.pop();
                    }
                }
                _ => {}
            }
            scope.pop();
        }
        Ok(())
    }

    fn check_provenance(&self, table: &Table, scope: &mut Scope) -> LayoutResult<()> {
        for (key, item) in table.iter() {
            scope.push(Segment::Key(key.to_owned()));
            match item {
                Item::Table(table) => {
                    match self.tables.get(scope) {
                        Some(original) if table.position() != original.position => {
                            return Err("existing table position changed; moves and reordering require explicit layout provenance");
                        }
                        None if table.position().is_some() => {
                            return Err("positioned table moved to an unknown source path; explicit layout provenance is required");
                        }
                        _ => {}
                    }
                    self.check_provenance(table, scope)?;
                }
                Item::ArrayOfTables(array) => {
                    let mut seen = HashSet::new();
                    for table in array.iter() {
                        if table.position().is_some() && !self.arrays.contains_key(scope) {
                            return Err("positioned array-table moved to an unknown source path; explicit layout provenance is required");
                        }
                        if let Some(originals) = self.arrays.get(scope) {
                            let rendered = subtree_text(table);
                            match table.position() {
                                Some(position)
                                    if !seen.insert(position)
                                        || originals.get(&position) != Some(&rendered) =>
                                {
                                    return Err("existing array-table layout provenance is uncertain; partial edits and repositioning require explicit provenance");
                                }
                                None if originals.values().any(|old| old == &rendered) => {
                                    return Err(
                                        "an existing array-table may have lost its source position",
                                    );
                                }
                                Some(_) | None => {}
                            }
                        }
                        scope.push(Segment::Array(table.position()));
                        self.check_provenance(table, scope)?;
                        scope.pop();
                    }
                }
                _ => {}
            }
            scope.pop();
        }
        Ok(())
    }
}

fn subtree_text(table: &Table) -> String {
    DocumentMut::from(table.clone()).to_string()
}

struct Markers {
    prefix: String,
}

impl Markers {
    fn new(inputs: &[&str]) -> Self {
        for id in 0u64.. {
            let prefix = format!("__LOONGPORT_TOML_LAYOUT_{id}__");
            if inputs.iter().all(|input| !input.contains(&prefix)) {
                return Self { prefix };
            }
        }
        unreachable!("finite input cannot contain every marker")
    }

    fn protect_raw(&self, raw: &RawString) -> LayoutResult<String> {
        let raw = raw
            .as_str()
            .ok_or("unresolved raw source span after mutation")?;
        Ok(raw.replace('\r', &format!("{}r", self.prefix)))
    }

    fn protect_decor(&self, decor: &mut Decor) -> LayoutResult<()> {
        if let Some(prefix) = decor.prefix() {
            decor.set_prefix(self.protect_raw(prefix)?);
        }
        if let Some(suffix) = decor.suffix() {
            decor.set_suffix(self.protect_raw(suffix)?);
        }
        Ok(())
    }

    fn ending(&self, decor: &mut Decor, ending: Ending) -> LayoutResult<()> {
        let tag = match ending {
            Ending::Lf => return Ok(()),
            Ending::CrLf => 'c',
            Ending::Eof => 'e',
        };
        let suffix = decor
            .suffix()
            .map(|raw| raw.as_str().ok_or("unresolved statement suffix"))
            .transpose()?
            .unwrap_or("");
        decor.set_suffix(format!("{suffix}{}{tag}", self.prefix));
        Ok(())
    }

    fn table(
        &self,
        table: &mut Table,
        scope: &mut Scope,
        source: &SourceLayout,
    ) -> LayoutResult<()> {
        self.protect_decor(table.decor_mut())?;
        if !table.is_dotted() {
            if let Some(layout) = source.tables.get(scope) {
                if let Some(ending) = layout.header {
                    self.ending(table.decor_mut(), ending)?;
                }
                let paths: Vec<Vec<String>> = table
                    .get_values()
                    .into_iter()
                    .map(|(keys, _)| keys.iter().map(|key| key.get().to_owned()).collect())
                    .collect();
                for keys in paths {
                    if let Some(ending) = layout.values.get(&keys) {
                        let value = value_at_mut(table, &keys)
                            .ok_or("emitted value path could not be resolved")?;
                        self.ending(value.decor_mut(), *ending)?;
                    }
                }
            }
        }
        for (mut key, item) in table.iter_mut() {
            self.protect_decor(key.leaf_decor_mut())?;
            self.protect_decor(key.dotted_decor_mut())?;
            scope.push(Segment::Key(key.get().to_owned()));
            match item {
                Item::Table(table) => self.table(table, scope, source)?,
                Item::ArrayOfTables(array) => {
                    for table in array.iter_mut() {
                        scope.push(Segment::Array(table.position()));
                        self.table(table, scope, source)?;
                        scope.pop();
                    }
                }
                Item::Value(value) => self.value(value)?,
                Item::None => {}
            }
            scope.pop();
        }
        Ok(())
    }

    fn value(&self, value: &mut Value) -> LayoutResult<()> {
        self.protect_decor(value.decor_mut())?;
        match value {
            Value::Array(array) => {
                array.set_trailing(self.protect_raw(array.trailing())?);
                for value in array.iter_mut() {
                    self.value(value)?;
                }
            }
            Value::InlineTable(table) => {
                table.set_trailing(self.protect_raw(table.trailing())?);
                for (mut key, value) in table.iter_mut() {
                    self.protect_decor(key.leaf_decor_mut())?;
                    self.protect_decor(key.dotted_decor_mut())?;
                    self.value(value)?;
                }
            }
            // Scalar display_repr() already writes the original representation,
            // including multiline CRLF. Replacing it would alter string contents.
            _ => {}
        }
        Ok(())
    }

    fn serialize(&self, mut doc: DocumentMut, source: &SourceLayout) -> LayoutResult<String> {
        self.table(doc.as_table_mut(), &mut Vec::new(), source)?;
        doc.set_trailing(self.protect_raw(doc.trailing())?);
        self.restore(&doc.to_string())
    }

    fn restore(&self, serialized: &str) -> LayoutResult<String> {
        let mut output = String::with_capacity(serialized.len());
        let mut rest = serialized;
        while let Some((before, after)) = rest.split_once(&self.prefix) {
            output.push_str(before);
            if let Some(after) = after.strip_prefix('r') {
                output.push('\r');
                rest = after;
            } else if let Some(after) = after.strip_prefix("c\n") {
                output.push_str("\r\n");
                rest = after;
            } else if let Some(after) = after.strip_prefix("e\n") {
                // An old EOF row needs a separator if new content follows it.
                if !after.is_empty() {
                    output.push('\n');
                }
                rest = after;
            } else {
                return Err("unexpected temporary layout marker");
            }
        }
        output.push_str(rest);
        Ok(output)
    }
}

fn value_at_mut<'a>(table: &'a mut Table, keys: &[String]) -> Option<&'a mut Value> {
    let (last, parents) = keys.split_last()?;
    let mut table: &mut dyn TableLike = table;
    for key in parents {
        table = table.get_mut(key)?.as_table_like_mut()?;
    }
    table.get_mut(last)?.as_value_mut()
}

fn layout_error(path: &Path, message: &'static str) -> LiveWriteError {
    LiveWriteError::Io {
        path: path.to_owned(),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Cannot preserve TOML byte layout: {message}"),
        ),
    }
}

pub(super) fn render(
    path: &Path,
    doc: DocumentMut,
    pre: Option<&[u8]>,
) -> Result<Vec<u8>, LiveWriteError> {
    let before = pre
        .map(|bytes| decode_utf8(path, bytes))
        .transpose()?
        .unwrap_or("");
    let original = Document::parse(before).map_err(|err| {
        let (line, column) = line_column(before, err.span().map_or(0, |span| span.start));
        LiveWriteError::Parse {
            path: path.to_owned(),
            line,
            column,
            message: err.message().to_owned(),
        }
    })?;
    let editable = original.clone().into_mut();
    let mut source = SourceLayout::default();
    source
        .collect(
            original.as_table(),
            editable.as_table(),
            before,
            &mut Vec::new(),
        )
        .map_err(|message| layout_error(path, message))?;
    source
        .check_provenance(doc.as_table(), &mut Vec::new())
        .map_err(|message| layout_error(path, message))?;
    let expected = doc.to_string();
    let baseline = editable.to_string();
    if expected == baseline {
        return Ok(before.as_bytes().to_vec());
    }
    let markers = Markers::new(&[before, &baseline, &expected]);
    let round_trip = markers
        .serialize(editable, &source)
        .map_err(|message| layout_error(path, message))?;
    if round_trip != before {
        return Err(layout_error(path,
            "source layout cannot round-trip exactly (for example, interleaved dotted keys); no output was produced"));
    }
    let output = markers
        .serialize(doc, &source)
        .map_err(|message| layout_error(path, message))?;
    // The seam may change only byte layout. Reparse through the same owner to
    // verify that marker restoration did not change its intended representation.
    let reparsed = Document::parse(output.as_str())
        .map_err(|_| layout_error(path, "rendered output is not valid TOML"))?;
    if reparsed.into_mut().to_string() != expected {
        return Err(layout_error(
            path,
            "rendered output differs from the patched document",
        ));
    }
    Ok(output.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml_edit::{value, Item, Table};

    fn edit(before: &str, change: impl FnOnce(&mut DocumentMut)) -> Result<String, LiveWriteError> {
        let mut doc: DocumentMut = before.parse().unwrap();
        change(&mut doc);
        render(Path::new("synthetic.toml"), doc, Some(before.as_bytes()))
            .map(|bytes| String::from_utf8(bytes).unwrap())
    }

    #[test]
    fn no_op_keeps_interleaved_dotted_statements_in_source_order() {
        let before = "a.x = 1\r\nb = 2\na.y = 3\r\n";
        assert_eq!(edit(before, |_| {}).unwrap(), before);
    }

    #[test]
    fn changed_non_round_trippable_layout_is_refused() {
        let before = "a.x = 1\r\nb = 2\na.y = 3\r\n";
        assert!(edit(before, |doc| {
            doc.insert("new", value(4));
        })
        .is_err());
    }

    #[test]
    fn dotted_and_quoted_paths_and_trailing_comments_round_trip() {
        for before in [
            "# only\r\n\n# final",
            "\r\n\n",
            "a.'b.c' = 1\r\na.d = 2\n# tail\r\n",
            "['a.b'.c] # h\r\nx = '''line\r\nline''' # e\r\n",
            "[a]\r\n[a.b]\nx = 1\r\n[a.c]\r\ny = 2",
            "[[a]]\r\nx = 1\n[a.b]\r\ny = 2\r\n[[a]]\nx = 3\r\n",
        ] {
            let before = format!("model = 'old'\r\n{before}");
            let after = edit(&before, |doc| {
                let decor = doc["model"].as_value().unwrap().decor().clone();
                doc["model"] = value("new");
                *doc["model"].as_value_mut().unwrap().decor_mut() = decor;
            })
            .unwrap();
            assert_eq!(after, before.replacen("'old'", "\"new\"", 1));
        }
    }

    #[test]
    fn insertion_and_deletion_keep_surviving_table_bytes() {
        let before = "[remove]\r\nx = 1\r\n\r\n[keep]\r\nx = 'keep'\r\n";
        let after = edit(before, |doc| {
            doc.remove("remove");
            let mut new = Table::new();
            new.insert("x", value(2));
            doc.insert("new", Item::Table(new));
        })
        .unwrap();
        assert_eq!(after, "\r\n[keep]\r\nx = 'keep'\r\n\n[new]\nx = 2\n");
    }

    #[test]
    fn repositioned_standard_tables_are_refused_without_explicit_provenance() {
        let before = "[one]\r\nx = 1\r\n[two]\nx = 2\n";
        assert!(edit(before, |doc| {
            doc["one"].as_table_mut().unwrap().set_position(Some(2));
            doc["two"].as_table_mut().unwrap().set_position(Some(1));
        })
        .is_err());
    }

    #[test]
    fn nested_collection_decor_and_repeated_lines_remain_exact() {
        let before = "model = 'old'\r\n# same\r\n# same\n\r\n\narr = [\r\n 1, # same\r\n 2,\n]\r\ninline = {\r\n a.b = 1, # same\r\n c = 2,\n}\r\n";
        let after = edit(before, |doc| {
            *doc["model"].as_value_mut().unwrap() = toml_edit::Value::from("new");
        })
        .unwrap();
        // Replacing this value without keep_layout intentionally changes its spacing only.
        assert_eq!(after, before.replacen("'old'", "\"new\"", 1));
    }

    #[test]
    fn missing_eof_is_preserved_until_a_new_statement_needs_a_separator() {
        assert_eq!(edit("[a] # last", |_| {}).unwrap(), "[a] # last");
        assert_eq!(
            edit("v='old' # last", |doc| {
                doc.insert("new", value(1));
            })
            .unwrap(),
            "v='old' # last\nnew = 1\n"
        );
    }

    #[test]
    fn inserted_array_table_without_source_position_is_generated() {
        let before = "[[items]]\r\nname='old'\r\n";
        let after = edit(before, |doc| {
            let mut table = Table::new();
            table.insert("name", value("new"));
            doc["items"].as_array_of_tables_mut().unwrap().push(table);
        })
        .unwrap();
        assert_eq!(
            after,
            "[[items]]\r\nname='old'\r\n\n[[items]]\nname = \"new\"\n"
        );
    }

    #[test]
    fn modified_existing_array_table_is_refused_when_provenance_is_uncertain() {
        assert!(edit("[[items]]\r\nname='old'\r\n", |doc| {
            doc["items"]
                .as_array_of_tables_mut()
                .unwrap()
                .get_mut(0)
                .unwrap()
                .insert("name", value("new"));
        })
        .is_err());
    }

    #[test]
    fn repositioned_existing_array_tables_are_refused() {
        assert!(edit(
            "[[items]]\r\nname='one'\r\n[[items]]\nname='two'\n",
            |doc| {
                let tables = doc["items"].as_array_of_tables_mut().unwrap();
                tables.get_mut(0).unwrap().set_position(Some(2));
                tables.get_mut(1).unwrap().set_position(Some(1));
            }
        )
        .is_err());
    }

    #[test]
    fn marker_like_user_content_is_never_interpreted() {
        let before = "# __LOONGPORT_TOML_LAYOUT_0__r\r\nmodel = 'old'\r\ntext = '''__LOONGPORT_TOML_LAYOUT_1__e\r\n__LOONGPORT_TOML_LAYOUT_0__c'''\r\n";
        let after = edit(before, |doc| {
            let old = doc["model"].as_value().unwrap().decor().clone();
            doc["model"] = value("new");
            *doc["model"].as_value_mut().unwrap().decor_mut() = old;
        })
        .unwrap();
        assert_eq!(after, before.replacen("'old'", "\"new\"", 1));
    }

    #[test]
    fn moved_positioned_standard_table_is_refused_instead_of_losing_body_crlf() {
        assert!(edit("[old]\r\nx=1\r\n", |doc| {
            let moved = doc.remove("old").unwrap();
            doc.insert("new", moved);
        })
        .is_err());
    }

    #[test]
    fn moved_positioned_array_table_is_refused_instead_of_losing_body_crlf() {
        assert!(edit("[[old]]\r\nx=1\r\n", |doc| {
            let moved = doc.remove("old").unwrap();
            doc.insert("new", moved);
        })
        .is_err());
    }

    #[test]
    fn moved_table_cannot_borrow_an_existing_destination_layout() {
        assert!(edit("[old]\r\nx=1\r\n[new]\ny=2\n", |doc| {
            let moved = doc.remove("old").unwrap();
            doc.insert("new", moved);
        })
        .is_err());
    }

    #[test]
    fn swapped_table_items_cannot_borrow_each_others_line_endings() {
        assert!(edit("[one]\r\nx=1\r\n[two]\ny=2\n", |doc| {
            let one = doc.remove("one").unwrap();
            let two = doc.remove("two").unwrap();
            doc.insert("one", two);
            doc.insert("two", one);
        })
        .is_err());
    }
}
