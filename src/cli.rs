//! Per-repo settings for agents and scripts: the operations behind `tern-azdo repo ...` and the MCP tools. Each takes
//! JSON arguments and returns JSON, so the two surfaces cannot drift apart.

use anyhow::{Context, bail};
use serde_json::{Value, json};

use crate::{config, remote, review};

fn kind_of(id: &str) -> anyhow::Result<review::Kind> {
	review::Kind::ALL.into_iter().find(|k| k.id() == id).with_context(|| {
		let all: Vec<&str> = review::Kind::ALL.iter().map(|k| k.id()).collect();
		format!("unknown prompt kind {id:?}; use one of {}", all.join(", "))
	})
}

fn key_of(args: &Value) -> anyhow::Result<String> {
	let key = args.get("repo").and_then(Value::as_str).context("`repo` is required, as project/repo")?.trim().to_lowercase();
	match key.split_once('/') {
		Some((p, r)) if !p.is_empty() && !r.is_empty() && !r.contains('/') => Ok(key),
		_ => bail!("`repo` must look like project/repo, got {key:?}"),
	}
}

fn text<'a>(args: &'a Value, name: &str) -> Option<&'a str> {
	args.get(name).and_then(Value::as_str)
}

/// The `{names}` in a template that no prompt variable fills.
fn unknown_variables(template: &str) -> Vec<String> {
	let mut out = Vec::new();
	for chunk in template.split('{').skip(1) {
		if let Some((name, _)) = chunk.split_once('}')
			&& !name.is_empty()
			&& name.chars().all(|c| c.is_ascii_lowercase() || c == '_')
			&& !review::NAMES.contains(&format!("{{{name}}}"))
		{
			out.push(format!("{{{name}}}"));
		}
	}
	out
}

fn describe(key: &str, r: &config::Review) -> Value {
	let mut prompts = serde_json::Map::new();
	for k in review::Kind::ALL {
		let custom = r.prompts.get(k.id()).filter(|t| !t.trim().is_empty());
		prompts.insert(k.id().to_owned(), json!({ "custom": custom.is_some(), "template": custom.map_or(k.default_prompt(), String::as_str) }));
	}
	json!({ "repo": key, "dir": r.dir, "agent": if r.agent.is_empty() { review::DEFAULT_AGENT } else { &r.agent }, "prompts": prompts })
}

/// What can be configured: the prompt kinds, their default templates, the variables, the agent setting and where it is saved.
pub fn info(_: &Value) -> anyhow::Result<Value> {
	let kinds: Vec<Value> = review::Kind::ALL.iter().map(|k| json!({ "id": k.id(), "title": k.title(), "default_template": k.default_prompt() })).collect();
	Ok(json!({
		"config_file": config::dir().join("config.json"),
		"repo_key": "project/repo, case-insensitive (the Azure DevOps project name and repo name)",
		"kinds": kinds,
		"variables": review::NAMES.split(' ').collect::<Vec<_>>(),
		"agent": "`claude`, `codex` or a command line; `{prompt}` marks where the prompt goes, else it is appended (default `claude`)",
		"dir": "A local checkout of the repo where the agent starts",
		"note": "Saved to the config file; the plugin reads it each time an AI action opens, so no restart is needed.",
	}))
}

pub fn list(_: &Value) -> anyhow::Result<Value> {
	let config = config::load().unwrap_or_default();
	Ok(Value::Array(config.repos.iter().map(|(k, r)| describe(k, r)).collect()))
}

pub fn get(args: &Value) -> anyhow::Result<Value> {
	let key = key_of(args)?;
	let r = config::review(&key).unwrap_or_default();
	Ok(describe(&key, &r))
}

