//! Why the walk did not enter a folder, and the counts the stats show of it
//! (`docs/spec/safety.md#walk-skips`, `docs/spec/plan.md#stats`).

use std::fmt;

/// Why the walk did not enter a folder (`docs/spec/safety.md#walk-skips`). A closed
/// skip is one its `[walk]` settings would open, and it carries every setting that
/// takes; a sealed one stays closed whatever the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkSkip {
    /// An entry left closed until every setting in `Needs` is on: a folder no rule was
    /// seen to match, since opening it could reveal targets inside, or a fixed path or
    /// leftover location that lies inside a mount the walk may not enter.
    Closed(Needs),
    /// A mount root that is never entered and never a target, whatever the settings:
    /// one a rule matches, since the context never deletes a mount point, or one inside an
    /// item being sized, since sizing never leaves the item's volume.
    SealedMount,
    /// A placeholder that is never opened and never a target, whatever the settings:
    /// one a rule matches, since a match is final, or one inside an item being sized.
    SealedPlaceholder,
    /// `EPERM` or another denial.
    Denied,
    /// With `enter_links = "all"`, a folder a symlink leads to that lies outside the
    /// roots; the skip names the folder, which `rosie roots add` would let in
    /// (`docs/spec/safety.md#enter_links`).
    LinkOutsideRoots,
    /// With `enter_links = "all"`, a symlink to a folder the walk has already walked or
    /// claimed, such as one of its own parents or a second link to one target.
    LinkWalked,
    /// With `enter_links = "all"`, a symlink whose chain of links never ends.
    LinkLoop,
    /// With `enter_links = "all"`, a symlink to nothing.
    LinkDangling,
}

impl WalkSkip {
    /// Why the path was skipped, briefly, as the line naming it shows it
    /// (`docs/spec/plan.md`): each closure of a closed skip, outermost first.
    pub fn reason(self) -> String {
        match self {
            WalkSkip::Closed(needs) => {
                let reasons: Vec<&str> = needs.settings().map(WalkSetting::reason).collect();
                reasons.join("; ")
            }
            WalkSkip::SealedMount => "mount point, never entered or a target".to_owned(),
            WalkSkip::SealedPlaceholder => "cloud placeholder, never opened or a target".to_owned(),
            WalkSkip::Denied => "permission denied".to_owned(),
            WalkSkip::LinkOutsideRoots => "linked to from the roots, but outside them".to_owned(),
            WalkSkip::LinkWalked => "symlink to a folder already walked".to_owned(),
            WalkSkip::LinkLoop => "symlink loop".to_owned(),
            WalkSkip::LinkDangling => "symlink to nothing".to_owned(),
        }
    }

    /// The count this skip adds to: a closed skip counts under its outermost closure.
    fn bucket(self) -> Bucket {
        match self {
            WalkSkip::Closed(needs) => match needs.outermost() {
                WalkSetting::Placeholders => Bucket::Placeholders,
                WalkSetting::Mounts => Bucket::Mounts,
                WalkSetting::Bundles => Bucket::Bundles,
            },
            WalkSkip::SealedMount => Bucket::Mounts,
            WalkSkip::SealedPlaceholder => Bucket::Placeholders,
            WalkSkip::Denied => Bucket::Denied,
            WalkSkip::LinkOutsideRoots
            | WalkSkip::LinkWalked
            | WalkSkip::LinkLoop
            | WalkSkip::LinkDangling => Bucket::Links,
        }
    }
}

/// A `[walk]` setting that opens a kind of closed folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkSetting {
    /// `enter_placeholders`: a dataless cloud placeholder.
    Placeholders,
    /// `enter_mounts`: a mount root on another volume.
    Mounts,
    /// `enter_bundles`: a bundle folder.
    Bundles,
}

impl WalkSetting {
    fn reason(self) -> &'static str {
        match self {
            WalkSetting::Placeholders => "cloud placeholder, not downloaded",
            WalkSetting::Mounts => "another volume",
            WalkSetting::Bundles => "bundle",
        }
    }
}

/// The `[walk]` settings a closed folder needs, all together, before the walk enters
/// it. Never empty: it starts from one setting and only grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Needs {
    placeholders: bool,
    mounts: bool,
    bundles: bool,
}

impl Needs {
    pub const PLACEHOLDERS: Needs = Needs::of(WalkSetting::Placeholders);
    pub const MOUNTS: Needs = Needs::of(WalkSetting::Mounts);
    pub const BUNDLES: Needs = Needs::of(WalkSetting::Bundles);

    pub const fn of(setting: WalkSetting) -> Needs {
        Needs {
            placeholders: matches!(setting, WalkSetting::Placeholders),
            mounts: matches!(setting, WalkSetting::Mounts),
            bundles: matches!(setting, WalkSetting::Bundles),
        }
    }

