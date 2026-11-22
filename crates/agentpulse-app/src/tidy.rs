//! Manual clean-up beyond processes: disk caches, stale git worktrees, and this
//! Mac's swap, simulators and virtual machines. Every function here blocks; the
//! window runs them on GPUI's background executor, never on the UI or
//! collector thread.
//!
//! The rules every action here keeps:
//! - Nothing runs until the user clicks Check, and Check changes nothing.
//! - Nothing changes without the confirmed second click.
//! - git never gets `--force`, the main worktree is never removed, and no
//!   branch is deleted.
//! - A worktree is removed only when no live session is in its repo, it holds
//!   no other worktree, `git status --porcelain` and `git rev-list HEAD --not
//!   --remotes` are both empty, and its ignored files are all build output;
//!   all checked again just before the removal.
//! - VMs and emulators are reported, never stopped: ending one can corrupt its disk.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use agentpulse_process::Pressure;

/// An unmerged worktree whose last commit is older than this is stale.
const STALE_DAYS: i64 = 14;

/// A tool's path. A GUI app doesn't get the shell's PATH, so these folders are
/// searched in order.
pub fn tool(name: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    [PathBuf::from("/opt/homebrew/bin"), "/usr/local/bin".into()]
        .into_iter()
        .chain(
            home.iter()
                .flat_map(|h| [h.join(".local/bin"), h.join(".cargo/bin")]),
        )
        .chain(["/usr/bin".into()])
        .map(|d| d.join(name))
        .find(|p| p.is_file())
        .filter(|p| !SHIMS.iter().any(|s| p == Path::new(s)) || command_line_tools())
}

/// Stubs that open the Command Line Tools installer when those aren't installed.
const SHIMS: [&str; 2] = ["/usr/bin/git", "/usr/bin/xcrun"];

fn command_line_tools() -> bool {
    Command::new("/usr/bin/xcode-select")
        .arg("-p")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `bin` with PATH set to its own folder and then the system's, so `npm` finds `node`.
fn command(bin: &Path) -> Command {
    let dir = bin.parent().unwrap_or(Path::new("/"));
    let mut c = Command::new(bin);
    c.env(
        "PATH",
        format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", dir.display()),
    )
    .stdin(Stdio::null());
    c
}

/// Stdout on success, else the exit status and the last line of stderr.
fn output(c: &mut Command) -> Result<String, String> {
    let out = c.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(failure(
            out.status.code(),
            &String::from_utf8_lossy(&out.stderr),
        ))
    }
}

fn run(bin: &Path, args: &[&str]) -> Result<String, String> {
    output(command(bin).args(args))
}

/// `git -C <dir> <args>`.
fn git(bin: &Path, dir: &Path, args: &[&str]) -> Result<String, String> {
    output(command(bin).arg("-C").arg(dir).args(args))
}

/// "Exit 1 · <last line of stderr>".
fn failure(code: Option<i32>, stderr: &str) -> String {
    let status = code.map_or_else(|| "Stopped by a signal".into(), |c| format!("Exit {c}"));
    match stderr.lines().map(str::trim).rfind(|l| !l.is_empty()) {
        Some(line) => format!("{status} · {line}"),
        None => status,
    }
}

/// Bytes allocated under `dir`, from `du -sk`; 0 when it's missing.
pub fn size(dir: &Path) -> u64 {
    if !dir.exists() {
        return 0;
    }
    // du exits non-zero on an unreadable entry but still prints the total.
    Command::new("/usr/bin/du")
        .arg("-sk")
        .arg(dir)
        .stdin(Stdio::null())
        .output()
        .ok()
        .and_then(|o| parse_du(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(0)
}

/// `du -sk` prints kilobytes, then a tab and the path.
pub fn parse_du(out: &str) -> Option<u64> {
    let kb: u64 = out.split_whitespace().next()?.parse().ok()?;
    Some(kb * 1024)
}

pub fn plural(n: usize, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
    }
}

// Disk caches.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cache {
    Brew,
    /// `brew autoremove`: formulae installed only as dependencies of removed ones.
    BrewFormulae,
    Uv,
    Npm,
}

impl Cache {
    const ALL: [Cache; 4] = [Cache::Brew, Cache::BrewFormulae, Cache::Uv, Cache::Npm];

