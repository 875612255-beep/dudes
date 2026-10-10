//! The reply from what the fixed commands printed. Pure: no I/O, so the budgets and
//! the string checks are tested without a repository.

use dudes_proto::{
    GitAgainst, GitBranch, GitChangeKind, GitCommit, GitCommittedFile, GitHead, GitSection,
    GitState, GitView, GitWorkingChange, MAX_GIT_BRANCH_BYTES, MAX_GIT_BRANCHES,
    MAX_GIT_CHANGE_BYTES, MAX_GIT_COMMIT_BYTES, MAX_GIT_COMMITS, MAX_GIT_COMMITTED_BYTES,
};
use dudes_types::Timestamp;

use super::{parse, text};

/// A local branch as `for-each-ref` printed it: its name and its 40-digit tip, as bytes.
pub(super) type Tip = (Vec<u8>, Vec<u8>);

/// The files a range of commits changed, and whether the output was cut.
pub(super) struct Committed {
    pub(super) files: Vec<(GitChangeKind, Vec<u8>)>,
    pub(super) cut: bool,
}

/// What the four steps printed, parsed and not yet checked for display.
pub(super) struct Facts<'a> {
    pub(super) head: &'a parse::Head,
    pub(super) now: Timestamp,
    pub(super) tips: &'a [Tip],
    pub(super) against: GitAgainst,
    pub(super) counts: Option<(u32, u32)>,
    pub(super) commits: &'a [parse::RawCommit],
    pub(super) log_cut: bool,
    pub(super) committed: Option<Committed>,
    pub(super) entries: &'a [parse::StatusEntry],
    pub(super) status_cut: bool,
}

/// The reply for a successful read.
pub(super) fn assemble(facts: &Facts<'_>) -> GitView {
    let current = match facts.head {
        parse::Head::Branch(name) => Some(name.as_bytes()),
        parse::Head::Detached(_) => None,
    };
    let untracked = facts
        .entries
        .iter()
        .filter(|entry| matches!(entry, parse::StatusEntry::Untracked))
        .count();
    let counts = facts.counts;
    GitView {
        state: GitState::Ok,
        read_at: facts.now,
        head: Some(GitHead {
            branch: current.and_then(text::name),
            id: head_id(facts.head, facts.tips),
            ahead: counts.map(|(_, ahead)| ahead),
            behind: counts.map(|(behind, _)| behind),
            against: facts.against,
            untracked: u32::try_from(untracked).unwrap_or(u32::MAX),
            untracked_cut: facts.status_cut,
        }),
        branches: branches(facts.tips, current),
        commits: commits(facts),
        committed: committed(facts.committed.as_ref()),
        changes: changes(facts.entries, facts.status_cut, untracked),
    }
}

fn head_id(head: &parse::Head, tips: &[Tip]) -> Option<String> {
    match head {
        parse::Head::Detached(id) => parse::short(id),
        parse::Head::Branch(name) => tips
            .iter()
            .find(|(candidate, _)| candidate == name.as_bytes())
            .and_then(|(_, tip)| parse::short(tip)),
    }
}

/// The current branch first, then the rest in the order git listed them.
fn branches(tips: &[Tip], current: Option<&[u8]>) -> GitSection<GitBranch> {
    let is_current = |name: &[u8]| Some(name) == current;
    let ordered = tips
        .iter()
        .filter(|(name, _)| is_current(name))
        .chain(tips.iter().filter(|(name, _)| !is_current(name)));
    text::fill(
        ordered.map(|(name, tip)| {
            Some(GitBranch {
                name: text::name(name)?,
                tip: parse::short(tip)?,
                current: is_current(name),
            })
        }),
        MAX_GIT_BRANCHES,
        MAX_GIT_BRANCH_BYTES,
    )
}

/// The largest integer a JavaScript number holds exactly, which the Floor requires of a time.
const MAX_SAFE_MILLIS: i64 = 9_007_199_254_740_991;

fn commits(facts: &Facts<'_>) -> GitSection<GitCommit> {
    let mut section = text::fill(
        facts.commits.iter().map(|commit| {
            if commit.at.unix_millis() > MAX_SAFE_MILLIS {
                return None;
            }
            Some(GitCommit {
                id: commit.id.clone(),
                parents: commit.parents.clone(),
                at: commit.at,
                subject: text::subject(&commit.subject)?,
            })
        }),
        MAX_GIT_COMMITS,
        MAX_GIT_COMMIT_BYTES,
    );
    // `log` was asked for at most twenty, so more than it printed is known from the count.
    let more = facts
        .counts
        .is_some_and(|(_, ahead)| ahead as usize > facts.commits.len());
    section.truncated |= facts.log_cut || more;
    section
}

fn committed(committed: Option<&Committed>) -> GitSection<GitCommittedFile> {
    let Some(Committed { files, cut }) = committed else {
        return GitSection::default();
    };
    let mut section = text::fill(
        files.iter().map(|(kind, path)| {
            let (path, cut) = text::path(path)?;
            Some(GitCommittedFile {
                path,
                cut,
                kind: *kind,
            })
        }),
        usize::MAX,
        MAX_GIT_COMMITTED_BYTES,
    );
    section.truncated |= *cut;
    section
}

