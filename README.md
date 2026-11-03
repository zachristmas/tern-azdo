# tern-azdo

[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![rust 2024](https://img.shields.io/badge/rust-2024_edition-orange.svg)](https://www.rust-lang.org)
[![tern plugin](https://img.shields.io/badge/tern-plugin-6f42c1.svg)](https://stencil.so/tern)

Azure DevOps pull requests inside [Tern](https://stencil.so/tern): every active PR in your organization, across all projects and repos, with each PR's overview, changed files, diffs and comment threads, drawn as native Tern surfaces. One Rust binary per OS with no runtime to install. It's the Tern counterpart to the [AzDO Pull Requests (Multi-Project)](https://github.com/zachristmas/vscode-pull-request-azdo) VS Code extension.

Built on [`tern-sdk`](https://crates.io/crates/tern-sdk).

## Features

- One list for the whole organization, grouped by project and repo, with tabs for PRs you review, PRs you created, and everything active. Each row shows drafts, vote counts, your own vote, the author, the age and the branches.
- Filter by title, author, repo, branch or PR id.
- A PR page with reviewers and their votes, linked work items, the description, and the general comment threads.
- Changed files with the diff of the selected file, paged from the keyboard, with that file's comment threads under it. `n` jumps the diff to the next comment. `x` switches between hunks and the whole file.
- Vote, comment, and reply without leaving the terminal.
- Open the PR in the browser, or copy its link or source branch.
- Sign-in through the Azure CLI, so nothing is stored. A personal access token in the OS credential store works too.

## Install

You need [Tern](https://stencil.so/tern), a stable Rust toolchain (2024 edition, Rust 1.85 or newer), and either the [Azure CLI](https://learn.microsoft.com/cli/azure/install-azure-cli) signed in with `az login` or a personal access token.

```sh
git clone https://github.com/zachristmas/tern-azdo
cd tern-azdo
cargo install --path .
tern plugin link plugin
```

Then open the Tern command palette:

| Command | What it does |
|---|---|
| Azure DevOps: Pull Requests | Opens the list in a new tab |
| Azure DevOps: Pull Requests Beside Current Pane | Opens it in a split to the right |

## Organization and sign-in

The organization comes from, in order: the `AZDO_ORG` environment variable, the saved config, then the `origin` remote of the directory tern-azdo starts in. On first run with none of those, it asks. `tern-azdo org Contoso` (or any `dev.azure.com` or repo URL) saves it to `%APPDATA%\tern-azdo\config.json` on Windows and `~/.config/tern-azdo/config.json` elsewhere. The file holds no secrets.

Sign-in uses `az account get-access-token`, so a working `az login` is all it needs. To use a personal access token instead (scope: Code, Read & Write; Work Items, Read), run `tern-azdo pat` and paste it. It goes into the OS credential store (Windows Credential Manager, macOS Keychain, Secret Service on Linux). `tern-azdo pat --forget` removes it. `AZDO_PAT` also works, for scripts.

## Command line

```text
tern-azdo                         the interactive view
tern-azdo list [review|mine|all]  active pull requests as a table
tern-azdo org [ORG]               show or set the organization
tern-azdo pat [--forget]          save (from stdin) or remove a personal access token
```

## Keys

### List

| Key | Action |
|---|---|
| Arrows, `j` / `k`, `PgUp` / `PgDn` | Move |
| `Enter` / double-click | Open the pull request |
| `1` `2` `3`, `Tab` | Reviewing / Created by me / All active |
| `f`, `Ctrl+F` or `/` | Filter (`Esc` clears) |
| `o` | Open in the browser |
| `y` / `b` | Copy the link / the source branch |
| `r` | Refresh |
| `q` / `Ctrl+Q` | Quit |

### Pull request

| Key | Action |
|---|---|
| `Tab`, `1` / `2` | Overview / Files |
| Arrows, `j` / `k` | Files: pick a file. Overview: scroll |
| `PgUp` / `PgDn`, `Space` | Scroll the diff |
| `n` / `p` | Next / previous comment thread (scrolls the diff to it) |
| `x` | Whole file or hunks |
| `v` | Vote: `1` approve, `2` approve with suggestions, `3` wait for author, `4` reject, `0` reset |
| `c` / `a` | New comment / reply to the selected thread (`Enter` sends, `Shift+Enter` new line) |
| `o`, `y`, `b` | Browser, copy link, copy branch |
| `r` | Refresh |
| `Esc` | Back to the list |

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

## License

[MIT](LICENSE)
