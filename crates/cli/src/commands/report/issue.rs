//! Filing a report: shown first, then, once the tester agrees, a pre-filled
//! GitHub issue page or `gh issue create`. Nothing leaves the machine before
//! that.

use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{bail, Context};

pub use mina_node_report::issue::{
    attach_note, check_repo, encode, issue_url, link, title_for_url, Link, MAX_URL_LEN,
};

use super::{
    crash::{now_rfc3339, private_dir, write_private},
    redact::Redactor,
};

/// Where `report-<time>.md` files go: `<work dir>/bug-reports`, readable by
/// this user only, as are the files.
pub fn save(dir: &Path, contents: &str) -> anyhow::Result<PathBuf> {
    let dir = dir.join("bug-reports");
    private_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let stamp: String = now_rfc3339()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let path = dir.join(format!("report-{stamp}.md"));
    write_private(&path, contents.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// The path `--output` wrote, as it is: in full, not under `~` (RC7-FIXES:
/// in Docker `~` is `/data`, which a reader on the host takes for their own
/// home).
fn output_shown(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_owned())
        .display()
        .to_string()
}

/// An executable called `name` in `PATH`.
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// How a report is filed, from the command's options.
pub struct Filing<'a> {
    pub repo: &'a str,
    pub gh: bool,
    pub yes: bool,
    pub no_browser: bool,
    pub output: Option<&'a Path>,
    /// Where reports are saved: the work directory, or the current one.
    pub save_dir: PathBuf,
    pub redactor: &'a Redactor,
}

pub fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

fn confirm(prompt: String) -> anyhow::Result<bool> {
    Ok(dialoguer::Confirm::new()
        .with_prompt(prompt)
        .default(false)
        .interact()?)
}

impl Filing<'_> {
    fn shown(&self, path: &Path) -> String {
        self.redactor.text(&path.display().to_string())
    }

    /// Shows the issue, then files it once the tester agrees. `summary`
    /// gives the note and summary for a link when `body` is too long for
    /// one.
    pub fn file(
        &self,
        title: &str,
        body: &str,
        summary: impl Fn(&str) -> (String, String),
    ) -> anyhow::Result<()> {
        let document = format!("# {title}\n\n{body}");
        if let Some(path) = self.output {
            write_private(path, document.as_bytes())
                .with_context(|| format!("writing {}", path.display()))?;
            eprintln!("Report saved to {}; it was not filed.", output_shown(path));
            return Ok(());
        }
        println!("{document}");
        let gh = if self.gh {
            let found = find_in_path("gh");
            if found.is_none() {
                eprintln!("The GitHub CLI (gh) is not installed: using a pre-filled issue page.");
            }
            found
        } else {
            None
        };
        if !self.yes {
            if !interactive() {
                let path = save(&self.save_dir, &document)?;
                eprintln!(
                    "Not filed: there is no terminal to confirm on. The report is saved at {}. \
                     Run again with --yes to file it, or attach the file to an issue at \
                     https://github.com/{}/issues/new",
                    self.shown(&path),
                    self.repo
                );
                return Ok(());
            }
            let prompt = match gh {
                Some(_) => format!("Create this issue on github.com/{} with gh now?", self.repo),
                None => format!(
                    "Open a pre-filled issue on github.com/{}? Nothing is posted until you \
                     submit it there.",
                    self.repo
                ),
            };
            if !confirm(prompt)? {
                let path = save(&self.save_dir, &document)?;
                eprintln!("Not filed. The report is saved at {}.", self.shown(&path));
                return Ok(());
            }
        }
        if let Some(gh) = gh {
            return self.gh_create(&gh, title, body);
        }
        let link = link(self.repo, title, body, || {
            let saved = save(&self.save_dir, &document);
            let place = match &saved {
                Ok(path) => format!("is saved at `{}`", self.shown(path)),
                Err(error) => format!("could not be saved ({error:#})"),
            };
            if let Ok(path) = &saved {
                eprintln!(
                    "The report is too long for a link: the full report is saved at {}; attach \
                     it to the issue.",
                    self.shown(path)
                );
            }
            summary(&place)
        });
        self.open(link.url());
        Ok(())
    }

    fn gh_create(&self, gh: &Path, title: &str, body: &str) -> anyhow::Result<()> {
        let path = save(&self.save_dir, body)?;
        let status = Command::new(gh)
            .args(["issue", "create", "--repo", self.repo, "--title"])
            .arg(title_for_url(title))
            .arg("--body-file")
            .arg(&path)
            .status()
            .context("running gh")?;
        if !status.success() {
            bail!(
                "gh issue create failed ({status}); the report is saved at {}",
                self.shown(&path)
            );
        }
        Ok(())
    }

    fn open(&self, url: &str) {
        let graphical = !cfg!(target_os = "linux")
            || std::env::var_os("DISPLAY").is_some()
            || std::env::var_os("WAYLAND_DISPLAY").is_some();
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let opened = !self.no_browser
            && graphical
            && Command::new(opener)
                .arg(url)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok();
        if opened {
            eprintln!("Opened the pre-filled issue in your browser. If it didn't open, use:");
        } else {
            eprintln!("Open this link to file the issue (review it, then submit):");
        }
        println!("{url}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_are_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = save(dir.path(), "# report").unwrap();
        assert!(path.starts_with(dir.path().join("bug-reports")));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# report");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(&dir.path().join("bug-reports")), 0o700);
        }
    }

    #[test]
    fn rc7_the_output_path_is_shown_in_full() {
        assert_eq!(
            output_shown(Path::new("/data/report.md")),
            "/data/report.md"
        );
        let relative = output_shown(Path::new("report.md"));
        assert!(Path::new(&relative).is_absolute(), "{relative}");
        assert!(relative.ends_with("/report.md"), "{relative}");
        assert!(!relative.starts_with('~'), "{relative}");
    }
}
