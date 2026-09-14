//! End-to-end tests for git sync, against a real git.
//!
//! The remote is a local bare repository, so these exercise clone, commit,
//! push and pull for real without touching the network. Everything the module
//! promises happens between two working copies and a remote, and none of it was
//! covered before: the suite tested path arithmetic, hashing and URL parsing,
//! and ran git exactly once, to build a repo with no remote and assert the
//! error. The failure that actually costs a user their work, a board that syncs
//! without complaint and loses a card, was invisible to all of it.
//!
//! `shadow_dir_for` resolves into the real user cache directory, keyed by a
//! hash of the remote URL. Each test therefore uses its own temporary remote,
//! so the hashes differ and tests cannot collide, and each wraps the shadow in
//! a `ShadowGuard` that removes it afterwards.

use std::path::{Path, PathBuf};
use std::process::Command;

use kando_core::board::sync::{
    commit_and_push, init_shadow, pull, shadow_dir_for, SyncStatus,
};

/// Run git, and panic with its stderr when it fails.
///
/// Identity is passed per-invocation rather than assumed from global config,
/// which a CI runner generally does not have.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=Kando Test"])
        .args(["-c", "user.email=test@example.com"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap_or_else(|e| panic!("could not run git {args:?}: {e}"));
    assert!(
        output.status.success(),
        "git {args:?} in {} failed:\n{}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Give a repository an identity of its own, for the commits the production
/// code makes itself. It shells out to a bare `git commit`, which fails outright
/// when neither global nor local config names an author.
fn set_identity(repo: &Path) {
    git(repo, &["config", "user.name", "Kando Test"]);
    git(repo, &["config", "user.email", "test@example.com"]);
    git(repo, &["config", "commit.gpgsign", "false"]);
}

/// Removes the shadow clone from the user's cache directory when dropped, so a
/// test run leaves nothing behind.
struct ShadowGuard(PathBuf);

impl Drop for ShadowGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    remote: PathBuf,
    work: PathBuf,
}

impl Fixture {
    fn kando_dir(&self) -> PathBuf {
        self.work.join(".kando")
    }

    /// A second working copy of the same remote, standing in for a teammate.
    fn clone_as(&self, name: &str) -> PathBuf {
        let path = self._tmp.path().join(name);
        git(
            self._tmp.path(),
            &["clone", &self.remote.to_string_lossy(), name],
        );
        set_identity(&path);
        path
    }
}

/// A bare remote plus a working copy with a board on `branch`, already pushed.
///
/// Pass `push: false` to leave the branch absent from the remote, which is the
/// state a board is in before its first sync.
fn fixture(branch: &str, push: bool) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote.git");
    std::fs::create_dir(&remote).unwrap();
    git(&remote, &["init", "--bare"]);

    let work = tmp.path().join("work");
    git(tmp.path(), &["clone", &remote.to_string_lossy(), "work"]);
    set_identity(&work);
    git(&work, &["checkout", "-b", branch]);

    write_card(&work.join(".kando"), "backlog", "1", "First card");
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-m", "initial board"]);
    if push {
        git(&work, &["push", "-u", "origin", branch]);
    }

    Fixture {
        _tmp: tmp,
        remote,
        work,
    }
}

fn write_card(kando_dir: &Path, column: &str, id: &str, title: &str) {
    let dir = kando_dir.join("columns").join(column);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{id}.md")),
        format!("---\nid = \"{id}\"\ntitle = \"{title}\"\n---\n\nbody\n"),
    )
    .unwrap();
}

fn card_path(root: &Path, column: &str, id: &str) -> PathBuf {
    root.join(".kando")
        .join("columns")
        .join(column)
        .join(format!("{id}.md"))
}

/// Start a shadow clone for `fixture`, with an identity and a cleanup guard.
fn shadow_for(fixture: &Fixture, branch: &str) -> (kando_core::board::sync::SyncState, ShadowGuard) {
    let state = init_shadow(&fixture.kando_dir(), branch).expect("init_shadow failed");
    let guard = ShadowGuard(state.shadow_path.clone());
    set_identity(&state.shadow_path);
    (state, guard)
}

#[test]
fn a_shadow_clone_lands_on_the_requested_branch() {
    let f = fixture("kando", true);
    let (state, _guard) = shadow_for(&f, "kando");

    assert!(
        state.shadow_path.join(".git").exists(),
        "no git repository in the shadow at {}",
        state.shadow_path.display()
    );
    assert_eq!(git(&state.shadow_path, &["rev-parse", "--abbrev-ref", "HEAD"]), "kando");
    assert!(
        state.shadow_path.join(".kando/columns/backlog/1.md").exists(),
        "the shadow clone did not bring the board with it"
    );
}

#[test]
fn a_card_written_locally_reaches_the_remote() {
    let f = fixture("kando", true);
    let (mut state, _guard) = shadow_for(&f, "kando");

    write_card(&f.kando_dir(), "backlog", "2", "Second card");
    commit_and_push(&mut state, &f.kando_dir(), "add card 2");

    assert!(state.online, "push failed: {:?}", state.last_error);
    assert_eq!(state.last_error, None);

    // Read it back from a clone nobody has touched, so the assertion is about
    // what the remote holds and not about local state.
    let fresh = f.clone_as("fresh");
    git(&fresh, &["checkout", "kando"]);
    let arrived = card_path(&fresh, "backlog", "2");
    assert!(
        arrived.exists(),
        "card 2 never reached the remote; shadow said online with no error"
    );
    assert!(std::fs::read_to_string(&arrived).unwrap().contains("Second card"));
}

