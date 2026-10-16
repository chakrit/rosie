//! How the command line prints a path: under home as `~` (`docs/spec/cli.md#check`).

use std::path::Path;

pub struct Shown<'h> {
    pub home: &'h Path,
}

impl Shown<'_> {
    pub fn path(&self, path: &Path) -> String {
        match path.strip_prefix(self.home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_under_home_print_from_tilde() {
        let shown = Shown {
            home: Path::new("/Users/me"),
        };

        assert_eq!(shown.path(Path::new("/Users/me")), "~");
        assert_eq!(shown.path(Path::new("/Users/me/My App")), "~/My App");
        assert_eq!(shown.path(Path::new("/Users/meme")), "/Users/meme");
    }
}
