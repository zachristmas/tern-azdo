//! How a script or an agent steers the running view: commands go into a file next to `config.json` that the view reads about
//! once a second, and the view writes what it shows to `state.json`. `tern-azdo files ...` and the MCP tools use it.

use std::{
	io::Write,
	path::PathBuf,
	time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, bail};
use serde_json::{Value, json};

use crate::config;

fn inbox() -> PathBuf {
	config::dir().join("inbox.jsonl")
}

fn state_path() -> PathBuf {
	config::dir().join("state.json")
}

/// Queue a command for the view. Returns its id: the state's `applied` reaches it once the view has acted.
pub fn send(mut command: Value) -> anyhow::Result<u64> {
	let id = SystemTime::now().duration_since(UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
	command["id"] = json!(id);
	std::fs::create_dir_all(config::dir())?;
	let mut file = std::fs::OpenOptions::new().create(true).append(true).open(inbox())?;
	writeln!(file, "{command}")?;
	Ok(id)
}

/// The queued commands, removed from the queue.
pub fn take() -> Vec<Value> {
	let (from, to) = (inbox(), config::dir().join("inbox.taken"));
	if !from.exists() {
		return Vec::new();
	}
	let _ = std::fs::remove_file(&to);
	if std::fs::rename(&from, &to).is_err() {
		return Vec::new();
	}
	let text = std::fs::read_to_string(&to).unwrap_or_default();
	let _ = std::fs::remove_file(&to);
	text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

pub fn write_state(state: &Value) {
	let _ = std::fs::create_dir_all(config::dir());
	let _ = std::fs::write(state_path(), serde_json::to_string(state).unwrap_or_default());
}

pub fn state() -> Option<Value> {
	serde_json::from_str(&std::fs::read_to_string(state_path()).ok()?).ok()
}

/// Send a command and wait for the view to act on it. Returns the state after it.
pub fn run(command: Value) -> anyhow::Result<Value> {
	let id = send(command)?;
	for _ in 0..100 {
		std::thread::sleep(Duration::from_millis(100));
		if let Some(s) = state().filter(|s| s["applied"].as_u64().is_some_and(|a| a >= id)) {
			return Ok(s);
		}
	}
	bail!("the tern-azdo view did not answer: is it running in Tern?")
}

/// What the view last showed.
pub fn current() -> anyhow::Result<Value> {
	state().context("no state yet: start tern-azdo in Tern first")
}
