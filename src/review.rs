//! "Review with AI": start a coding agent in a new Tern block, in a checkout of the PR's repo, with a prompt about the
//! PR or one of its comments.

use std::path::Path;

pub const DEFAULT_AGENT: &str = "claude";

/// What the agent is asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
	/// Review the whole PR.
	Review,
	/// Give the agent the PR as background and wait for questions.
	Context,
	/// Explain a review comment.
	Explain,
	/// Draft a reply to a review comment.
	Respond,
	/// Explain the code at a line of the diff.
	ExplainCode,
	/// Review the code at a line of the diff.
	ReviewCode,
	/// Review the files selected in the file tree.
	ReviewFiles,
	/// Give the agent the files selected in the file tree as background.
	ContextFiles,
}

impl Kind {
	pub const ALL: [Kind; 8] = [Kind::Review, Kind::Context, Kind::Explain, Kind::Respond, Kind::ExplainCode, Kind::ReviewCode, Kind::ReviewFiles, Kind::ContextFiles];

	pub fn id(self) -> &'static str {
		match self {
			Kind::Review => "review",
			Kind::Context => "context",
			Kind::Explain => "explain",
			Kind::Respond => "respond",
			Kind::ExplainCode => "explain_code",
			Kind::ReviewCode => "review_code",
			Kind::ReviewFiles => "review_files",
			Kind::ContextFiles => "context_files",
		}
	}

	/// Whether it is about the files selected in the file tree.
	pub fn is_selection(self) -> bool {
		matches!(self, Kind::ReviewFiles | Kind::ContextFiles)
	}

	pub fn label(self) -> &'static str {
		match self {
			Kind::Review => "review",
			Kind::Context => "context",
			Kind::Explain => "explain",
			Kind::Respond => "respond",
			Kind::ExplainCode => "explain code",
			Kind::ReviewCode => "review code",
			Kind::ReviewFiles => "review selected",
			Kind::ContextFiles => "context selected",
		}
	}

	/// What the confirmation modal calls it.
	pub fn title(self) -> &'static str {
		match self {
			Kind::Review => "Review with AI",
			Kind::Context => "Add as context",
			Kind::Explain => "Explain this comment",
			Kind::Respond => "Draft a reply",
			Kind::ExplainCode => "Explain this code",
			Kind::ReviewCode => "Review this code",
			Kind::ReviewFiles => "Review the selected files",
			Kind::ContextFiles => "Add the selected files as context",
		}
	}

	pub fn default_prompt(self) -> &'static str {
		match self {
			Kind::Review => REVIEW,
			Kind::Context => CONTEXT,
			Kind::Explain => EXPLAIN,
			Kind::Respond => RESPOND,
			Kind::ExplainCode => EXPLAIN_CODE,
			Kind::ReviewCode => REVIEW_CODE,
			Kind::ReviewFiles => REVIEW_FILES,
			Kind::ContextFiles => CONTEXT_FILES,
		}
	}
}

const REVIEW: &str = "Review this pull request: Azure DevOps pull request !{id}, \"{title}\", by {author} in {project}/{repo} ({source} into {target}).\n\
	Link: {url}\n\n\
	Description:\n{description}\n\n\
	Changed files:\n{files}\n\n\
	Comments so far:\n{comments}\n\n\
	Run `git fetch origin {source} {target}`, then read `git diff origin/{target}...origin/{source}` and the code around each change. \
	Look for bugs, security problems, missing tests, broken edge cases and needless complexity, and check whether the open comments were addressed. \
	List findings by file and line, most serious first, then give a one-line verdict. \
	Do not post anything to the pull request and do not change any files.";

const CONTEXT: &str = "Background for this session, no task yet: Azure DevOps pull request !{id}, \"{title}\", by {author} in {project}/{repo} ({source} into {target}).\n\
	Link: {url}\n\n\
	Description:\n{description}\n\n\
	Changed files:\n{files}\n\n\
	Comments so far:\n{comments}\n\n\
	Run `git fetch origin {source} {target}` and read `git diff origin/{target}...origin/{source}` so you know the change. \
	Then reply with a two-sentence summary and wait for my questions. Do not review or change anything until I ask.";

