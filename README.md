<div align="center">

# tern-azdo

**Azure DevOps pull requests, native in [Tern](https://stencil.so/tern).**

Every active PR in your organization, across all projects and repos, with diffs, comment threads and votes, one keystroke away from your shell.

[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![rust 2024](https://img.shields.io/badge/rust-2024_edition-orange.svg)](https://www.rust-lang.org)
[![tern plugin](https://img.shields.io/badge/tern-plugin-6f42c1.svg)](https://stencil.so/tern)
[![azure devops](https://img.shields.io/badge/azure_devops-pull_requests-0078d4.svg)](https://learn.microsoft.com/azure/devops/repos/git/pull-requests)

<img src="docs/list.png" alt="The pull request list: grouped by project and repo, with vote counts, your vote, authors, ages and branches" width="900">

</div>

## Why

Reviewing in Azure DevOps usually means a browser tab per pull request. tern-azdo puts the whole organization in one list and keeps you in the terminal from "what needs my review" to "approved". It's one Rust binary with no runtime, and it draws real Tern surfaces (tabs, tables, syntax-highlighted diffs) instead of text art. It's the Tern counterpart to the [AzDO Pull Requests (Multi-Project)](https://github.com/zachristmas/vscode-pull-request-azdo) VS Code extension.

## Features

**One list for the organization.** Tabs for PRs you review, PRs you created, and everything active, grouped by project and repo. Each row shows drafts, vote counts, your own vote, the author, the age and the branches. Press `f` and type to filter by title, author, repo, branch or PR id.

**A page per pull request.** A status card says where the PR stands: who approved, who is waiting or rejected, required reviewers still to vote, merge conflicts, and how many comment threads are open. The description renders as Markdown, and reviewers and linked work items sit beside it.

<p align="center"><img src="docs/overview.png" alt="A pull request's overview: the status card, the rendered description, and the reviewers and work items beside it" width="900"></p>

**The whole conversation, in order.** Under the description, the activity timeline lists everything newest first: votes, pushes, reviewers being added, and every comment thread. Each thread is its own card with the file, its line range, the code it is about and its status (Active, Resolved, Won't fix). Every comment in it shows in full, with the author's name and initials, when they wrote it, whether it was edited, and its Markdown (lists, bold, code). `n` jumps from thread to thread, and `a` replies to the one you are on.

<p align="center"><img src="docs/activity.png" alt="The activity timeline: a comment thread on tax.ts as a card with the code it is about and three comments, each with its author and time" width="900"></p>

**Diffs you can read.** The changed files sit beside a syntax-highlighted diff with real line numbers, paged from the keyboard. Each file's comment threads appear under its diff, and the file list marks which files have comments. `n` jumps to the next comment, moving across files and scrolling the diff to its line. `x` switches between hunks and the whole file.

<p align="center"><img src="docs/diff.png" alt="The Files tab: changed files on the left, the diff on the right, and the comment thread for the selected line underneath" width="900"></p>

**Review without leaving.** Vote (approve, approve with suggestions, wait for author, reject), start a comment thread, or reply to one. Open the PR in the browser, or copy its link or source branch.

**Nothing to store.** Sign-in goes through the Azure CLI's existing `az login`. A personal access token in the OS credential store works too.

## Try it

You need [Tern](https://stencil.so/tern) and a stable Rust toolchain (2024 edition, Rust 1.85 or newer).

```sh
git clone https://github.com/zachristmas/tern-azdo
cd tern-azdo
cargo install --path .
tern plugin link plugin
tern-azdo --demo        # a made-up organization, no account needed
```

For your own organization, sign in with the [Azure CLI](https://learn.microsoft.com/cli/azure/install-azure-cli) (`az login`), then open the Tern command palette:

| Command | What it does |
|---|---|
| Azure DevOps: Pull Requests | Opens the list in a new tab |
| Azure DevOps: Pull Requests Beside Current Pane | Opens it in a split to the right |

## Organization and sign-in

tern-azdo finds the organization from, in order: the `AZDO_ORG` environment variable, the saved config, then the `origin` remote of the directory it starts in. With none of those, it asks on first run. `tern-azdo org Contoso` (or any `dev.azure.com` or repo URL) saves it to `%APPDATA%\tern-azdo\config.json` on Windows and `~/.config/tern-azdo/config.json` elsewhere. The file holds no secrets.

Tokens come from `az account get-access-token`, so a working `az login` is all it needs. To use a personal access token instead (scopes: Code, Read & Write; Work Items, Read), run `tern-azdo pat` and paste it. It goes into the OS credential store (Windows Credential Manager, macOS Keychain, Secret Service on Linux), and `tern-azdo pat --forget` removes it. `AZDO_PAT` also works, for scripts.

## Command line

```text
tern-azdo                         the interactive view
tern-azdo --demo                  the interactive view on made-up data
tern-azdo list [review|mine|all]  active pull requests as a table
tern-azdo org [ORG]               show or set the organization
tern-azdo pat [--forget]          save (from stdin) or remove a personal access token
```

Outside Tern, `tern-azdo list` prints a plain-text table, so it works in scripts and other terminals.

## Keys

<table>
<tr><th>List</th><th>Pull request</th></tr>
<tr valign="top"><td>

| Key | Action |
|---|---|
| `↑` `↓`, `j` `k`, `PgUp` `PgDn` | Move |
| `gg` / `G` | First / last |
| `Ctrl+D` `Ctrl+U`, `d` `u` | Half a screen down / up |
| `Enter`, double-click | Open |
| `1` `2` `3`, `Tab` | Reviewing / mine / all |
| `f`, `Ctrl+F` | Filter (`Esc` clears) |
| `o` | Open in the browser |
| `y` / `b` | Copy link / branch |
| `r` | Refresh |
| `q`, `Ctrl+Q` | Quit |

</td><td>

| Key | Action |
|---|---|
| `Tab`, `1` / `2` | Overview / Files |
| `↑` `↓`, `j` `k` | Pick a file (Overview: scroll) |
| `gg` / `G` | Top / bottom of the overview or diff |
| `Ctrl+D` `Ctrl+U`, `d` `u` | Half a screen down / up |
| `Ctrl+F` `Ctrl+B`, `Space`, `PgDn` `PgUp` | A screen down / up |
| `n` / `p` | Next / previous comment |
| `x` | Whole file or hunks |
| `v` then `1` `2` `3` `4` `0` | Approve, with suggestions, wait, reject, reset |
| `c` / `a` | New comment / reply (`Enter` sends) |
| `Esc` | Leave the thread you are on, then back to the list |

</td></tr>
</table>

## Platform support

| OS | Status |
|---|---|
| Windows | Built and run against a live organization |
| macOS, Linux | Should build (CI workflow included); not tested by hand |

## Roadmap

- [ ] Check out the PR branch in a local clone
- [ ] Comment on a diff line
- [ ] Policy and build status
- [ ] Complete (merge) and abandon
- [ ] Commits and iteration-to-iteration diffs

## Related

- [tern-mssql](https://github.com/zachristmas/tern-mssql): a SQL Server client for Tern, built the same way.
- [AzDO Pull Requests (Multi-Project)](https://github.com/zachristmas/vscode-pull-request-azdo): the VS Code extension this mirrors.

## License

[MIT](LICENSE)