/// Set a repo's checkout folder, agent and prompt templates. `prompts` maps a kind to a template; an empty string
/// puts the default back.
pub fn set(args: &Value) -> anyhow::Result<Value> {
	let key = key_of(args)?;
	let mut r = config::review(&key).unwrap_or_default();
	let mut warnings = Vec::new();
	if let Some(dir) = text(args, "dir") {
		if !dir.is_empty() && !std::path::Path::new(dir).is_dir() {
			warnings.push(format!("{dir} is not a folder on this machine"));
		}
		r.dir = dir.to_owned();
	}
	if let Some(agent) = text(args, "agent") {
		r.agent = if agent.trim().is_empty() { review::DEFAULT_AGENT.to_owned() } else { agent.trim().to_owned() };
	}
	if r.agent.is_empty() {
		r.agent = review::DEFAULT_AGENT.to_owned();
	}
	// Templates can come from a JSON file (kind to template), so none of their text goes through a shell; `prompts` wins over it.
	let mut prompts = serde_json::Map::new();
	if let Some(path) = text(args, "prompts_file") {
		let body = std::fs::read_to_string(path).with_context(|| format!("could not read {path}"))?;
		let file: Value = serde_json::from_str(&body).with_context(|| format!("{path} is not JSON"))?;
		prompts.extend(file.as_object().context("the prompts file must be a JSON object of kind to template")?.clone());
	}
	if let Some(given) = args.get("prompts") {
		prompts.extend(given.as_object().context("`prompts` must map a kind to a template")?.clone());
	}
	{
		for (id, template) in &prompts {
			let kind = kind_of(id)?;
			let template = template.as_str().context("a template must be a string")?;
			let unknown = unknown_variables(template);
			if !unknown.is_empty() {
				warnings.push(format!("{id}: {} is not a known variable and will be sent as written", unknown.join(" ")));
			}
			// A template equal to the default is not saved, so the default can improve later.
			if template.trim().is_empty() || template == kind.default_prompt() {
				r.prompts.remove(kind.id());
			} else {
				r.prompts.insert(kind.id().to_owned(), template.to_owned());
			}
		}
	}
	config::save_review(&key, r.clone())?;
	let mut out = describe(&key, &r);
	out["warnings"] = json!(warnings);
	Ok(out)
}

/// The state of the file tree, cut down to what a caller needs: the files shown, which are selected.
fn files_summary(state: Value) -> Value {
	let files = state["files"].as_array().cloned().unwrap_or_default();
	let shown: Vec<Value> = files.iter().filter(|f| f["shown"] == true).map(|f| json!({ "path": f["path"], "status": f["status"], "selected": f["selected"] })).collect();
	let selected: Vec<Value> = files.iter().filter(|f| f["selected"] == true).map(|f| f["path"].clone()).collect();
	json!({ "pr": state["pr"], "tab": state["tab"], "filter": state["filter"], "filter_error": state["filter_error"], "total": files.len(), "shown": shown, "selected": selected })
}

/// The pull request on screen, its file tree's filter, the files shown and the files selected.
pub fn files(_: &Value) -> anyhow::Result<Value> {
	Ok(files_summary(remote::current()?))
}

/// Filter the file tree on screen. Terms are separated by commas; a term is a fuzzy name, a `*.cs` glob or a `/regex/`,
/// and `!` in front leaves out what it matches. An empty filter shows everything.
pub fn files_filter(args: &Value) -> anyhow::Result<Value> {
	let text = text(args, "text").context("`text` is required (\"\" clears the filter)")?;
	Ok(files_summary(remote::run(json!({ "cmd": "filter", "text": text }))?))
}

/// Select files in the file tree on screen, for "Review selected" and "Add selected as context". `text` is a filter like
/// `files_filter` takes (omitted: the files the tree shows), or `paths` names them. `mode` is set (default), add, remove or clear.
pub fn files_select(args: &Value) -> anyhow::Result<Value> {
	let mut cmd = json!({ "cmd": "select" });
	for name in ["text", "paths", "mode"] {
		if let Some(v) = args.get(name).filter(|v| !v.is_null()) {
			cmd[name] = v.clone();
		}
	}
	Ok(files_summary(remote::run(cmd)?))
}

/// Open a pull request from the list on its Files tab.
pub fn open_pr(args: &Value) -> anyhow::Result<Value> {
	let id = args.get("pr").and_then(Value::as_u64).context("`pr` is required: the pull request number")?;
	Ok(files_summary(remote::run(json!({ "cmd": "open", "pr": id }))?))
}

/// Whether every word of `query` is in `item` (anywhere in its JSON, any case).
fn has_words(item: &Value, query: &str) -> bool {
	let hay = item.to_string().to_lowercase();
	query.to_lowercase().split_whitespace().all(|w| hay.contains(w))
}