    pub fn name(self) -> &'static str {
        match self {
            Cache::Brew => "Homebrew",
            Cache::BrewFormulae => "Homebrew unused formulae",
            Cache::Uv => "uv",
            Cache::Npm => "npm",
        }
    }

    fn tool(self) -> &'static str {
        match self {
            Cache::Brew | Cache::BrewFormulae => "brew",
            Cache::Uv => "uv",
            Cache::Npm => "npm",
        }
    }

    fn clean_args(self) -> &'static [&'static str] {
        match self {
            Cache::Brew => &["cleanup"],
            Cache::BrewFormulae => &["autoremove"],
            Cache::Uv => &["cache", "prune"],
            // npm refuses to clean its cache without its own --force; this is not git's.
            Cache::Npm => &["cache", "clean", "--force"],
        }
    }

    /// The folder to size; None for the formulae, which have no size.
    fn dir(self, bin: &Path) -> Result<Option<PathBuf>, String> {
        let line = |args: &[&str]| run(bin, args).map(|o| PathBuf::from(o.trim()));
        Ok(match self {
            Cache::Brew => Some(line(&["--cache"])?),
            Cache::BrewFormulae => None,
            Cache::Uv => Some(line(&["cache", "dir"])?),
            Cache::Npm => Some(line(&["config", "get", "cache"])?.join("_cacache")),
        })
    }
}

#[derive(Debug)]
pub struct CacheRow {
    pub cache: Cache,
    pub dir: Option<PathBuf>,
    pub bytes: u64,
    /// What `brew autoremove` would remove.
    pub formulae: Vec<String>,
    /// The failed command's status and stderr.
    pub error: Option<String>,
}

/// Each cache whose tool is installed, with its size. Changes nothing.
pub fn check_caches() -> Vec<CacheRow> {
    Cache::ALL
        .into_iter()
        .filter_map(|c| Some(check_cache(c, &tool(c.tool())?)))
        .filter(|r| r.cache != Cache::BrewFormulae || !r.formulae.is_empty() || r.error.is_some())
        .collect()
}

fn check_cache(cache: Cache, bin: &Path) -> CacheRow {
    let mut row = CacheRow {
        cache,
        dir: None,
        bytes: 0,
        formulae: Vec::new(),
        error: None,
    };
    let found = match cache {
        Cache::BrewFormulae => {
            run(bin, &["autoremove", "-n"]).map(|o| row.formulae = parse_autoremove(&o))
        }
        _ => cache.dir(bin).map(|d| {
            row.bytes = d.as_deref().map_or(0, size);
            row.dir = d;
        }),
    };
    row.error = found.err();
    row
}

/// `brew autoremove -n`: a "==> Would autoremove …" line, then one formula per line.
pub fn parse_autoremove(out: &str) -> Vec<String> {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("==>"))
        .map(String::from)
        .collect()
}

/// Bytes freed: the folder's size before minus after.
fn clean_cache(cache: Cache) -> Result<u64, String> {
    let bin = tool(cache.tool()).ok_or_else(|| format!("{} not found", cache.tool()))?;
    let dir = cache.dir(&bin)?;
    let before = dir.as_deref().map_or(0, size);
    // A running `uvx` tool holds uv's cache lock, and uv waits 300 s for it by default.
    output(
        command(&bin)
            .args(cache.clean_args())
            .env("UV_LOCK_TIMEOUT", "10"),
    )?;
    Ok(before.saturating_sub(dir.as_deref().map_or(0, size)))
}

// Worktrees.

/// Ignored files under these names are build output that can be made again, so
/// `git worktree remove` (which deletes ignored files without `--force`) may take them.
const REBUILDABLE: [&str; 12] = [
    "target",
    "node_modules",
    "build",
    "dist",
    ".gradle",
    ".next",
    ".turbo",
    "__pycache__",
    ".venv",
    "venv",
    "DerivedData",
    ".build",
];

/// The real path, so symlinks and `/tmp` vs `/private/tmp` compare equal; the
/// path itself when it can't be resolved.
fn real(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// One entry of `git worktree list --porcelain -z`.
#[derive(Debug, Default, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: String,
    /// Without `refs/heads/`.
    pub branch: Option<String>,
    pub detached: bool,
    pub locked: bool,
    /// Its folder is gone.
    pub prunable: bool,
}

