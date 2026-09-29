//! A plan's delete entries: sorted by path, with no entry at or inside another
//! (`docs/spec/plan.md#plan-file`).
//!
//! A scan collapses nested matches into the outer one; a plan file that carries a nested
//! or repeated entry is refused. Run's user-then-sudo ordering (`docs/spec/plan.md#run`)
//! has no safe order for a pair like a user-owned folder holding a system launch job's
//! plist, since deleting the outer folder first would take the inner item with it before
//! its own turn.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::ops::Deref;
use std::path::PathBuf;

use super::{Delete, Error, RunAs};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Deletes(Vec<Delete>);

impl Deletes {
    /// Keeps only the outermost of nested paths. The outer entry keeps its own rules,
    /// size, and status. It takes over the bootouts of the entries inside it, so their
    /// jobs are unloaded exactly when it is deleted, and runs elevated when any of them
    /// would. The map is keyed by each entry's own path, so it holds each path once.
    pub(super) fn collapsing(deletes: BTreeMap<PathBuf, Delete>) -> Deletes {
        let absorbed = outermost(deletes.into_values(), |outer, inner| {
            absorb(outer, inner);
            Ok::<(), Infallible>(())
        });
        match absorbed {
            Ok(deletes) => deletes,
            Err(never) => match never {},
        }
    }

    /// Refuses a path that repeats another or lies inside another.
    pub(super) fn refusing_nested(mut deletes: Vec<Delete>) -> Result<Deletes, Error> {
        deletes.sort_by(|a, b| a.path.cmp(&b.path));

        outermost(deletes.into_iter(), |outer, inner| {
            match inner.path == outer.path {
                true => Err(Error::RepeatedDelete { path: inner.path }),
                false => Err(Error::NestedDeletes {
                    outer: outer.path.clone(),
                    inner: inner.path,
                }),
            }
        })
    }

    /// The entries `keep` admits. Dropping entries cannot nest the rest.
    pub(super) fn retaining(&self, keep: impl Fn(&Delete) -> bool) -> Deletes {
        Deletes(self.0.iter().filter(|d| keep(d)).cloned().collect())
    }
}

impl Deref for Deletes {
    type Target = [Delete];

    fn deref(&self) -> &[Delete] {
        &self.0
    }
}

/// Hands each of `sorted`, which comes in path order, that lies at or inside an earlier
/// path to `nested`, together with that outer entry. `Path` orders by component, so every
/// path inside another sorts after it with only paths inside the same outer one in
/// between: one comparison with the last outermost entry finds every nesting.
fn outermost<E>(
    sorted: impl ExactSizeIterator<Item = Delete>,
    mut nested: impl FnMut(&mut Delete, Delete) -> Result<(), E>,
) -> Result<Deletes, E> {
    let mut outermost: Vec<Delete> = Vec::with_capacity(sorted.len());
    for delete in sorted {
        match outermost.last_mut() {
            Some(outer) if delete.path.starts_with(&outer.path) => nested(outer, delete)?,
            _ => outermost.push(delete),
        }
    }
    Ok(Deletes(outermost))
}

fn absorb(outer: &mut Delete, inner: Delete) {
    outer.bootouts.extend(inner.bootouts);
    if inner.run_as == RunAs::Sudo {
        outer.run_as = RunAs::Sudo;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::plan::{ItemKind, Size, Status};

    fn sibling(i: usize) -> Delete {
        Delete {
            path: PathBuf::from(format!("/Users/me/code/project-{i}/node_modules")),
            kind: ItemKind::Folder,
            size: Size::bytes(1),
            rules: vec!["rosie/node".to_owned()],
            status: Status::Ticked,
            run_as: RunAs::User,
            bootouts: Vec::new(),
        }
    }

    /// A tree scan of a large code folder finds thousands of entries, and `rosie run`
    /// reads them all back: finding nested entries must not take time that grows with the
    /// square of their number. A quadratic pass over this many entries takes seconds.
    #[test]
    fn finds_nested_entries_among_twenty_thousand_quickly() {
        let reversed: Vec<Delete> = (0..20_000).rev().map(sibling).collect();
        let by_path: BTreeMap<PathBuf, Delete> = reversed
            .iter()
            .map(|delete| (delete.path.clone(), delete.clone()))
            .collect();

        let started = Instant::now();
        let refused = Deletes::refusing_nested(reversed).expect("siblings are not nested");
        let collapsed = Deletes::collapsing(by_path);
        let elapsed = started.elapsed();

        assert_eq!(refused.len(), 20_000);
        assert_eq!(collapsed.len(), 20_000);
        assert!(elapsed < Duration::from_millis(50), "took {elapsed:?}");
    }
}
