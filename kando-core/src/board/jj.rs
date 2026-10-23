//! Built-in jj integration, switched on per board with `kando jj on`.
//!
//! When a card enters the In Progress column it gets its own jj change in the
//! board's project repo:
//!
//! - The change is created with `jj new --no-edit`, so the working copy is
//!   left alone. Run `jj edit` on it when you start the work.
//! - Its description is the card title plus a `Kando-Card: <id>` trailer. The
//!   trailer is how a card's change is found again, so nothing is written to
//!   card files and there is no state to keep in sync with the repo.
//! - Its parents are the changes of the card's blockers (done or not), reduced
//!   to their heads so a blocker that already builds on another doesn't add a
//!   redundant merge. With none, it starts from the configured base (`main`).
//! - A card that already has a change keeps it.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::hooks::{send_notification, HookNotification};
use super::Board;
use crate::config::LocalConfig;

/// Revset a card's change starts from when no blocker has a change.
pub const DEFAULT_BASE: &str = "main";
/// Slug of the column whose cards get a change.
pub const TRIGGER_COLUMN: &str = "in-progress";
/// Description trailer that ties a change to its card.
const TRAILER: &str = "Kando-Card";

/// The integration's settings for one board, present only while it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JjSettings {
    /// The jj repo: the board's project root.
    pub repo: PathBuf,
    /// Revset for cards whose blockers have no change.
    pub base: String,
}

impl JjSettings {
    /// `Some` when the board has the integration switched on.
    pub fn from_config(cfg: &LocalConfig, project_root: &Path) -> Option<Self> {
        cfg.jj.then(|| Self {
            repo: project_root.to_path_buf(),
            base: cfg.jj_base.clone().unwrap_or_else(|| DEFAULT_BASE.into()),
        })
    }

    /// The work to do now that `card_id` has been moved, if it landed in
    /// In Progress. Call after the move, with the board as it now is.
    pub fn job_for_move(&self, board: &Board, card_id: &str) -> Option<CardChangeJob> {
        let (col, idx) = board.find_card(card_id)?;
        if board.columns[col].slug != TRIGGER_COLUMN {
            return None;
        }
        let card = &board.columns[col].cards[idx];
        Some(CardChangeJob {
            settings: self.clone(),
            card_id: card.id.clone(),
            title: card.title.clone(),
            blocked_by: card.blocked_by.clone(),
        })
    }
}

/// Give one card its change. Holds copies of everything it needs, so it can
/// run on another thread while the board moves on.
#[derive(Debug, Clone)]
pub struct CardChangeJob {
    settings: JjSettings,
    card_id: String,
    title: String,
    blocked_by: Vec<String>,
}

impl CardChangeJob {
    /// Run it now. Both outcomes are a one-line, user-facing message.
    pub fn run(&self) -> Result<String, String> {
        let repo = &self.settings.repo;
        let id = &self.card_id;
        check_id(id)?;

        if let Some(existing) = change_for(repo, id)? {
            return Ok(format!("jj: #{id} already has change {existing}"));
        }

        let mut parents: Vec<String> = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        for blocker in &self.blocked_by {
            if check_id(blocker).is_err() {
                continue;
            }
            match change_for(repo, blocker)? {
                Some(change) if !parents.contains(&change) => parents.push(change),
                Some(_) => {}
                None => missing.push(format!("#{blocker}")),
            }
        }
        let revset = if parents.is_empty() {
            self.settings.base.clone()
        } else {
            format!("heads({})", parents.join("|"))
        };

        let title = if self.title.trim().is_empty() { format!("card {id}") } else { self.title.clone() };
        let message = format!("{title}\n\n{TRAILER}: {id}\n");
        jj(repo, &["new", "--no-edit", "--quiet", &revset, "-m", &message])?;

        let change = change_for(repo, id)?
            .ok_or_else(|| format!("jj: created a change for #{id} but cannot find it"))?;
        let on = jj(repo, &[
            "log", "--no-graph", "-r", &format!("{change}-"),
            "-T", r#"change_id.short(12) ++ " ""#,
        ])?;
        let mut msg = format!("jj: #{id} -> {change} on {}", on.trim());
        if !missing.is_empty() {
            msg.push_str(&format!(" (no change for {})", missing.join(", ")));
        }
        Ok(msg)
    }

    /// Run it on a background thread and report through the TUI's
    /// notification channel, like a hook.
    pub fn spawn(self) {
        std::thread::spawn(move || {
            let (message, is_error) = match self.run() {
                Ok(m) => (m, false),
                Err(m) => (m, true),
            };
            send_notification(HookNotification { hook_name: "jj".into(), message, is_error });
        });
    }
}