impl Worktree {
    fn label(&self) -> String {
        match &self.branch {
            Some(b) if !self.detached => b.clone(),
            _ => format!("Detached at {}", &self.head[..self.head.len().min(7)]),
        }
    }
}

/// Fields are NUL-separated and an empty field ends an entry; the first entry
/// is the main worktree.
pub fn parse_worktrees(porcelain_z: &str) -> Vec<Worktree> {
    let mut list: Vec<Worktree> = Vec::new();
    for field in porcelain_z.split('\0').filter(|f| !f.is_empty()) {
        let (key, value) = field.split_once(' ').unwrap_or((field, ""));
        if key == "worktree" {
            list.push(Worktree {
                path: value.into(),
                ..Worktree::default()
            });
            continue;
        }
        let Some(w) = list.last_mut() else { continue };
        match key {
            "HEAD" => w.head = value.into(),
            "branch" => w.branch = Some(value.strip_prefix("refs/heads/").unwrap_or(value).into()),
            "detached" => w.detached = true,
            "locked" => w.locked = true,
            "prunable" => w.prunable = true,
            _ => {}
        }
    }
    list
}

fn list_worktrees(bin: &Path, repo: &Path) -> Option<Vec<Worktree>> {
    let out = git(bin, repo, &["worktree", "list", "--porcelain", "-z"]).ok()?;
    Some(parse_worktrees(&out))
}

/// The paths of `git status --porcelain -z --ignored` entries (`!! path`) that
/// aren't under a [`REBUILDABLE`] folder.
pub fn unrebuildable(status_z: &str) -> Vec<String> {
    status_z
        .split('\0')
        .filter_map(|e| e.strip_prefix("!! "))
        .filter(|p| {
            !Path::new(p).components().any(|c| {
                c.as_os_str()
                    .to_str()
                    .is_some_and(|c| REBUILDABLE.contains(&c))
            })
        })
        .map(String::from)
        .collect()
}

/// What decides whether Remove is offered.
#[derive(Debug, Default)]
pub struct Safety {
    /// Another worktree's folder is inside this one.
    pub nested: bool,
    /// `git status --porcelain` printed something (untracked files included), or failed.
    pub dirty: bool,
    /// `git rev-list HEAD --not --remotes`; None when that failed.
    pub unpushed: Option<usize>,
    /// Ignored files that aren't build output; None when that check failed.
    pub ignored: Option<Vec<String>>,
}

impl Safety {
    /// Why the worktree can't be removed safely, if it can't.
    fn blocker(&self) -> Option<String> {
        if self.nested {
            return Some("Holds another worktree".into());
        }
        if self.dirty {
            return Some("Uncommitted changes".into());
        }
        match self.unpushed {
            Some(0) => {}
            Some(n) => return Some(format!("{} not pushed", plural(n, "commit"))),
            None => return Some("Couldn't check what's pushed".into()),
        }
        match self.ignored.as_deref() {
            Some([]) => None,
            Some(files) => {
                let more = if files.len() > 3 { ", …" } else { "" };
                Some(format!(
                    "Ignored files: {}{more}",
                    files[..files.len().min(3)].join(", ")
                ))
            }
            None => Some("Couldn't check ignored files".into()),
        }
    }
}

/// Reads [`Safety`] for the worktree at `path`; `others` are the repo's other worktrees.
fn safety(bin: &Path, path: &Path, others: &[&Path]) -> Safety {
    let at = real(path);
    let status = |args: &[&str]| {
        git(
            bin,
            path,
            &[&["status", "--porcelain", "-z"][..], args].concat(),
        )
    };
    Safety {
        nested: others
            .iter()
            .map(|o| real(o))
            .any(|o| o != at && o.starts_with(&at)),
        // Explicit flags: repo config such as status.showUntrackedFiles=no would hide files.
        dirty: status(&["--untracked-files=all", "--ignore-submodules=none"])
            .map_or(true, |o| !o.is_empty()),
        unpushed: git(
            bin,
            path,
            &["rev-list", "--count", "HEAD", "--not", "--remotes"],
        )
        .ok()
        .and_then(|o| o.trim().parse().ok()),
        // Traditional mode with normal untracked files: a wholly ignored folder is one `!! dir/`.
        ignored: status(&["--ignored=traditional", "--untracked-files=normal"])
            .ok()
            .map(|o| unrebuildable(&o)),
    }
}

