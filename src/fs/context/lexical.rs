use std::path::{Component, Path, PathBuf};

use crate::fs::error::Error;

/// Resolves `.` and `..` by text alone, never touching the disk. `..` at `/` stays at
/// `/`, as the kernel does. Relative paths are refused: rosie never guesses a base.
pub fn resolve_dots(path: &Path) -> Result<PathBuf, Error> {
    let relative = || Error::Relative {
        path: path.to_path_buf(),
    };
    if !path.has_root() {
        return Err(relative());
    }

    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => resolved.push(Component::RootDir),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => resolved.push(name),
            Component::Prefix(_) => return Err(relative()),
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(path: &str) -> PathBuf {
        resolve_dots(Path::new(path)).expect("absolute path resolves")
    }

    #[test]
    fn drops_dots_and_applies_parents_by_text() {
        assert_eq!(resolved("/a/./b/../c"), PathBuf::from("/a/c"));
        assert_eq!(resolved("/a/b/../../.."), PathBuf::from("/"));
        assert_eq!(resolved("/../etc"), PathBuf::from("/etc"));
    }

    #[test]
    fn refuses_relative_paths() {
        let result = resolve_dots(Path::new("code/../x"));

        assert!(matches!(result, Err(Error::Relative { .. })));
    }
}