#[test]
fn a_first_push_creates_the_branch_on_the_remote() {
    // The branch exists only locally, so clone_shadow takes its fallback path:
    // clone the default branch, then create the sync branch on top.
    let f = fixture("kando", false);
    let (mut state, _guard) = shadow_for(&f, "kando");

    write_card(&f.kando_dir(), "backlog", "2", "Second card");
    commit_and_push(&mut state, &f.kando_dir(), "first sync");

    assert!(state.online, "first push failed: {:?}", state.last_error);
    let branches = git(&f.remote, &["branch", "--list", "kando"]);
    assert!(
        branches.contains("kando"),
        "the first push did not create the branch on the remote; saw {branches:?}"
    );
}

#[test]
fn a_card_pushed_by_someone_else_arrives_on_pull() {
    let f = fixture("kando", true);
    let (mut state, _guard) = shadow_for(&f, "kando");

    // A teammate adds a card and pushes it.
    let mate = f.clone_as("mate");
    git(&mate, &["checkout", "kando"]);
    write_card(&mate.join(".kando"), "backlog", "7", "From a teammate");
    git(&mate, &["add", "-A"]);
    git(&mate, &["commit", "-m", "add card 7"]);
    git(&mate, &["push", "origin", "kando"]);

    assert_eq!(pull(&mut state, &f.kando_dir()), SyncStatus::Updated);

    let arrived = card_path(&f.work, "backlog", "7");
    assert!(
        arrived.exists(),
        "pull reported Updated but the card never reached the working directory"
    );
    assert!(std::fs::read_to_string(&arrived).unwrap().contains("From a teammate"));
}

#[test]
fn a_card_deleted_locally_is_deleted_on_the_remote() {
    // The failure worth catching: a sync that reports success while quietly
    // keeping a card the user deleted, or dropping one they did not.
    //
    // Deletion has two independent mechanisms, and either one alone is enough:
    // commit_and_push clears the shadow's .kando/ before copying, and
    // copy_dir_contents removes entries in the destination that are not in the
    // source. Disabling one still passes this test. That is belt and braces in
    // the code rather than a gap in the test, but it is worth knowing before
    // deciding one of them is dead and removing it.
    let f = fixture("kando", true);
    let (mut state, _guard) = shadow_for(&f, "kando");

    write_card(&f.kando_dir(), "backlog", "2", "Doomed");
    commit_and_push(&mut state, &f.kando_dir(), "add card 2");
    assert!(state.online, "setup push failed: {:?}", state.last_error);

    std::fs::remove_file(card_path(&f.work, "backlog", "2")).unwrap();
    commit_and_push(&mut state, &f.kando_dir(), "delete card 2");
    assert!(state.online, "delete push failed: {:?}", state.last_error);

    let fresh = f.clone_as("fresh");
    git(&fresh, &["checkout", "kando"]);
    assert!(
        !card_path(&fresh, "backlog", "2").exists(),
        "the deleted card is still on the remote"
    );
    assert!(
        card_path(&fresh, "backlog", "1").exists(),
        "deleting card 2 took card 1 with it"
    );
}

#[test]
fn pulling_nothing_reports_already_up_to_date() {
    // This is what pins LC_ALL=C in git_base: the status is decided by matching
    // git's own wording, which git translates when the user has a locale set.
    let f = fixture("kando", true);
    let (mut state, _guard) = shadow_for(&f, "kando");

    assert_eq!(pull(&mut state, &f.kando_dir()), SyncStatus::AlreadyUpToDate);
    assert!(state.online);
    assert_eq!(state.last_error, None);
}

#[test]
fn an_unreachable_remote_reports_offline_and_keeps_the_board() {
    let f = fixture("kando", true);
    let (mut state, _guard) = shadow_for(&f, "kando");

    // Losing the remote stands in for losing the network.
    std::fs::remove_dir_all(&f.remote).unwrap();

    assert_eq!(pull(&mut state, &f.kando_dir()), SyncStatus::Offline);
    assert!(!state.online);
    assert!(state.last_error.is_some(), "offline with no error to show");
    assert!(
        card_path(&f.work, "backlog", "1").exists(),
        "going offline destroyed the local board"
    );
}

#[test]
fn committing_nothing_is_not_an_error() {
    // Called on every save in the TUI, so the no-change path has to stay quiet
    // rather than reporting a failed empty commit.
    let f = fixture("kando", true);
    let (mut state, _guard) = shadow_for(&f, "kando");

    commit_and_push(&mut state, &f.kando_dir(), "nothing to do");
    assert_eq!(state.last_error, None);

    let before = git(&state.shadow_path, &["rev-parse", "HEAD"]);
    commit_and_push(&mut state, &f.kando_dir(), "still nothing");
    assert_eq!(
        git(&state.shadow_path, &["rev-parse", "HEAD"]),
        before,
        "an empty commit was made anyway"
    );
}

#[test]
fn shadow_dir_is_scoped_to_the_remote() {
    // Two boards with different remotes must not share a shadow, or one would
    // overwrite the other's cards.
    assert_ne!(
        shadow_dir_for("git@github.com:krfl/one.git"),
        shadow_dir_for("git@github.com:krfl/two.git")
    );
}