/// What decides whether a worktree is listed as stale.
#[derive(Debug, Default)]
pub struct Facts {
    pub locked: bool,
    /// Its branch, without `refs/heads/`.
    pub branch: Option<String>,
    /// `origin/HEAD`'s branch, else `main`, else `master`.
    pub default: Option<String>,
    /// HEAD is an ancestor of the default branch.
    pub ancestor: bool,
    /// HEAD is the default branch's tip.
    pub at_default_tip: bool,
    /// Since its last commit.
    pub age_days: i64,
    pub safety: Safety,
}

impl Facts {
    /// The default branch this worktree's work was merged into. Not a checkout
    /// of the default branch itself, nor one sitting at its tip: those hold no
    /// finished work, only the age rule applies to them.
    fn merged_into(&self) -> Option<&str> {
        let d = self.default.as_deref()?;
        let on_default = self.branch.as_deref() == Some(d.strip_prefix("origin/").unwrap_or(d));
        (self.ancestor && !on_default && !self.at_default_tip).then_some(d)
    }
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    /// In use or recent: not listed.
    Keep,
    /// Listed with why it's stale; `blocker` says why Remove isn't offered.
    Stale {
        why: String,
        blocker: Option<String>,
    },
}

pub fn verdict(f: &Facts) -> Verdict {
    if f.locked {
        return Verdict::Keep;
    }
    let why = match f.merged_into() {
        Some(b) => format!("Merged into {b}"),
        None if f.age_days > STALE_DAYS => format!("Last commit {} days ago", f.age_days),
        None => return Verdict::Keep,
    };
    Verdict::Stale {
        why,
        blocker: f.safety.blocker(),
    }
}

/// `origin/HEAD`'s branch, else `main`, else `master`.
fn default_branch(bin: &Path, repo: &Path) -> Option<String> {
    if let Ok(o) = git(bin, repo, &["rev-parse", "--abbrev-ref", "origin/HEAD"]) {
        return Some(o.trim().into());
    }
    ["main", "master"]
        .into_iter()
        .find(|b| {
            git(
                bin,
                repo,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{b}"),
                ],
            )
            .is_ok()
        })
        .map(String::from)
}

fn facts(bin: &Path, w: &Worktree, default: Option<&str>, others: &[&Path], now_s: i64) -> Facts {
    let at = &w.path;
    let rev = |r: &str| {
        git(bin, at, &["rev-parse", r])
            .ok()
            .map(|o| o.trim().to_owned())
    };
    Facts {
        locked: w.locked,
        branch: w.branch.clone().filter(|_| !w.detached),
        default: default.map(String::from),
        ancestor: default
            .is_some_and(|d| git(bin, at, &["merge-base", "--is-ancestor", "HEAD", d]).is_ok()),
        at_default_tip: default.is_some_and(|d| rev("HEAD").is_some_and(|h| rev(d) == Some(h))),
        age_days: git(bin, at, &["log", "-1", "--format=%ct", "HEAD"])
            .ok()
            .and_then(|o| o.trim().parse::<i64>().ok())
            .map_or(0, |ct| (now_s - ct) / 86_400),
        safety: safety(bin, at, others),
    }
}