/// What the view shows. Lists are left out unless asked for: `prs` and `threads` take words to narrow them (or true for
/// all), `files` true.
fn view_of(state: Value, args: &Value) -> Value {
	let narrowed = |name: &str, list: &Value| -> Option<Value> {
		let items = list.as_array()?;
		match args.get(name)? {
			Value::String(q) if !q.trim().is_empty() => Some(Value::Array(items.iter().filter(|i| has_words(i, q)).cloned().collect())),
			Value::Bool(false) | Value::Null => None,
			_ => Some(list.clone()),
		}
	};
	let mut v = json!({
		"pr": state["pr"], "tab": state["tab"], "file": state["file"], "thread": state["thread"], "filter": state["filter"],
		"filter_error": state["filter_error"], "picker": state["picker"],
		"counts": { "prs": state["prs"].as_array().map_or(0, Vec::len), "files": state["files"].as_array().map_or(0, Vec::len), "threads": state["threads"].as_array().map_or(0, Vec::len) },
	});
	for name in ["prs", "threads", "files"] {
		if let Some(list) = narrowed(name, &state[name]) {
			v[name] = list;
		}
	}
	if !args["files"].is_null() && args["files"] != false {
		v["selected"] = Value::Array(state["files"].as_array().map_or_else(Vec::new, |f| f.iter().filter(|x| x["selected"] == true).map(|x| x["path"].clone()).collect()));
	}
	v
}

/// Where the user is in the tern-azdo view, and what they could be taken to: the pull requests, and the open one's files and
/// comments, each narrowed by words.
pub fn view(args: &Value) -> anyhow::Result<Value> {
	Ok(view_of(remote::current()?, args))
}

/// Take the user somewhere: a pull request (`pr`), its `tab` (overview or files), a file-tree `filter`, a `file` (a path, or
/// words the path matches), a `line` of it, or a comment `thread` (`view`: overview to show it in the overview instead of on the
/// diff). Any mix; they apply in that order. Returns where the view ended up.
pub fn go(args: &Value) -> anyhow::Result<Value> {
	navigate("go", args)
}

/// Reload the pull request on screen and the list (new commits, new comments), then go where `azdo_go` would, once the
/// new data is in. Use it after the code or the comments changed.
pub fn refresh(args: &Value) -> anyhow::Result<Value> {
	navigate("refresh", args)
}

fn navigate(command: &str, args: &Value) -> anyhow::Result<Value> {
	let mut cmd = json!({ "cmd": command });
	for name in ["pr", "tab", "filter", "file", "line", "thread", "view"] {
		if let Some(v) = args.get(name).filter(|v| !v.is_null()) {
			cmd[name] = v.clone();
		}
	}
	Ok(view_of(remote::run(cmd)?, &json!({})))
}

/// Open the pull request picker with `text` typed in, in front of the user. Returns the pull requests that match it.
pub fn search(args: &Value) -> anyhow::Result<Value> {
	let q = text(args, "text").context("`text` is required")?;
	let state = remote::run(json!({ "cmd": "search", "text": q }))?;
	Ok(view_of(state, &json!({ "prs": q })))
}

/// Close the picker and any dialog.
pub fn dismiss(_: &Value) -> anyhow::Result<Value> {
	Ok(view_of(remote::run(json!({ "cmd": "dismiss" }))?, &json!({})))
}

pub fn remove(args: &Value) -> anyhow::Result<Value> {
	let key = key_of(args)?;
	let mut c = config::load().unwrap_or_default();
	let removed = c.repos.remove(&key).is_some();
	if removed {
		config::save(&c)?;
	}
	Ok(json!({ "repo": key, "removed": removed }))
}