    /// These settings, and `more` when there is one.
    pub fn and(self, more: Option<WalkSetting>) -> Needs {
        let more = more.map_or(self, Needs::of);
        Needs {
            placeholders: self.placeholders || more.placeholders,
            mounts: self.mounts || more.mounts,
            bundles: self.bundles || more.bundles,
        }
    }

    pub fn contains(self, setting: WalkSetting) -> bool {
        match setting {
            WalkSetting::Placeholders => self.placeholders,
            WalkSetting::Mounts => self.mounts,
            WalkSetting::Bundles => self.bundles,
        }
    }

    /// The settings, outermost first: placeholders, then mounts, then bundles.
    pub fn settings(self) -> impl Iterator<Item = WalkSetting> {
        [
            WalkSetting::Placeholders,
            WalkSetting::Mounts,
            WalkSetting::Bundles,
        ]
        .into_iter()
        .filter(move |setting| self.contains(*setting))
    }

    /// The first of [`Needs::settings`]; there is always one.
    pub fn outermost(self) -> WalkSetting {
        match (self.placeholders, self.mounts) {
            (true, _) => WalkSetting::Placeholders,
            (false, true) => WalkSetting::Mounts,
            (false, false) => WalkSetting::Bundles,
        }
    }
}

/// One count of [`WalkSkips`].
#[derive(Debug, Clone, Copy)]
enum Bucket {
    Bundles,
    Mounts,
    Placeholders,
    Denied,
    Links,
}

impl Bucket {
    /// `3 bundles`, `1 mount`, `2 denied`.
    fn counted(self, count: usize) -> String {
        let (one, many) = match self {
            Bucket::Bundles => ("bundle", "bundles"),
            Bucket::Mounts => ("mount", "mounts"),
            Bucket::Placeholders => ("placeholder", "placeholders"),
            Bucket::Denied => ("denied", "denied"),
            Bucket::Links => ("link", "links"),
        };
        match count {
            1 => format!("{count} {one}"),
            _ => format!("{count} {many}"),
        }
    }
}

/// Walk-skip counts per reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WalkSkips {
    pub bundles: usize,
    pub mounts: usize,
    pub placeholders: usize,
    pub denied: usize,
    pub links: usize,
}

impl WalkSkips {
    pub fn record(&mut self, skip: WalkSkip) {
        let count = match skip.bucket() {
            Bucket::Bundles => &mut self.bundles,
            Bucket::Mounts => &mut self.mounts,
            Bucket::Placeholders => &mut self.placeholders,
            Bucket::Denied => &mut self.denied,
            Bucket::Links => &mut self.links,
        };
        *count += 1;
    }

    pub fn total(&self) -> usize {
        self.bundles + self.mounts + self.placeholders + self.denied + self.links
    }

    fn counts(&self) -> [(Bucket, usize); 5] {
        [
            (Bucket::Bundles, self.bundles),
            (Bucket::Mounts, self.mounts),
            (Bucket::Placeholders, self.placeholders),
            (Bucket::Denied, self.denied),
            (Bucket::Links, self.links),
        ]
    }
}

impl FromIterator<WalkSkip> for WalkSkips {
    fn from_iter<I: IntoIterator<Item = WalkSkip>>(skips: I) -> Self {
        let mut counts = WalkSkips::default();
        for skip in skips {
            counts.record(skip);
        }
        counts
    }
}

/// `walk skipped 3 bundles, 2 denied`; empty when nothing was skipped.
impl fmt::Display for WalkSkips {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.total() == 0 {
            return Ok(());
        }

        let parts: Vec<String> = self
            .counts()
            .into_iter()
            .filter(|(_, count)| *count > 0)
            .map(|(bucket, count)| bucket.counted(count))
            .collect();
        write!(f, "walk skipped {}", parts.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_skips_count_per_reason() {
        let mut skips = WalkSkips::default();
        let seen = [
            WalkSkip::Closed(Needs::BUNDLES),
            WalkSkip::Denied,
            WalkSkip::Closed(Needs::BUNDLES),
            WalkSkip::Closed(Needs::PLACEHOLDERS),
            WalkSkip::LinkLoop,
            WalkSkip::LinkOutsideRoots,
        ];
        for skip in seen {
            skips.record(skip);
        }

        assert_eq!(
            skips,
            WalkSkips {
                bundles: 2,
                mounts: 0,
                placeholders: 1,
                denied: 1,
                links: 2,
            }
        );
        assert_eq!(
            skips.to_string(),
            "walk skipped 2 bundles, 1 placeholder, 1 denied, 2 links"
        );
    }
}