fn changes(
    entries: &[parse::StatusEntry],
    status_cut: bool,
    untracked: usize,
) -> GitSection<GitWorkingChange> {
    let mut section = text::fill(
        entries.iter().filter_map(|entry| match entry {
            parse::StatusEntry::Untracked => None,
            parse::StatusEntry::Changed { kind, staged, path } => {
                Some(text::path(path).map(|(path, cut)| GitWorkingChange {
                    path,
                    cut,
                    kind: *kind,
                    staged: *staged,
                }))
            }
        }),
        usize::MAX,
        MAX_GIT_CHANGE_BYTES,
    );
    // Tracked changes come before untracked files in the output, so a cut that has reached an
    // untracked file left every tracked change whole.
    section.truncated |= status_cut && untracked == 0;
    section
}

#[cfg(test)]
mod tests {
    use dudes_proto::MAX_GIT_VIEW_BYTES;

    use super::*;

    fn id(digit: char) -> Vec<u8> {
        std::iter::repeat_n(digit as u8, 40).collect()
    }

    /// Every section as full as it can be, with the strings that grow most when serialized: a
    /// quote and a backslash each cost two bytes, and the path cap is 200 bytes of them.
    #[test]
    fn the_worst_case_reply_stays_under_the_cap() {
        let heavy_path = "\"\\".repeat(150);
        let heavy_name = "\"\\".repeat(60);
        let heavy_subject = "\"\\".repeat(80);
        let tips: Vec<Tip> = (0..200)
            .map(|n| (format!("{heavy_name}{n}").into_bytes(), id('a')))
            .collect();
        let commits: Vec<parse::RawCommit> = (0..20)
            .map(|_| parse::RawCommit {
                id: "a".repeat(12),
                parents: vec!["b".repeat(12); 4],
                at: Timestamp::from_unix_millis(i64::MAX),
                subject: heavy_subject.clone().into_bytes(),
            })
            .collect();
        let files: Vec<(GitChangeKind, Vec<u8>)> = (0..5000)
            .map(|n| {
                (
                    GitChangeKind::TypeChanged,
                    format!("{heavy_path}{n}").into_bytes(),
                )
            })
            .collect();
        let entries: Vec<parse::StatusEntry> = (0..5000)
            .map(|n| parse::StatusEntry::Changed {
                kind: GitChangeKind::Conflicted,
                staged: true,
                path: format!("{heavy_path}{n}").into_bytes(),
            })
            .collect();
        let head = parse::Head::Branch(format!("{heavy_name}0"));
        let view = assemble(&Facts {
            head: &head,
            now: Timestamp::from_unix_millis(i64::MAX),
            tips: &tips,
            against: GitAgainst::RemoteDefaultBranch,
            counts: Some((u32::MAX, u32::MAX)),
            commits: &commits,
            log_cut: true,
            committed: Some(Committed { files, cut: true }),
            entries: &entries,
            status_cut: true,
        });
        for (name, truncated) in [
            ("branches", view.branches.truncated),
            ("commits", view.commits.truncated),
            ("committed", view.committed.truncated),
            ("changes", view.changes.truncated),
        ] {
            assert!(truncated, "{name} was filled to its budget");
        }
        let wire = serde_json::to_vec(&view).unwrap();
        assert!(wire.len() < MAX_GIT_VIEW_BYTES, "{}", wire.len());
        assert!(size(&view.branches) <= dudes_proto::MAX_GIT_BRANCH_BYTES);
        assert!(size(&view.commits) <= dudes_proto::MAX_GIT_COMMIT_BYTES);
        assert!(size(&view.committed) <= dudes_proto::MAX_GIT_COMMITTED_BYTES);
        assert!(size(&view.changes) <= dudes_proto::MAX_GIT_CHANGE_BYTES);
        let head = serde_json::to_vec(&view.head).unwrap();
        assert!(
            head.len() <= dudes_proto::MAX_GIT_HEAD_BYTES,
            "{}",
            head.len()
        );
    }

    fn size(value: &impl serde::Serialize) -> usize {
        serde_json::to_vec(value).unwrap().len()
    }

    #[test]
    fn a_commit_time_past_the_safe_integer_range_drops_the_commit_and_not_the_reply() {
        let raw = |millis| parse::RawCommit {
            id: "a".repeat(12),
            parents: vec![],
            at: Timestamp::from_unix_millis(millis),
            subject: b"s".to_vec(),
        };
        let commits = [raw(1_000), raw(MAX_SAFE_MILLIS + 1), raw(MAX_SAFE_MILLIS)];
        let head = parse::Head::Detached(id('a'));
        let view = assemble(&Facts {
            head: &head,
            now: Timestamp::UNIX_EPOCH,
            tips: &[],
            against: GitAgainst::WorktreeStart,
            counts: None,
            commits: &commits,
            log_cut: false,
            committed: None,
            entries: &[],
            status_cut: false,
        });
        assert_eq!((view.commits.items.len(), view.commits.dropped), (2, 1));
    }

    #[test]
    fn a_hostile_name_drops_one_branch_and_the_rest_stay() {
        let tips: Vec<Tip> = vec![
            (b"main".to_vec(), id('a')),
            ("bad\u{202e}name".as_bytes().to_vec(), id('b')),
            (b"zed".to_vec(), id('c')),
        ];
        let head = parse::Head::Branch("zed".into());
        let view = assemble(&Facts {
            head: &head,
            now: Timestamp::UNIX_EPOCH,
            tips: &tips,
            against: GitAgainst::RemoteDefaultBranch,
            counts: None,
            commits: &[],
            log_cut: false,
            committed: None,
            entries: &[],
            status_cut: false,
        });
        let names: Vec<_> = view
            .branches
            .items
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(names, ["zed", "main"], "the current branch first");
        assert_eq!(view.branches.dropped, 1);
        assert_eq!(view.head.unwrap().id.as_deref(), Some("cccccccccccc"));
    }
}