/// Check that `repo` is a jj repo, for `kando jj on`.
pub fn check_repo(repo: &Path) -> Result<(), String> {
    jj(repo, &["root"]).map(|_| ())
}

/// Check that `base` resolves in `repo`, for `kando jj on`.
pub fn check_base(repo: &Path, base: &str) -> Result<(), String> {
    jj(repo, &["log", "--no-graph", "-r", base, "-T", "''"]).map(|_| ())
}

/// The change tagged with `card_id`'s trailer, if there is one.
fn change_for(repo: &Path, card_id: &str) -> Result<Option<String>, String> {
    let revset = format!(r#"description(regex:"(?m)^{TRAILER}: {card_id}$")"#);
    let out = jj(repo, &[
        "log", "--no-graph", "-r", &revset,
        "-T", r#"change_id.short(12) ++ "\n""#,
    ])?;
    Ok(out.lines().next().map(str::to_string))
}

/// Card IDs go into a regex and a revset, so only allow plain ones.
fn check_id(id: &str) -> Result<(), String> {
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        Ok(())
    } else {
        Err(format!("jj: unexpected card ID {id:?}"))
    }
}

/// Run jj in `repo` and return its stdout, or the first line of its error.
fn jj(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("jj")
        .arg("--repository")
        .arg(repo)
        .args(["--no-pager", "--color", "never"])
        .args(args)
        .output()
        .map_err(|e| format!("jj: could not run jj: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let line = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("failed");
    Err(format!("jj: {}", line.trim_start_matches("Error: ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{Card, Column, Policies};

    fn board_with(cards: &[(&str, &str, &[&str])]) -> Board {
        let col = |slug: &str, order: u32| Column {
            slug: slug.into(),
            name: slug.into(),
            order,
            wip_limit: None,
            hidden: false,
            cards: Vec::new(),
        };
        let mut board = Board {
            name: "Test".into(),
            next_card_id: 100,
            policies: Policies::default(),
            sync_branch: None,
            nerd_font: false,
            created_at: None,
            columns: vec![col("backlog", 0), col("in-progress", 1), col("done", 2)],
        };
        for (slug, id, blocked_by) in cards {
            let mut card = Card::new((*id).into(), format!("Card {id}"));
            card.blocked_by = blocked_by.iter().map(|b| b.to_string()).collect();
            let c = board.columns.iter().position(|c| c.slug == *slug).unwrap();
            board.columns[c].cards.push(card);
        }
        board
    }

    fn settings(repo: &Path) -> JjSettings {
        JjSettings { repo: repo.to_path_buf(), base: DEFAULT_BASE.into() }
    }

    #[test]
    fn settings_only_when_switched_on() {
        let root = Path::new("/project");
        assert!(JjSettings::from_config(&LocalConfig::default(), root).is_none());
        let on = LocalConfig { jj: true, ..Default::default() };
        assert_eq!(
            JjSettings::from_config(&on, root),
            Some(JjSettings { repo: root.into(), base: "main".into() })
        );
        let custom = LocalConfig { jj: true, jj_base: Some("trunk()".into()), ..Default::default() };
        assert_eq!(JjSettings::from_config(&custom, root).unwrap().base, "trunk()");
    }

    #[test]
    fn only_moves_into_in_progress_make_a_job() {
        let board = board_with(&[("backlog", "1", &[]), ("in-progress", "2", &["1"]), ("done", "3", &[])]);
        let s = settings(Path::new("/project"));
        assert!(s.job_for_move(&board, "1").is_none());
        assert!(s.job_for_move(&board, "3").is_none());
        assert!(s.job_for_move(&board, "99").is_none());
        let job = s.job_for_move(&board, "2").unwrap();
        assert_eq!(job.card_id, "2");
        assert_eq!(job.title, "Card 2");
        assert_eq!(job.blocked_by, vec!["1"]);
    }

    #[test]
    fn odd_card_ids_are_refused() {
        assert!(check_id("12").is_ok());
        assert!(check_id("a-b_c").is_ok());
        assert!(check_id("").is_err());
        assert!(check_id("1|all()").is_err());
        assert!(check_id("1$").is_err());
    }

    // ── against a real jj repo (skipped when jj isn't installed) ──

    fn jj_available() -> bool {
        Command::new("jj").arg("--version").output().is_ok_and(|o| o.status.success())
    }

    /// A fresh repo with a `main` bookmark and uncommitted work in `@`.
    fn scratch_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        // `--repository` must name an existing repo, so init without it.
        let init = Command::new("jj").args(["git", "init", "--quiet"]).arg(repo).output().unwrap();
        assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));
        std::fs::write(repo.join("f"), "base").unwrap();
        jj(repo, &["describe", "--quiet", "-m", "base"]).unwrap();
        jj(repo, &["bookmark", "create", "--quiet", "main", "-r", "@"]).unwrap();
        jj(repo, &["new", "--quiet"]).unwrap();
        std::fs::write(repo.join("wip"), "wip").unwrap();
        dir
    }

    fn run(repo: &Path, id: &str, blocked_by: &[&str]) -> Result<String, String> {
        CardChangeJob {
            settings: settings(repo),
            card_id: id.into(),
            title: format!("Card {id}"),
            blocked_by: blocked_by.iter().map(|b| b.to_string()).collect(),
        }
        .run()
    }

    /// Change IDs of the parents of `card_id`'s change.
    fn parents_of(repo: &Path, card_id: &str) -> Vec<String> {
        let change = change_for(repo, card_id).unwrap().unwrap();
        let out = jj(repo, &[
            "log", "--no-graph", "-r", &format!("{change}-"),
            "-T", r#"change_id.short(12) ++ "\n""#,
        ]).unwrap();
        let mut ids: Vec<String> = out.lines().map(str::to_string).collect();
        ids.sort();
        ids
    }

    fn change_of_rev(repo: &Path, rev: &str) -> String {
        jj(repo, &["log", "--no-graph", "-r", rev, "-T", "change_id.short(12)"]).unwrap()
    }

    #[test]
    fn creates_tagged_changes_with_blocker_parents() {
        if !jj_available() {
            eprintln!("skipping: jj not installed");
            return;
        }
        let dir = scratch_repo();
        let repo = dir.path();
        let main = change_of_rev(repo, "main");
        let wc_before = change_of_rev(repo, "@");

        // No blockers: starts from main.
        let msg = run(repo, "1", &[]).unwrap();
        assert!(msg.starts_with("jj: #1 -> "), "{msg}");
        assert_eq!(parents_of(repo, "1"), vec![main.clone()]);
        let c1 = change_for(repo, "1").unwrap().unwrap();

        // Running again keeps the existing change.
        assert_eq!(run(repo, "1", &[]).unwrap(), format!("jj: #1 already has change {c1}"));

        // A blocker with a change becomes the parent.
        run(repo, "2", &["1"]).unwrap();
        let c2 = change_for(repo, "2").unwrap().unwrap();
        assert_eq!(parents_of(repo, "2"), vec![c1.clone()]);

        // 1 is an ancestor of 2, so only 2 is kept; 9 has no change.
        let msg = run(repo, "3", &["1", "2", "9"]).unwrap();
        assert!(msg.ends_with("(no change for #9)"), "{msg}");
        assert_eq!(parents_of(repo, "3"), vec![c2.clone()]);

        // Independent blockers make a merge.
        run(repo, "4", &[]).unwrap();
        let c4 = change_for(repo, "4").unwrap().unwrap();
        run(repo, "5", &["2", "4"]).unwrap();
        let mut expected = vec![c2, c4];
        expected.sort();
        assert_eq!(parents_of(repo, "5"), expected);

        // Card 10 is not mistaken for card 1.
        run(repo, "10", &[]).unwrap();
        assert_eq!(parents_of(repo, "10"), vec![main]);

        // The working copy never moved.
        assert_eq!(change_of_rev(repo, "@"), wc_before);
    }

    #[test]
    fn reports_a_base_that_does_not_resolve() {
        if !jj_available() {
            eprintln!("skipping: jj not installed");
            return;
        }
        let dir = scratch_repo();
        let mut job = CardChangeJob {
            settings: settings(dir.path()),
            card_id: "1".into(),
            title: "Card".into(),
            blocked_by: Vec::new(),
        };
        job.settings.base = "no-such-bookmark".into();
        let err = job.run().unwrap_err();
        assert!(err.starts_with("jj: "), "{err}");
        assert!(check_base(dir.path(), "no-such-bookmark").is_err());
        assert!(check_base(dir.path(), "main").is_ok());
    }

    #[test]
    fn check_repo_rejects_a_plain_directory() {
        if !jj_available() {
            eprintln!("skipping: jj not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        assert!(check_repo(dir.path()).is_err());
        let repo = scratch_repo();
        assert!(check_repo(repo.path()).is_ok());
    }
}