/// `tern-azdo repo ...` and `tern-azdo prompts`. Returns what to print.
pub fn command(args: &[&str]) -> anyhow::Result<String> {
	let pretty = |v: Value| serde_json::to_string_pretty(&v).unwrap_or_default();
	// `--name value` pairs after the positional words.
	let flag = |name: &str| args.iter().position(|a| *a == name).and_then(|i| args.get(i + 1)).map(|s| (*s).to_owned());
	match args {
		["prompts"] => Ok(pretty(info(&json!({}))?)),
		["repos"] | ["repo", "list"] => Ok(pretty(list(&json!({}))?)),
		["repo", "show", key] => Ok(pretty(get(&json!({ "repo": key }))?)),
		["repo", "remove", key] => Ok(pretty(remove(&json!({ "repo": key }))?)),
		["repo", "set", key, ..] => {
			let mut a = json!({ "repo": key });
			for (name, key) in [("dir", "dir"), ("agent", "agent"), ("prompts-file", "prompts_file")] {
				if let Some(v) = flag(&format!("--{name}")) {
					a[key] = json!(v);
				}
			}
			Ok(pretty(set(&a)?))
		},
		// `repo prompt KEY KIND` prints the effective template; `--text T`, `--stdin` or `--reset` change it.
		["repo", "prompt", key, kind, rest @ ..] => {
			let k = kind_of(kind)?;
			let new = if rest.contains(&"--reset") {
				Some(String::new())
			} else if rest.contains(&"--stdin") {
				let mut s = String::new();
				std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
				Some(s.trim_end_matches(['\r', '\n']).to_owned())
			} else {
				flag("--text")
			};
			match new {
				Some(t) => Ok(pretty(set(&json!({ "repo": key, "prompts": { k.id(): t } }))?)),
				None => {
					let r = config::review(&key.to_lowercase()).unwrap_or_default();
					Ok(r.prompts.get(k.id()).filter(|t| !t.trim().is_empty()).map_or(k.default_prompt(), String::as_str).to_owned())
				},
			}
		},
		["files"] => Ok(pretty(files(&json!({}))?)),
		["files", "clear"] => Ok(pretty(files_select(&json!({ "mode": "clear" }))?)),
		["files", "filter", rest @ ..] => Ok(pretty(files_filter(&json!({ "text": rest.join(" ") }))?)),
		// `files select [TEXT] [--mode add|remove|set]`: no text selects what the tree shows.
		["files", "select", rest @ ..] => {
			let words: Vec<&str> = rest.iter().copied().filter(|w| *w != "--mode" && Some((*w).to_owned()) != flag("--mode")).collect();
			let mut a = json!({});
			if !words.is_empty() {
				a["text"] = json!(words.join(" "));
			}
			if let Some(m) = flag("--mode") {
				a["mode"] = json!(m);
			}
			Ok(pretty(files_select(&a)?))
		},
		["view", rest @ ..] => {
			let mut a = json!({});
			if rest.contains(&"--files") {
				a["files"] = json!(true);
			}
			for name in ["prs", "threads"] {
				if let Some(v) = flag(&format!("--{name}")) {
					a[name] = json!(v);
				}
			}
			Ok(pretty(view(&a)?))
		},
		["search", rest @ ..] => Ok(pretty(search(&json!({ "text": rest.join(" ") }))?)),
		["dismiss"] => Ok(pretty(dismiss(&json!({}))?)),
		// `go [--pr N] [--tab overview|files] [--filter F] [--file F] [--line N] [--thread ID] [--view overview]`
		["go" | "refresh", ..] => {
			let mut a = json!({});
			for name in ["pr", "line", "thread"] {
				if let Some(v) = flag(&format!("--{name}")) {
					a[name] = json!(v.parse::<u64>().with_context(|| format!("--{name} takes a number"))?);
				}
			}
			for name in ["tab", "filter", "file", "view"] {
				if let Some(v) = flag(&format!("--{name}")) {
					a[name] = json!(v);
				}
			}
			Ok(pretty(if args[0] == "refresh" { refresh(&a)? } else { go(&a)? }))
		},
		["open", id] => Ok(pretty(open_pr(&json!({ "pr": id.parse::<u64>().context("the pull request number")? }))?)),
		_ => bail!("usage: tern-azdo prompts | repos | repo show KEY | repo set KEY [--dir D] [--agent A] | repo prompt KEY KIND [--text T | --stdin | --reset] | repo remove KEY | files [filter TEXT | select [TEXT] [--mode M] | clear] | view [--prs W] [--threads W] [--files] | refresh [--file F] [--line N] | go [--pr N] [--tab T] [--filter F] [--file F] [--line N] [--thread ID] | search TEXT | dismiss | open PR"),
	}
}