/// The repo's shared git folder, resolved, for telling which repo a path is in.
fn common_dir(bin: &Path, path: &Path) -> Option<PathBuf> {
    let out = git(
        bin,
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()?;
    Some(real(Path::new(out.trim())))
}

/// The repos live sessions are in. A session's project is its first folder, so
/// an agent that moved into a worktree still shows the repo's root: the whole
/// repo counts as in use.
fn live_repos(bin: &Path, live: &[String]) -> HashSet<PathBuf> {
    live.iter()
        .filter_map(|p| common_dir(bin, Path::new(p)))
        .collect()
}

#[derive(Debug)]
pub struct Repo {
    /// The main worktree, which is never offered for removal.
    pub main: PathBuf,
    /// Worktrees whose folder is gone.
    pub prunable: usize,
    pub stale: Vec<Stale>,
}

#[derive(Debug)]
pub struct Stale {
    pub path: PathBuf,
    /// The branch, or where it's detached.
    pub name: String,
    pub why: String,
    pub bytes: u64,
    pub blocker: Option<String>,
}

/// Repos of `projects` (every session's project in the snapshot) with a stale
/// or prunable worktree. In a repo with a live session (`live`), no worktree
/// is stale. Changes nothing.
pub fn check_worktrees(projects: &[String], live: &[String]) -> Vec<Repo> {
    let Some(bin) = tool("git") else {
        return Vec::new();
    };
    let live = live_repos(&bin, live);
    let now_s = chrono::Utc::now().timestamp();
    let mut seen = HashSet::new();
    projects
        .iter()
        .map(Path::new)
        .filter_map(|p| {
            let common = common_dir(&bin, p)?;
            seen.insert(common.clone())
                .then(|| repo(&bin, p, live.contains(&common), now_s))
                .flatten()
        })
        .filter(|r| r.prunable > 0 || !r.stale.is_empty())
        .collect()
}

fn repo(bin: &Path, any: &Path, live: bool, now_s: i64) -> Option<Repo> {
    let list = list_worktrees(bin, any)?;
    let (main, rest) = list.split_first()?;
    let default = default_branch(bin, &main.path);
    let paths: Vec<&Path> = list.iter().map(|w| w.path.as_path()).collect();
    let stale = rest
        .iter()
        .filter(|w| !live && !w.prunable)
        .filter_map(
            |w| match verdict(&facts(bin, w, default.as_deref(), &paths, now_s)) {
                Verdict::Keep => None,
                Verdict::Stale { why, blocker } => Some(Stale {
                    path: w.path.clone(),
                    name: w.label(),
                    why,
                    bytes: size(&w.path),
                    blocker,
                }),
            },
        )
        .collect();
    Some(Repo {
        main: main.path.clone(),
        prunable: rest.iter().filter(|w| w.prunable).count(),
        stale,
    })
}

/// Removes a linked worktree after checking again everything Check checked:
/// a commit, a file or an agent may have appeared since. Ok is the bytes freed.
fn remove(bin: &Path, main: &Path, path: &Path, live: &[String]) -> Result<u64, String> {
    let list = list_worktrees(bin, main).ok_or("Couldn't list the worktrees")?;
    let at = real(path);
    if list.first().is_none_or(|m| real(&m.path) == at) || real(main) == at {
        return Err("The main worktree stays".into());
    }
    let common = common_dir(bin, main).ok_or("Couldn't find the repo")?;
    if live_repos(bin, live).contains(&common) {
        return Err("An agent is working in this repo".into());
    }
    let paths: Vec<&Path> = list.iter().map(|w| w.path.as_path()).collect();
    if let Some(why) = safety(bin, path, &paths).blocker() {
        return Err(why);
    }
    let before = size(path);
    // No --force, so git itself refuses a tree that changed. The branch stays.
    output(
        command(bin)
            .arg("-C")
            .arg(main)
            .args(["worktree", "remove"])
            .arg(path),
    )?;
    Ok(before.saturating_sub(size(path)))
}

// This Mac.

#[derive(Debug, Default, PartialEq)]
pub struct Simulators {
    pub booted: usize,
    pub unavailable: usize,
    /// The unavailable ones' data folders.
    pub unavailable_bytes: u64,
}

/// Counts from `xcrun simctl list devices -j`.
pub fn parse_simctl(json: &str) -> Option<Simulators> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let mut s = Simulators::default();
    for d in v
        .get("devices")?
        .as_object()?
        .values()
        .filter_map(|r| r.as_array())
        .flatten()
    {
        if d["state"] == "Booted" {
            s.booted += 1;
        }
        if d["isAvailable"] == false {
            s.unavailable += 1;
            s.unavailable_bytes += d["dataPathSize"].as_u64().unwrap_or(0);
        }
    }
    Some(s)
}

/// None without `xcrun`, or when simctl fails.
fn simulators() -> Option<Simulators> {
    parse_simctl(&run(&tool("xcrun")?, &["simctl", "list", "devices", "-j"]).ok()?)
}