const EXPLAIN: &str = "Explain this review comment on Azure DevOps pull request !{id}, \"{title}\" ({project}/{repo}, {source} into {target}).\n\
	Link: {url}\n\n\
	File: {file}, line {line}\n\
	Thread:\n{thread}\n\n\
	Run `git fetch origin {source} {target}`, then read {file} around line {line} and `git diff origin/{target}...origin/{source} -- {file}`. \
	Explain what the commenter is asking for and why, whether they are right, and what changing the code would involve. \
	Do not post anything to the pull request and do not change any files.";

const RESPOND: &str = "Draft my reply to this review comment on Azure DevOps pull request !{id}, \"{title}\" ({project}/{repo}, {source} into {target}).\n\
	Link: {url}\n\n\
	File: {file}, line {line}\n\
	Thread:\n{thread}\n\n\
	Run `git fetch origin {source} {target}`, then read {file} around line {line} and the diff for it before answering. \
	Write the reply as I would post it: short, specific and polite. If the comment is right, say what will change (with the code); \
	if it is not, say why, pointing at the code. Give only the reply text. Do not post it and do not change any files.";

const EXPLAIN_CODE: &str = "Explain this code from Azure DevOps pull request !{id}, \"{title}\" ({project}/{repo}, {source} into {target}).\n\
	Link: {url}\n\n\
	File: {file}, around line {line}\n\
	{code}\n\n\
	Run `git fetch origin {source} {target}`, then read {file} and `git diff origin/{target}...origin/{source} -- {file}`. \
	Explain what this code does, what the change to it does and why it may have been made, and anything surprising or risky about it. \
	Do not post anything to the pull request and do not change any files.";

const REVIEW_CODE: &str = "Review this code from Azure DevOps pull request !{id}, \"{title}\" ({project}/{repo}, {source} into {target}).\n\
	Link: {url}\n\n\
	File: {file}, around line {line}\n\
	{code}\n\n\
	My note on it, if any: {comment}\n\n\
	Run `git fetch origin {source} {target}`, then read {file} and `git diff origin/{target}...origin/{source} -- {file}`. \
	Review the code at and around that line for bugs, edge cases, security problems and needless complexity, and say whether my note is right. \
	Give findings by line, most serious first. Do not post anything to the pull request and do not change any files.";

const REVIEW_FILES: &str = "Review part of this pull request: Azure DevOps pull request !{id}, \"{title}\", by {author} in {project}/{repo} ({source} into {target}).\n\
	Link: {url}\n\n\
	Description:\n{description}\n\n\
	Review only these {selected_count} files:\n{selected}\n\n\
	Comments so far:\n{comments}\n\n\
	Run `git fetch origin {source} {target}`, then read `git diff origin/{target}...origin/{source}` limited to those paths, and the code around each change. \
	Use the rest of the pull request only for context. \
	Look for bugs, security problems, missing tests, broken edge cases and needless complexity, and check whether the open comments about these files were addressed. \
	List findings by file and line, most serious first, then give a one-line verdict. \
	Do not post anything to the pull request and do not change any files.";

const CONTEXT_FILES: &str = "Background for this session, no task yet: Azure DevOps pull request !{id}, \"{title}\", by {author} in {project}/{repo} ({source} into {target}).\n\
	Link: {url}\n\n\
	Description:\n{description}\n\n\
	The files that matter ({selected_count}):\n{selected}\n\n\
	Comments so far:\n{comments}\n\n\
	Run `git fetch origin {source} {target}` and read `git diff origin/{target}...origin/{source}` limited to those paths, so you know that part of the change. \
	Then reply with a two-sentence summary and wait for my questions. Do not review or change anything until I ask.";

/// The values a prompt can use, as `{name}` (see [`NAMES`]).
#[derive(Clone, Debug, Default)]
pub struct Vars {
	pub id:          String,
	pub title:       String,
	pub url:         String,
	pub project:     String,
	pub repo:        String,
	pub source:      String,
	pub target:      String,
	pub author:      String,
	pub description: String,
	pub files:       String,
	pub comments:    String,
	pub thread:      String,
	pub file:        String,
	pub line:        String,
	pub comment:     String,
	pub code:        String,
	pub selected:    String,
	pub selected_count: String,
}

