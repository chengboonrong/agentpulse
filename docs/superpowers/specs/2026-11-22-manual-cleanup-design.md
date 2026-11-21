# Manual clean-up: caches, worktrees, Mac checks

Date: 2026-11-22. Status: owner said "just dive in" (2026-11-22); every call below is the
builder's, not the owner's. Part 2 of issue #60. Part 1 (orphan and idle processes) merged as
PR #61; its memory-low banner and notification already cover "suggest a clean-up when swap or
memory pressure crosses a threshold".

## Intent

Free disk and memory that agent work leaves behind, beyond processes. On the owner's Mac today:
npm cache 4.7 GB (`~/.npm/_cacache`), uv cache 1.7 GB, Homebrew cache 499 MB, a 3.3 GB
Virtualization.framework VM, swap 4.9 of 6 GB used, worktrees spread over `~/ai-projects` and
`~/orca/workspaces`.

## Goals

- Three cards on the Clean-up screen, below the process groups: **Disk caches**, **Worktrees**,
  **This Mac**. Each starts with a **Check** button. Check lists what it found with sizes and
  changes nothing (the dry run).
- Each action uses part 1's two-click confirm (End → "Confirm end" for 5 s), with its own label.
  After an action the row shows what it freed (size before minus size after). The card shows
  the total freed this session.
- All commands run off the UI and collector threads (`cx.background_spawn`). A row shows
  "Checking…" or "Cleaning…" while its command runs. Results live in the window's memory and are
  not saved.

## Non-goals and anti-goals

- No automatic action of any kind in this part. Nothing runs without a click on Check, and
  nothing changes without a confirmed action.
- Never remove a worktree that has uncommitted changes, untracked files, or commits that are on
  no remote. Never remove the main worktree. Never delete a branch. Never pass `--force`.
- Never stop a VM or an emulator: ending its process can corrupt its disk. Report only.
- Nothing that needs admin rights. No caches beyond brew, uv and npm (`~/.npm/_npx` stays).
- No new crate dependencies.

## Approaches

1. **Window-side background tasks (chosen).** The view spawns the command on GPUI's background
   executor and stores the result. The collector is untouched, so its ~1% budget can't regress,
   and a 2-minute `brew cleanup` can't stall ticks.
2. Commands through the collector's `Command` channel, like End and Keep. Ruled out: the
   collector thread would block on slow commands, or would need its own worker pool.
3. A Terminal window that runs the commands. Ruled out: no before/after sizes, and no
   pre-checks for worktrees.

## Design

New module `crates/agentpulse-app/src/tidy.rs`: blocking functions that shell out with
`std::process::Command`, plus pure parsers and decisions that can be tested. The UI part goes in
`ui/cleanup.rs` (or a sibling `ui/tidy.rs` if that file gets too long).

**Finding tools.** A GUI app doesn't get the shell's PATH. Look for each binary in
`/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`, `~/.cargo/bin` and `/usr/bin`, in that
order. Run each child with PATH set to the binary's own directory followed by
`/usr/bin:/bin:/usr/sbin:/sbin`, so that `npm` can find `node`. A tool that isn't found is left
out of the card.

**Sizes.** Use `du -sk <dir>` (blocks actually allocated). It takes 0.3 s for the 4.7 GB npm
cache. A missing directory counts as 0.

### Disk caches

| Row | Directory sized | Action |
|---|---|---|
| Homebrew | `brew --cache` | `brew cleanup` |
| Homebrew unused formulae | — | `brew autoremove`. Check shows the formulae from `brew autoremove -n`; there is no size. Hidden when there are none. |
| uv | `uv cache dir` | `uv cache prune` |
| npm | `npm config get cache` + `/_cacache` | `npm cache clean --force` |

A failing command shows its exit status and the last line of its stderr on the row.

### Worktrees

- **Repos:** the `git rev-parse --path-format=absolute --git-common-dir` of every snapshot
  session's `project_path` (live and ended), de-duplicated. A repo no agent session touched
  isn't listed: this is an accepted gap.
- For each repo, parse `git worktree list --porcelain` (`worktree`, `HEAD`, `branch`,
  `detached`, `locked`, `prunable` lines). The first entry is the main worktree and is never
  listed for removal.
- **Prunable** (folder gone): one **Prune** action per repo, `git worktree prune`.
- **Stale:** not locked, no live session's `project_path` inside it, and either its HEAD is
  merged into the default branch (`origin/HEAD`, else `main`, else `master`:
  `git merge-base --is-ancestor HEAD <default>`) or its last commit is more than 14 days old.
  The row shows the branch, why it's stale, and `du -sk` of its folder (Rust `target/` is
  usually the bulk).
- **Remove is offered only when it's safe:** `git status --porcelain` is empty (this includes
  untracked files) and `git rev-list HEAD --not --remotes` is empty (every commit is on some
  remote). Otherwise the row shows why ("Uncommitted changes", "3 commits not pushed") and has
  no button. The action is `git -C <repo> worktree remove <path>`, without `--force`, so git
  itself refuses a tree that changed since Check. The branch stays.

### This Mac

- **Memory:** swap used of total (add `swap_total_bytes` next to `swap_used_bytes` in
  `agentpulse-process`, from the same `xsw_usage`) and memory pressure (existing).
- **iOS simulators** (only when `xcrun` exists): parse `xcrun simctl list devices -j`; show the
  booted count and the unavailable count. Actions: **Shut down all** (`xcrun simctl shutdown
  all`), shown when at least one is booted; **Delete unavailable** (`xcrun simctl delete
  unavailable`), shown when at least one is unavailable.
- **Emulators and VMs**, report only: one `ps -axo rss=,comm=` call, matched by executable path
  or name: Android `qemu-system-*` and `emulator`; Docker `com.docker.*`; OrbStack; Colima and
  Lima `limactl`; UTM `QEMULauncher`; Parallels `prl_vm_app`; any Apple
  `com.apple.Virtualization.VirtualMachine`. Group the rows by product, summing resident memory,
  with "Quit it from its own app to free this".

## Tests (one module, `tidy.rs`)

- The porcelain parser on a fixture with a main, a normal, a detached, a locked and a prunable
  worktree.
- The stale/safe decision as a pure function over facts (merged, last commit age, live session
  inside, dirty, unpushed count, locked), covering each "no" reason.
- The simctl JSON counts, `du -sk` parsing, and the VM matcher on sample `ps` lines.

## Acceptance

- Check changes nothing, and every action needs the two-click confirm.
- A worktree with changes, untracked files or unpushed commits shows no Remove.
- After an action the row and the card show what was freed.
- The `--bench` collector overhead is unchanged: the collector isn't touched.
- Manual check on the owner's Mac: Check all three cards, clean the uv cache, and see the size
  drop.