/// The tools the MCP server offers: name, description, JSON schema of the arguments, and the operation.
type Tool = (&'static str, &'static str, fn() -> Value, fn(&Value) -> anyhow::Result<Value>);

const TOOLS: [Tool; 14] = [
	("azdo_ai_info", "What can be configured for the AI actions in tern-azdo: prompt kinds with their default templates, the template variables, the agent setting. Read this first.", || json!({ "type": "object", "properties": {} }), info),
	("azdo_ai_repos", "List the repos that have saved AI settings, with their effective prompts.", || json!({ "type": "object", "properties": {} }), list),
	("azdo_ai_repo_get", "One repo's checkout folder, agent and effective prompt templates (a default is reported with custom=false).", || json!({ "type": "object", "properties": { "repo": { "type": "string", "description": "project/repo" } }, "required": ["repo"] }), get),
	(
		"azdo_ai_repo_set",
		"Set a repo's checkout folder, agent command and/or prompt templates. Omitted fields keep their value. A template of \"\" restores the default. Returns the result and any warnings (unknown variables, missing folder).",
		|| {
			json!({ "type": "object", "properties": {
				"repo": { "type": "string", "description": "project/repo" },
				"dir": { "type": "string", "description": "local checkout folder" },
				"agent": { "type": "string", "description": "claude, codex, or a command line with an optional {prompt}" },
				"prompts": { "type": "object", "description": "kind (see azdo_ai_info) to template", "additionalProperties": { "type": "string" } },
				"prompts_file": { "type": "string", "description": "path to a JSON file of kind to template; use it for long templates or ones with | < > or quotes, so none of the text goes through a shell" }
			}, "required": ["repo"] })
		},
		set,
	),
	("azdo_files", "The pull request open in the tern-azdo view, its file tree's filter, the files shown and the files selected.", || json!({ "type": "object", "properties": {} }), files),
	(
		"azdo_files_filter",
		"Filter the file tree in the tern-azdo view. Comma-separated terms, any of which may match: a plain term is a fuzzy match on the path (letters in order), `*.cs` or `src/**/Tax*` is a glob, `/regex/` is a case-insensitive regex (commas allowed inside), and a leading `!` leaves out what the term matches. \"\" clears it. Returns the files now shown.",
		|| json!({ "type": "object", "properties": { "text": { "type": "string", "description": "e.g. \"*.vue, /Order|Tax/, !tests\"" } }, "required": ["text"] }),
		files_filter,
	),
	(
		"azdo_files_select",
		"Select files in the file tree, so the human can press Review selected or Add selected as context (or an agent can ask them to). `text` takes the same filter syntax as azdo_files_filter; omit it to select what the tree currently shows; `paths` lists exact paths instead. `mode`: set (default), add, remove or clear.",
		|| {
			json!({ "type": "object", "properties": {
				"text": { "type": "string" },
				"paths": { "type": "array", "items": { "type": "string" } },
				"mode": { "type": "string", "enum": ["set", "add", "remove", "clear"] }
			} })
		},
		files_select,
	),
	("azdo_open_pr", "Open a pull request from the list (by number) on its Files tab. Its files load a moment later; call azdo_files to see them.", || json!({ "type": "object", "properties": { "pr": { "type": "integer" } }, "required": ["pr"] }), open_pr),
	(
		"azdo_view",
		"Where the user is in the tern-azdo view (pull request, tab, file, selected comment, file filter, picker) and counts. Ask for lists to find where to take them: `prs` (words, e.g. \"ledger tax\", or true), `threads` (words matched against comment text, author, file, or true), `files` (true). Thread ids and file paths from here feed azdo_go.",
		|| json!({ "type": "object", "properties": { "prs": { "type": ["string", "boolean"] }, "threads": { "type": ["string", "boolean"] }, "files": { "type": "boolean" } } }),
		view,
	),
	(
		"azdo_go",
		"Take the user somewhere in the tern-azdo view, any mix: `pr` (number, opened from the list), `tab` (overview or files), `filter` (file tree filter, same syntax as azdo_files_filter, e.g. \"*.config\"), `file` (a path or words it matches), `line` (a line of that file's new side), `thread` (a comment thread id from azdo_view; `view`: \"overview\" shows it in the overview, else it is shown on the diff when it has a file). Returns where the view ended up.",
		|| {
			json!({ "type": "object", "properties": {
				"pr": { "type": "integer" }, "tab": { "type": "string", "enum": ["overview", "files"] }, "filter": { "type": "string" },
				"file": { "type": "string" }, "line": { "type": "integer" }, "thread": { "type": "integer" }, "view": { "type": "string", "enum": ["overview", "diff"] }
			} })
		},
		go,
	),
	(
		"azdo_refresh",
		"Reload the pull request open in the tern-azdo view (its commits, files, diffs and comments) and the list, then optionally take the user somewhere with the same fields as azdo_go (file, line, thread, filter, tab). Call it after code was pushed to the PR's branch or a comment was added, so the user sees the change. Returns where the view ended up.",
		|| {
			json!({ "type": "object", "properties": {
				"tab": { "type": "string", "enum": ["overview", "files"] }, "filter": { "type": "string" },
				"file": { "type": "string" }, "line": { "type": "integer" }, "thread": { "type": "integer" }, "view": { "type": "string", "enum": ["overview", "diff"] }
			} })
		},
		refresh,
	),
	("azdo_search", "Open the pull request picker in front of the user with words typed in, and return the matching pull requests. Use azdo_go with a pr to open one, or azdo_dismiss to close the picker.", || json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }), search),
	("azdo_dismiss", "Close the pull request picker and any dialog in the tern-azdo view.", || json!({ "type": "object", "properties": {} }), dismiss),
	("azdo_ai_repo_remove", "Forget a repo's saved AI settings (back to defaults).", || json!({ "type": "object", "properties": { "repo": { "type": "string", "description": "project/repo" } }, "required": ["repo"] }), remove),
];

