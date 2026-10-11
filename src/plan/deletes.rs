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
use std::path::{Path, PathBuf};

use super::{Delete, Error, Status};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Deletes(Vec<Delete>);

/// Something with a path that nesting is judged by.
pub(super) trait Located {
    fn path(&self) -> &Path;
}

impl Located for Delete {
    fn path(&self) -> &Path {
        &self.path
    }
}

/// What a scan found at one path, before it becomes a [`Delete`].
pub(super) trait Found: Located {
    /// Takes over what this outer item needs from an item inside it.
    fn absorb(&mut self, inner: Self);
}

impl Deletes {
    /// Keeps only the outermost of nested paths, each absorbing the items inside it, then
    /// makes each a delete with `finish`. The map is keyed by each item's own path, so it
    /// holds each path once.
    pub(super) fn collapsing<F: Found>(
        found: BTreeMap<PathBuf, F>,
        finish: impl Fn(F) -> Delete,
    ) -> Deletes {
        let absorbed = outermost(found.into_values(), |outer: &mut F, inner| {
            outer.absorb(inner);
            Ok::<(), Infallible>(())
        });
        let outermost = match absorbed {
            Ok(outermost) => outermost,
            Err(never) => match never {},
        };
        Deletes(outermost.into_iter().map(finish).collect())
    }

    /// Refuses a path that repeats another or lies inside another.
    pub(super) fn refusing_nested(mut deletes: Vec<Delete>) -> Result<Deletes, Error> {
        deletes.sort_by(|a, b| a.path.cmp(&b.path));

        let outermost = outermost(deletes.into_iter(), |outer: &mut Delete, inner| {
            match inner.path == outer.path {
                true => Err(Error::RepeatedDelete { path: inner.path }),
                false => Err(Error::NestedDeletes {
                    outer: outer.path.clone(),
                    inner: inner.path,
                }),
            }
        })?;
        Ok(Deletes(outermost))
    }

    /// The entries `keep` admits. Dropping entries cannot nest the rest.
    pub(super) fn retaining(&self, keep: impl Fn(&Delete) -> bool) -> Deletes {
        Deletes(self.0.iter().filter(|d| keep(d)).cloned().collect())
    }

    /// The entries with the statuses `status` gives; their paths stay as they were.
    pub(super) fn restatusing(&self, status: impl Fn(&Delete) -> Status) -> Deletes {
        let restatused = self.0.iter().map(|delete| Delete {
            status: status(delete),
            ..delete.clone()
        });
        Deletes(restatused.collect())
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
fn outermost<T: Located, E>(
    sorted: impl ExactSizeIterator<Item = T>,
    mut nested: impl FnMut(&mut T, T) -> Result<(), E>,
) -> Result<Vec<T>, E> {
    let mut outermost: Vec<T> = Vec::with_capacity(sorted.len());
    for item in sorted {
        match outermost.last_mut() {
            Some(outer) if item.path().starts_with(outer.path()) => nested(outer, item)?,
            _ => outermost.push(item),
        }
    }
    Ok(outermost)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::plan::{ItemKind, RunAs, Size, Status};

    /// A found item that is already a delete; siblings never nest, so nothing is absorbed.
    struct Sibling(Delete);

    impl Located for Sibling {
        fn path(&self) -> &Path {
            &self.0.path
        }
    }

    impl Found for Sibling {
        fn absorb(&mut self, _inner: Self) {}
    }

    fn sibling(i: usize) -> Delete {
        Delete {
            path: PathBuf::from(format!("/Users/me/code/project-{i}/node_modules")),
            kind: ItemKind::Folder,
            size: Size::bytes(1),
            rules: vec!["rosie/node".to_owned()],
            status: Status::Clean,
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
        let by_path: BTreeMap<PathBuf, Sibling> = reversed
            .iter()
            .map(|delete| (delete.path.clone(), Sibling(delete.clone())))
            .collect();

        let started = Instant::now();
        let refused = Deletes::refusing_nested(reversed).expect("siblings are not nested");
        let collapsed = Deletes::collapsing(by_path, |sibling| sibling.0);
        let elapsed = started.elapsed();

        assert_eq!(refused.len(), 20_000);
        assert_eq!(collapsed.len(), 20_000);
        assert!(elapsed < Duration::from_millis(50), "took {elapsed:?}");
    }
}
