//! In-place edits of the `config.toml` document. Each edit changes only the key or
//! array entry it names; comments, spelling, and layout elsewhere are kept.
//!
//! Every document edited here has already loaded as a [`super::Config`], so `roots` is
//! an array and `walk` is a table when present.

use toml_edit::{Array, DocumentMut, Item, Value, table, value};

use super::WalkFlag;

// walk flags

pub(super) fn with_flag(document: &DocumentMut, flag: WalkFlag, on: bool) -> DocumentMut {
    let mut edited = document.clone();
    let walk = edited
        .entry("walk")
        .or_insert_with(table)
        .as_table_like_mut()
        .expect("a loaded config's walk is a table");
    walk.insert(flag.field(), value(on));
    edited
}

/// The document without `flag`, or `None` when the document does not set it.
pub(super) fn without_flag(document: &DocumentMut, flag: WalkFlag) -> Option<DocumentMut> {
    let mut edited = document.clone();
    edited
        .get_mut("walk")
        .and_then(Item::as_table_like_mut)
        .and_then(|walk| walk.remove(flag.field()))?;
    Some(edited)
}

// roots

/// The document with `entry` appended to `roots`, laid out like the entries before it.
pub(super) fn with_root(document: &DocumentMut, entry: &str) -> DocumentMut {
    let mut edited = document.clone();
    let roots = edited
        .entry("roots")
        .or_insert_with(|| value(Array::new()))
        .as_array_mut()
        .expect("a loaded config's roots is an array");

    let mut new = Value::from(entry);
    let prefix = next_entry_prefix(roots);
    new.decor_mut().set_prefix(prefix);
    roots.push_formatted(new);
    edited
}

/// The document without the `index`th `roots` entry. Comment lines written above the
/// entry move to what follows it, so no comment is lost.
pub(super) fn without_root(document: &DocumentMut, index: usize) -> DocumentMut {
    let mut edited = document.clone();
    let roots = edited
        .get_mut("roots")
        .and_then(Item::as_array_mut)
        .expect("a loaded config's roots is an array");

    let removed = roots.remove(index);
    let removed_prefix = prefix_of(&removed);
    if removed_prefix.contains('#') {
        carry_comments(roots, index, &removed_prefix);
    }
    edited
}

/// The prefix a new last entry takes: a line of its own at the last entry's indent in a
/// multi-line array, a single space otherwise.
fn next_entry_prefix(roots: &Array) -> String {
    let last_prefix = roots.iter().last().map(prefix_of);
    match last_prefix
        .as_deref()
        .and_then(|prefix| prefix.rsplit_once('\n'))
    {
        Some((_, indent)) => format!("\n{indent}"),
        None if roots.is_empty() => String::new(),
        None => " ".to_string(),
    }
}

/// Puts the comment lines of a removed entry's `removed_prefix` in front of whatever now
/// sits at `index`: the next entry, or the array's closing bracket.
fn carry_comments(roots: &mut Array, index: usize, removed_prefix: &str) {
    let comments = removed_prefix
        .rsplit_once('\n')
        .map_or(removed_prefix, |(comments, _)| comments);

    match roots.get_mut(index) {
        Some(next) => {
            let joined = join_prefixes(comments, &prefix_of(next));
            next.decor_mut().set_prefix(joined);
        }
        None => {
            let trailing = roots.trailing().as_str().unwrap_or_default().to_string();
            roots.set_trailing(join_prefixes(comments, &trailing));
        }
    }
}

/// `comments` (ending at its last comment line) followed by `rest` from its first line
/// break, so `rest`'s own indent and comments stay in place.
fn join_prefixes(comments: &str, rest: &str) -> String {
    let rest_from_break = rest.find('\n').map_or("\n", |start| &rest[start..]);
    format!("{comments}{rest_from_break}")
}

fn prefix_of(value: &Value) -> String {
    value
        .decor()
        .prefix()
        .and_then(|prefix| prefix.as_str())
        .unwrap_or_default()
        .to_string()
}