/// The variables, for the hint above the prompt editor.
pub const NAMES: &str = "{id} {title} {url} {project} {repo} {source} {target} {author} {description} {files} {comments} {thread} {file} {line} {comment} {code} {selected} {selected_count}";

/// The prompt with its variables filled in.
pub fn render(prompt: &str, v: &Vars) -> String {
	let pairs = [
		("{id}", &v.id),
		("{title}", &v.title),
		("{url}", &v.url),
		("{project}", &v.project),
		("{repo}", &v.repo),
		("{source}", &v.source),
		("{target}", &v.target),
		("{author}", &v.author),
		("{description}", &v.description),
		("{files}", &v.files),
		("{comments}", &v.comments),
		("{thread}", &v.thread),
		("{file}", &v.file),
		("{line}", &v.line),
		("{comment}", &v.comment),
		("{code}", &v.code),
		("{selected}", &v.selected),
		("{selected_count}", &v.selected_count),
	];
	// One pass, so a value that contains `{title}` stays as written.
	let mut out = String::with_capacity(prompt.len());
	let mut rest = prompt;
	'scan: while let Some(open) = rest.find('{') {
		out.push_str(&rest[..open]);
		rest = &rest[open..];
		for (name, value) in &pairs {
			if let Some(after) = rest.strip_prefix(name) {
				out.push_str(value);
				rest = after;
				continue 'scan;
			}
		}
		out.push('{');
		rest = &rest[1..];
	}
	out.push_str(rest);
	out
}

/// The program and arguments for an agent setting: a command line split on spaces, with `{prompt}` replaced by the
/// prompt (one argument) or the prompt appended. `claude` and `codex` take the prompt as their first argument.
pub fn argv(agent: &str, prompt: &str) -> Vec<String> {
	let mut parts: Vec<String> = agent.split_whitespace().map(str::to_owned).collect();
	if parts.is_empty() {
		parts.push(DEFAULT_AGENT.to_owned());
	}
	if parts.iter().any(|p| p.contains("{prompt}")) {
		parts.iter().map(|p| p.replace("{prompt}", prompt)).collect()
	} else {
		parts.push(prompt.to_owned());
		parts
	}
}

/// Open the agent in a block beside this one (a new tab when the plugin is not in a Tern pane). Returns what happened.
pub fn launch(dir: &str, argv: &[String]) -> anyhow::Result<String> {
	if !Path::new(dir).is_dir() {
		anyhow::bail!("{dir} is not a folder");
	}
	let mut cmd = std::process::Command::new("tern");
	match std::env::var("TERN_PANE") {
		Ok(pane) if !pane.is_empty() => cmd.args(["split", &pane, "right"]),
		_ => cmd.args(["new", "tab"]),
	};
	cmd.args(["--cwd", dir, "--keep-open", "--"]).args(argv);
	let out = cmd.output()?;
	if !out.status.success() {
		let err = String::from_utf8_lossy(&out.stderr);
		anyhow::bail!("tern: {}", err.lines().next().unwrap_or("could not open the block").trim());
	}
	Ok(format!("started {} in {dir}", argv.first().map_or("the agent", String::as_str)))
}

