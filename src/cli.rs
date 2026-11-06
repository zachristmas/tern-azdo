//! Per-repo settings for agents and scripts: the operations behind `tern-azdo repo ...` and the MCP tools. Each takes
//! JSON arguments and returns JSON, so the two surfaces cannot drift apart.

use anyhow::{Context, bail};
use serde_json::{Value, json};

use crate::{config, review};

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
	if let Some(prompts) = args.get("prompts") {
		let prompts = prompts.as_object().context("`prompts` must map a kind to a template")?;
		for (id, template) in prompts {
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
			for name in ["dir", "agent"] {
				if let Some(v) = flag(&format!("--{name}")) {
					a[name] = json!(v);
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
		_ => bail!("usage: tern-azdo prompts | repos | repo show KEY | repo set KEY [--dir D] [--agent A] | repo prompt KEY KIND [--text T | --stdin | --reset] | repo remove KEY"),
	}
}

/// The tools the MCP server offers: name, description, JSON schema of the arguments, and the operation.
type Tool = (&'static str, &'static str, fn() -> Value, fn(&Value) -> anyhow::Result<Value>);

const TOOLS: [Tool; 5] = [
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
				"prompts": { "type": "object", "description": "kind (see azdo_ai_info) to template", "additionalProperties": { "type": "string" } }
			}, "required": ["repo"] })
		},
		set,
	),
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
