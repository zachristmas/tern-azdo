//! Which organization to talk to. `AZDO_ORG` wins, then the saved config,
//! then the `origin` remote of the directory tern-azdo starts in.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What `config.json` holds. No secrets.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Config {
	/// `https://dev.azure.com/<org>`.
	pub org: String,
}

/// The folder for `config.json`.
pub fn dir() -> PathBuf {
	#[cfg(windows)]
	let base = std::env::var_os("APPDATA").map(PathBuf::from);
	#[cfg(not(windows))]
	let base = std::env::var_os("XDG_CONFIG_HOME")
		.map(PathBuf::from)
		.or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
	base.unwrap_or_default().join("tern-azdo")
}

fn path() -> PathBuf {
	dir().join("config.json")
}

pub fn load() -> Option<Config> {
	let text = std::fs::read_to_string(path()).ok()?;
	serde_json::from_str(&text).ok()
}

pub fn save(config: &Config) -> anyhow::Result<()> {
	std::fs::create_dir_all(dir())?;
	std::fs::write(path(), serde_json::to_string_pretty(config)?)?;
	Ok(())
}

/// `https://dev.azure.com/<org>` from an org name, a dev.azure.com or visualstudio.com URL,
/// or a git remote (HTTPS or SSH).
pub fn normalize(input: &str) -> Option<String> {
	let s = input.trim().trim_end_matches('/');
	if s.is_empty() {
		return None;
	}
	// SSH: git@ssh.dev.azure.com:v3/<org>/<project>/<repo>, <org>@vs-ssh.visualstudio.com:v3/<org>/...
	if let Some(rest) = s.split_once(":v3/").map(|(_, r)| r) {
		return rest.split('/').next().filter(|o| !o.is_empty()).map(|o| format!("https://dev.azure.com/{o}"));
	}
	let no_scheme = s.split_once("://").map_or(s, |(_, r)| r);
	// Drop any user@ prefix (HTTPS remotes carry one).
	let host_path = no_scheme.rsplit_once('@').map_or(no_scheme, |(_, r)| r);
	let mut parts = host_path.split('/');
	let host = parts.next().unwrap_or_default().to_lowercase();
	if host == "dev.azure.com" {
		return parts.next().filter(|o| !o.is_empty()).map(|o| format!("https://dev.azure.com/{o}"));
	}
	if let Some(org) = host.strip_suffix(".visualstudio.com") {
		return Some(format!("https://dev.azure.com/{org}"));
	}
	// A bare org name.
	if !s.contains(['/', ':', '.', ' ']) {
		return Some(format!("https://dev.azure.com/{s}"));
	}
	None
}

/// The org of the `origin` remote in the current directory, if it is on Azure DevOps.
pub fn detect() -> Option<String> {
	let out = std::process::Command::new("git").args(["remote", "get-url", "origin"]).output().ok()?;
	if !out.status.success() {
		return None;
	}
	let url = String::from_utf8_lossy(&out.stdout).trim().to_owned();
	if url.contains("dev.azure.com") || url.contains("visualstudio.com") { normalize(&url) } else { None }
}

/// The org to use, if one can be found.
pub fn resolve() -> Option<String> {
	if let Some(o) = std::env::var("AZDO_ORG").ok().and_then(|o| normalize(&o)) {
		return Some(o);
	}
	if let Some(c) = load().and_then(|c| normalize(&c.org)) {
		return Some(c);
	}
	detect()
}

/// The org's short name, for the status bar.
pub fn short(org: &str) -> &str {
	org.rsplit('/').next().unwrap_or(org)
}

#[cfg(test)]
mod tests {
	use super::normalize;

	#[test]
	fn every_remote_shape_becomes_the_org_url() {
		let want = Some("https://dev.azure.com/Contoso".to_owned());
		for input in [
			"Contoso",
			"https://dev.azure.com/Contoso",
			"https://dev.azure.com/Contoso/",
			"https://dev.azure.com/Contoso/Proj/_git/Repo",
			"https://Contoso@dev.azure.com/Contoso/Proj/_git/Repo",
			"git@ssh.dev.azure.com:v3/Contoso/Proj/Repo",
			"Contoso@vs-ssh.visualstudio.com:v3/Contoso/Proj/Repo",
		] {
			assert_eq!(normalize(input), want, "{input}");
		}
		assert_eq!(normalize("https://contoso.visualstudio.com/Proj/_git/Repo"), Some("https://dev.azure.com/contoso".to_owned()));
		assert_eq!(normalize("https://github.com/x/y"), None);
		assert_eq!(normalize(""), None);
	}
}