fn git(dir: Option<&Path>, args: &[&str]) -> Option<String> {
	let mut cmd = std::process::Command::new("git");
	if let Some(d) = dir {
		cmd.arg("-C").arg(d);
	}
	let out = cmd.args(args).output().ok().filter(|o| o.status.success())?;
	Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Whether `dir` is a git checkout whose `origin` is a repo called `repo`.
fn is_checkout_of(dir: &Path, repo: &str) -> bool {
	let Some(url) = git(Some(dir), &["remote", "get-url", "origin"]) else { return false };
	url.trim_end_matches('/').trim_end_matches(".git").rsplit(['/', ':']).next().is_some_and(|n| n.eq_ignore_ascii_case(repo))
}

/// Where the clone of `repo` is, from where the plugin started: that folder's repo, a folder called `repo` inside it or
/// beside it, or the same up one more level.
pub fn find_checkout(repo: &str) -> Option<String> {
	let here = std::env::current_dir().ok()?;
	if let Some(top) = git(Some(&here), &["rev-parse", "--show-toplevel"]) {
		let top = std::path::PathBuf::from(top);
		if is_checkout_of(&top, repo) {
			return Some(show(&top));
		}
	}
	let mut bases = vec![here.clone()];
	bases.extend(here.ancestors().skip(1).take(2).map(Path::to_path_buf));
	for base in bases {
		let Ok(entries) = std::fs::read_dir(&base) else { continue };
		for e in entries.flatten() {
			let path = e.path();
			if path.is_dir() && e.file_name().to_string_lossy().eq_ignore_ascii_case(repo) && is_checkout_of(&path, repo) {
				return Some(show(&path));
			}
		}
	}
	None
}

/// A path as text, with the separators the OS uses.
pub fn show(path: &Path) -> String {
	let text = path.to_string_lossy().into_owned();
	text.strip_prefix(r"\\?\").map_or(text.clone(), str::to_owned)
}

/// A folder in the picker.
pub struct Dir {
	pub name: String,
	pub path: std::path::PathBuf,
}

/// The folders inside `dir`, by name, without hidden ones. One read of the folder, no looking inside them.
pub fn subdirs(dir: &Path) -> Vec<Dir> {
	let mut v: Vec<Dir> = std::fs::read_dir(dir)
		.into_iter()
		.flatten()
		.flatten()
		.filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) || e.path().is_dir())
		.map(|e| Dir { name: e.file_name().to_string_lossy().into_owned(), path: e.path() })
		.filter(|d| !d.name.starts_with('.'))
		.collect();
	v.sort_by_key(|d| d.name.to_lowercase());
	v
}

/// How well `q` matches `hay` as a fuzzy (in-order, not necessarily touching) match, case-insensitive; None when it does not.
pub fn fuzzy(q: &str, hay: &str) -> Option<i32> {
	let q: Vec<char> = q.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
	let h: Vec<char> = hay.to_lowercase().chars().collect();
	let (mut qi, mut score, mut last) = (0, 0, None::<usize>);
	for (i, c) in h.iter().enumerate() {
		if qi < q.len() && *c == q[qi] {
			score += 10;
			if last.is_some_and(|l| l + 1 == i) {
				score += 15;
			}
			if i == 0 || !h[i - 1].is_alphanumeric() {
				score += 10;
			}
			last = Some(i);
			qi += 1;
		}
	}
	(qi == q.len()).then(|| score - h.len() as i32 / 4)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_prompt_is_one_argument() {
		assert_eq!(argv("claude", "a b\nc"), ["claude", "a b\nc"]);
		assert_eq!(argv("codex --model x {prompt}", "p q"), ["codex", "--model", "x", "p q"]);
		assert_eq!(argv("  ", "p"), ["claude", "p"]);
	}

	#[test]
	fn fuzzy_matches_in_order_and_prefers_tight_matches() {
		assert!(fuzzy("ldgr", "ledger").is_some());
		assert!(fuzzy("rgdl", "ledger").is_none());
		assert!(fuzzy("led", "ledger") > fuzzy("led", "a-l-e-d"));
		assert!(fuzzy("LED", "Ledger").is_some());
	}

	#[test]
	fn variables_fill_in_once_and_unknown_braces_stay() {
		let v = Vars { id: "7".into(), title: "uses {id}".into(), ..Vars::default() };
		assert_eq!(render("!{id} {title} {nope} { x", &v), "!7 uses {id} {nope} { x");
	}

	#[test]
	fn every_default_prompt_uses_only_known_variables() {
		let v = Vars::default();
		for k in Kind::ALL {
			let text = render(k.default_prompt(), &v);
			assert!(!text.contains("{id}") && !text.contains("{title}"), "{}", k.id());
		}
		for k in Kind::ALL {
			for word in k.default_prompt().split('{').skip(1) {
				let name = word.split('}').next().unwrap_or_default();
				assert!(NAMES.contains(&format!("{{{name}}}")) || name.is_empty() || name.contains(' '), "{} uses {{{name}}}", k.id());
			}
		}
	}
}
