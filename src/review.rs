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
}

impl Kind {
	pub const ALL: [Kind; 4] = [Kind::Review, Kind::Context, Kind::Explain, Kind::Respond];

	pub fn id(self) -> &'static str {
		match self {
			Kind::Review => "review",
			Kind::Context => "context",
			Kind::Explain => "explain",
			Kind::Respond => "respond",
		}
	}

	pub fn label(self) -> &'static str {
		match self {
			Kind::Review => "review",
			Kind::Context => "context",
			Kind::Explain => "explain",
			Kind::Respond => "respond",
		}
	}

	pub fn default_prompt(self) -> &'static str {
		match self {
			Kind::Review => REVIEW,
			Kind::Context => CONTEXT,
			Kind::Explain => EXPLAIN,
			Kind::Respond => RESPOND,
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
}

/// The variables, for the hint above the prompt editor.
pub const NAMES: &str = "{id} {title} {url} {project} {repo} {source} {target} {author} {description} {files} {comments} {thread} {file} {line} {comment}";

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

/// The current directory when it is a checkout of `repo` (its `origin` ends in the repo's name).
pub fn guess_dir(repo: &str) -> Option<String> {
	let git = |args: &[&str]| {
		let out = std::process::Command::new("git").args(args).output().ok().filter(|o| o.status.success())?;
		Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())
	};
	let url = git(&["remote", "get-url", "origin"])?;
	let name = url.trim_end_matches('/').trim_end_matches(".git").rsplit(['/', ':']).next()?.to_lowercase();
	if name != repo.to_lowercase() {
		return None;
	}
	git(&["rev-parse", "--show-toplevel"]).map(|p| if cfg!(windows) { p.replace('/', "\\") } else { p })
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