/// The product a VM or emulator process belongs to, from `ps`'s executable path.
pub fn vm_product(comm: &str) -> Option<&'static str> {
    let name = comm.rsplit('/').next().unwrap_or(comm);
    Some(match name {
        // The Android SDK's emulator runs its own qemu-system-* from emulator/qemu/.
        _ if name.starts_with("qemu-system-") && comm.contains("/emulator/") => "Android emulator",
        _ if name.starts_with("qemu-system-") => "QEMU",
        "emulator" => "Android emulator",
        _ if name.starts_with("com.docker.") => "Docker",
        _ if comm.contains("/OrbStack.app/") || name == "OrbStack" => "OrbStack",
        "colima" => "Colima",
        "limactl" => "Lima",
        "QEMULauncher" => "UTM",
        "prl_vm_app" => "Parallels",
        "com.apple.Virtualization.VirtualMachine" => "Apple Virtualization",
        _ => return None,
    })
}

/// `ps -axo rss=,comm=` lines grouped by product, resident bytes summed, largest first.
pub fn parse_vms(ps: &str) -> Vec<(&'static str, u64)> {
    let mut groups: Vec<(&'static str, u64)> = Vec::new();
    for line in ps.lines() {
        let Some((rss, comm)) = line.trim_start().split_once(' ') else {
            continue;
        };
        let (Ok(kb), Some(product)) = (rss.parse::<u64>(), vm_product(comm.trim())) else {
            continue;
        };
        match groups.iter_mut().find(|g| g.0 == product) {
            Some(g) => g.1 += kb * 1024,
            None => groups.push((product, kb * 1024)),
        }
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.1));
    groups
}

#[derive(Debug)]
pub struct Mac {
    pub swap_used: Option<u64>,
    pub swap_total: Option<u64>,
    pub pressure: Option<Pressure>,
    /// None without `xcrun`.
    pub simulators: Option<Simulators>,
    /// Report only.
    pub vms: Vec<(&'static str, u64)>,
}

/// Swap, memory pressure, simulators and running VMs. Changes nothing.
pub fn check_mac() -> Mac {
    let ps = output(
        Command::new("/bin/ps")
            .args(["-axo", "rss=,comm="])
            .stdin(Stdio::null()),
    );
    Mac {
        swap_used: agentpulse_process::swap_used_bytes(),
        swap_total: agentpulse_process::swap_total_bytes(),
        pressure: agentpulse_process::memory_pressure(),
        simulators: simulators(),
        vms: ps.map(|o| parse_vms(&o)).unwrap_or_default(),
    }
}

// Actions.

/// Something a confirmed second click changes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Clean(Cache),
    /// `git worktree prune` in the repo whose main worktree this is.
    Prune(PathBuf),
    Remove {
        main: PathBuf,
        path: PathBuf,
    },
    ShutDownSimulators,
    DeleteUnavailableSimulators,
}

