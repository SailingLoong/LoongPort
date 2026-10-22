//! Allocates detached worktrees; only this owner chooses a runner's working directory.
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub(super) struct Workspace {
    pub path: PathBuf,
    pub base_commit: String,
}
fn git(repository: &Path, args: &[&std::ffi::OsStr]) -> Result<String, String> {
    use std::{
        process::{Command, Stdio},
        time::Duration,
    };
    let mut command = Command::new("git");
    command
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-C",
        ])
        .arg(repository)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::process::isolate_child_process_group(&mut command);
    let child = command.spawn().map_err(|_| "agents.git_unavailable")?;
    let result = crate::process::wait_child_output(
        child,
        crate::process::CommandDeadline::from_timeout(Some(Duration::from_secs(30))),
    )?;
    if !result.status.success() || result.stdout.len() > 1024 * 1024 {
        return Err("agents.workspace_git_failed".into());
    }
    String::from_utf8(result.stdout).map_err(|_| "agents.workspace_invalid_output".into())
}
impl Workspace {
    pub(super) fn create(
        repository: &Path,
        base: &str,
        destination: &Path,
    ) -> Result<Self, String> {
        let repository = repository
            .canonicalize()
            .map_err(|_| "agents.repository_unavailable")?;
        if !destination.is_absolute()
            || destination.starts_with(&repository)
            || destination.exists()
            || base.is_empty()
            || base.len() > 1024
        {
            return Err("agents.workspace_conflict".into());
        }
        let revision = format!("{base}^{{commit}}");
        let base_commit = git(
            &repository,
            &[
                "rev-parse".as_ref(),
                "--verify".as_ref(),
                "--end-of-options".as_ref(),
                revision.as_ref(),
            ],
        )?
        .trim()
        .to_owned();
        if !matches!(base_commit.len(), 40 | 64)
            || !base_commit.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("agents.workspace_invalid_revision".into());
        }
        git(
            &repository,
            &[
                "worktree".as_ref(),
                "add".as_ref(),
                "--detach".as_ref(),
                "--".as_ref(),
                destination.as_os_str(),
                base_commit.as_ref(),
            ],
        )?;
        let path = destination
            .canonicalize()
            .map_err(|_| "agents.workspace_unavailable")?;
        Ok(Self { path, base_commit })
    }
    /// Preserve the checkout and commit references for the integration owner.
    /// No auto-commit, cleanup, or merge is performed here.
    pub(super) fn inspect(&self) -> Result<(String, String), String> {
        let head = git(&self.path, &["rev-parse".as_ref(), "HEAD".as_ref()])?
            .trim()
            .to_owned();
        let diff = git(
            &self.path,
            &[
                "diff".as_ref(),
                "--no-ext-diff".as_ref(),
                "--stat".as_ref(),
                self.base_commit.as_ref(),
                "--".as_ref(),
            ],
        )?;
        let status = git(
            &self.path,
            &[
                "status".as_ref(),
                "--porcelain=v1".as_ref(),
                "--untracked-files=all".as_ref(),
            ],
        )?;
        Ok((head, format!("{diff}{status}")))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn git(path: &Path, args: &[&str]) {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    #[test]
    fn independent_tasks_write_separate_worktrees_and_source_is_untouched() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("source.txt"), "base\n").unwrap();
        git(&repo, &["add", "source.txt"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-qm",
                "base",
            ],
        );
        let first = Workspace::create(&repo, "HEAD", &root.path().join("first")).unwrap();
        let second = Workspace::create(&repo, "HEAD", &root.path().join("second")).unwrap();
        std::fs::write(first.path.join("source.txt"), "first\n").unwrap();
        std::fs::write(second.path.join("source.txt"), "second\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.join("source.txt")).unwrap(),
            "base\n"
        );
        assert_ne!(first.path, second.path);
        assert_eq!(first.base_commit, second.base_commit);
        assert!(first.inspect().unwrap().1.contains("source.txt"));
        assert!(Workspace::create(&repo, "HEAD", &first.path).is_err());
    }
}