/// A Model Context Protocol server on stdin and stdout: newline-delimited JSON-RPC.
pub fn serve() -> anyhow::Result<()> {
	use std::io::{BufRead, Write};
	let stdin = std::io::stdin();
	let mut out = std::io::stdout().lock();
	for line in stdin.lock().lines() {
		let line = line?;
		let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
		// Notifications carry no id and get no answer.
		let Some(id) = msg.get("id").cloned() else { continue };
		let params = msg.get("params").cloned().unwrap_or(Value::Null);
		let result: Result<Value, (i64, String)> = match msg.get("method").and_then(Value::as_str).unwrap_or_default() {
			"initialize" => Ok(json!({
				"protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2024-11-05"),
				"capabilities": { "tools": {} },
				"serverInfo": { "name": "tern-azdo", "version": env!("CARGO_PKG_VERSION") },
			})),
			"ping" => Ok(json!({})),
			"tools/list" => Ok(json!({ "tools": TOOLS.iter().map(|(n, d, schema, _)| json!({ "name": n, "description": d, "inputSchema": schema() })).collect::<Vec<_>>() })),
			"tools/call" => {
				let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
				match TOOLS.iter().find(|t| t.0 == name) {
					None => Err((-32602, format!("unknown tool {name}"))),
					Some((_, _, _, run)) => {
						let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
						Ok(match run(&args) {
							Ok(v) => json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v)? }] }),
							Err(e) => json!({ "isError": true, "content": [{ "type": "text", "text": format!("{e:#}") }] }),
						})
					},
				}
			},
			other => Err((-32601, format!("method not found: {other}"))),
		};
		let reply = match result {
			Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
			Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
		};
		writeln!(out, "{reply}")?;
		out.flush()?;
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn repo_keys_are_checked_and_lowercased() {
		assert_eq!(key_of(&json!({ "repo": " ECS Api/ECS API " })).unwrap(), "ecs api/ecs api");
		assert!(key_of(&json!({ "repo": "nope" })).is_err());
		assert!(key_of(&json!({ "repo": "a/b/c" })).is_err());
	}

	#[test]
	fn unknown_variables_are_found() {
		assert_eq!(unknown_variables("{id} {title} {nope} and {{x}"), vec!["{nope}".to_owned(), "{x}".to_owned()]);
		assert!(unknown_variables("{id} {code}").is_empty());
	}

	#[test]
	fn kinds_are_by_id() {
		assert!(kind_of("explain_code").is_ok());
		assert!(kind_of("bogus").unwrap_err().to_string().contains("review_code"));
	}
}