/// Runs a confirmed action. Ok is the bytes it freed, size before minus after.
/// `live` are live sessions' projects as the window sees them now.
pub fn act(a: &Action, live: &[String]) -> Result<u64, String> {
    let need = |name: &str| tool(name).ok_or_else(|| format!("{name} not found"));
    match a {
        Action::Clean(c) => clean_cache(*c),
        Action::Prune(main) => git(&need("git")?, main, &["worktree", "prune"]).map(|_| 0),
        Action::Remove { main, path } => remove(&need("git")?, main, path, live),
        Action::ShutDownSimulators => {
            run(&need("xcrun")?, &["simctl", "shutdown", "all"]).map(|_| 0)
        }
        Action::DeleteUnavailableSimulators => {
            let before = simulators().map_or(0, |s| s.unavailable_bytes);
            run(&need("xcrun")?, &["simctl", "delete", "unavailable"])?;
            Ok(before.saturating_sub(simulators().map_or(0, |s| s.unavailable_bytes)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `git worktree list --porcelain -z`: NUL after each field, and one more after each entry.
    const PORCELAIN: &str = concat!(
        "worktree /r/main\0HEAD 1111111111111111111111111111111111111111\0branch refs/heads/main\0\0",
        "worktree /r/feature\0HEAD 2222222222222222222222222222222222222222\0branch refs/heads/feature/x\0\0",
        "worktree /r/detached\0HEAD 3333333333333333333333333333333333333333\0detached\0\0",
        "worktree /r/locked\0HEAD 4444444444444444444444444444444444444444\0branch refs/heads/locked\0",
        "locked busy\non a USB drive\0\0",
        "worktree /r/gone\0HEAD 5555555555555555555555555555555555555555\0branch refs/heads/gone\0",
        "prunable gitdir file points to non-existent location\0\0",
    );

    #[test]
    fn porcelain_parses_each_kind() {
        let w = parse_worktrees(PORCELAIN);
        assert_eq!(w.len(), 5);
        assert_eq!(w[0].path, Path::new("/r/main"));
        assert_eq!(w[0].branch.as_deref(), Some("main"));
        assert_eq!(w[1].branch.as_deref(), Some("feature/x"));
        assert_eq!(w[1].label(), "feature/x");
        assert!(!w[1].detached && !w[1].locked && !w[1].prunable);
        assert!(w[2].detached);
        assert_eq!(w[2].branch, None);
        assert_eq!(w[2].label(), "Detached at 3333333");
        assert!(w[3].locked && !w[3].prunable);
        assert!(w[4].prunable && !w[4].locked);
        assert_eq!(w[4].head, "5555555555555555555555555555555555555555");
    }

    fn safe() -> Safety {
        Safety {
            unpushed: Some(0),
            ignored: Some(Vec::new()),
            ..Safety::default()
        }
    }

    fn clean_merged() -> Facts {
        Facts {
            branch: Some("feature".into()),
            default: Some("origin/main".into()),
            ancestor: true,
            safety: safe(),
            ..Facts::default()
        }
    }

    fn stale(why: &str, blocker: Option<&str>) -> Verdict {
        Verdict::Stale {
            why: why.into(),
            blocker: blocker.map(String::from),
        }
    }

    #[test]
    fn verdict_lists_stale_and_offers_remove_only_when_safe() {
        const MERGED: &str = "Merged into origin/main";
        assert_eq!(verdict(&clean_merged()), stale(MERGED, None));
        let old = Facts {
            ancestor: false,
            age_days: 20,
            ..clean_merged()
        };
        assert_eq!(verdict(&old), stale("Last commit 20 days ago", None));

        // Not listed: locked, or unmerged and recent.
        let locked = Facts {
            locked: true,
            ..clean_merged()
        };
        assert_eq!(verdict(&locked), Verdict::Keep);
        let recent = Facts {
            age_days: 14,
            ..old
        };
        assert_eq!(verdict(&recent), Verdict::Keep);

        // A checkout of the default branch, or one at its tip, isn't merged work.
        let on_main = |age_days| Facts {
            branch: Some("main".into()),
            age_days,
            ..clean_merged()
        };
        assert_eq!(verdict(&on_main(3)), Verdict::Keep);
        assert_eq!(
            verdict(&on_main(30)),
            stale("Last commit 30 days ago", None)
        );
        let at_tip = Facts {
            at_default_tip: true,
            ..clean_merged()
        };
        assert_eq!(verdict(&at_tip), Verdict::Keep);
        let local_main = Facts {
            branch: Some("main".into()),
            default: Some("main".into()),
            ..clean_merged()
        };
        assert_eq!(verdict(&local_main), Verdict::Keep);

        // Listed without Remove, one reason each.
        let blocked = |safety| {
            verdict(&Facts {
                safety,
                ..clean_merged()
            })
        };
        let cases = [
            (
                Safety {
                    nested: true,
                    ..safe()
                },
                "Holds another worktree",
            ),
            (
                Safety {
                    dirty: true,
                    ..safe()
                },
                "Uncommitted changes",
            ),
            (
                Safety {
                    unpushed: Some(3),
                    ..safe()
                },
                "3 commits not pushed",
            ),
            (
                Safety {
                    unpushed: Some(1),
                    ..safe()
                },
                "1 commit not pushed",
            ),
            (
                Safety {
                    unpushed: None,
                    ..safe()
                },
                "Couldn't check what's pushed",
            ),
            (
                Safety {
                    ignored: Some(vec![".env".into(), ".omc/".into()]),
                    ..safe()
                },
                "Ignored files: .env, .omc/",
            ),
            (
                Safety {
                    ignored: Some(["a", "b", "c", "d"].map(String::from).into()),
                    ..safe()
                },
                "Ignored files: a, b, c, …",
            ),
            (
                Safety {
                    ignored: None,
                    ..safe()
                },
                "Couldn't check ignored files",
            ),
        ];
        for (safety, why) in cases {
            assert_eq!(blocked(safety), stale(MERGED, Some(why)));
        }
    }

    #[test]
    fn only_build_output_may_be_ignored() {
        let status = concat!(
            "!! target/\0!! app/node_modules/\0!! ios/build/\0!! web/.next/\0",
            "!! src/__pycache__/\0!! .venv/\0!! DerivedData/\0",
            "!! .env\0!! .omc/\0!! notes/targets.txt\0",
            "?? new.txt\0 M src/lib.rs\0",
        );
        assert_eq!(
            unrebuildable(status),
            [".env", ".omc/", "notes/targets.txt"]
        );
        assert!(unrebuildable("").is_empty());
    }

    #[test]
    fn simctl_counts() {
        let json = r#"{"devices": {
            "com.apple.CoreSimulator.SimRuntime.iOS-26-5": [
                {"state": "Booted", "isAvailable": true, "name": "iPhone 17e"},
                {"state": "Shutdown", "isAvailable": true, "name": "iPhone Air"}
            ],
            "com.apple.CoreSimulator.SimRuntime.iOS-17-0": [
                {"state": "Shutdown", "isAvailable": false, "dataPathSize": 1048576, "name": "Old"},
                {"state": "Booted", "isAvailable": false, "name": "Older"}
            ]
        }}"#;
        assert_eq!(
            parse_simctl(json),
            Some(Simulators {
                booted: 2,
                unavailable: 2,
                unavailable_bytes: 1 << 20,
            })
        );
        assert_eq!(parse_simctl("not json"), None);
    }

    #[test]
    fn du_and_autoremove_parse() {
        assert_eq!(
            parse_du("4823004\t/Users/me/.npm/_cacache\n"),
            Some(4823004 * 1024)
        );
        assert_eq!(parse_du(""), None);
        assert_eq!(
            parse_autoremove("==> Would autoremove 2 unneeded formulae:\nlibfoo\nlibbar\n"),
            ["libfoo", "libbar"]
        );
        assert!(parse_autoremove("").is_empty());
    }

    #[test]
    fn vms_group_by_product() {
        let ps = "\
3419536 /System/Library/Frameworks/Virtualization.framework/Versions/A/XPCServices/com.apple.Virtualization.VirtualMachine.xpc/Contents/MacOS/com.apple.Virtualization.VirtualMachine
  59088 /Applications/OrbStack.app/Contents/MacOS/OrbStack
   1000 /Applications/OrbStack.app/Contents/MacOS/OrbStack Helper
2000000 /Users/me/Library/Android/sdk/emulator/qemu/darwin-aarch64/qemu-system-aarch64
   4000 /Users/me/Library/Android/sdk/emulator/emulator
 500000 /Applications/Docker.app/Contents/MacOS/com.docker.backend
   3000 /opt/homebrew/bin/limactl
 800000 /opt/homebrew/bin/qemu-system-x86_64
 700000 /Applications/UTM.app/Contents/XPCServices/QEMUHelper.xpc/Contents/MacOS/QEMULauncher
 600000 /Applications/Parallels Desktop.app/Contents/MacOS/prl_vm_app
  12000 /opt/homebrew/bin/colima
  90000 /Applications/Safari.app/Contents/MacOS/Safari
 garbage
";
        assert_eq!(
            parse_vms(ps),
            [
                ("Apple Virtualization", 3419536 * 1024),
                ("Android emulator", 2004000 * 1024),
                ("QEMU", 800000 * 1024),
                ("UTM", 700000 * 1024),
                ("Parallels", 600000 * 1024),
                ("Docker", 500000 * 1024),
                ("OrbStack", 60088 * 1024),
                ("Colima", 12000 * 1024),
                ("Lima", 3000 * 1024),
            ]
        );
    }

    #[test]
    fn failure_shows_status_and_last_stderr_line() {
        assert_eq!(
            failure(Some(1), "warning\nError: locked\n\n"),
            "Exit 1 · Error: locked"
        );
        assert_eq!(failure(Some(2), ""), "Exit 2");
        assert_eq!(failure(None, ""), "Stopped by a signal");
    }

    /// Read-only: runs the three Checks on this Mac and prints what they found.
    /// `cargo test -p agentpulse-app tidy::tests::checks_on_this_mac -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn checks_on_this_mac() {
        println!("{:#?}", check_caches());
        let repo = env!("CARGO_MANIFEST_DIR").to_string();
        println!("{:#?}", check_worktrees(&[repo], &[]));
        println!("{:#?}", check_mac());
    }
}
