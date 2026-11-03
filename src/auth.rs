//! The Authorization header. A personal access token wins when one is set
//! (`AZDO_PAT`, or saved with `tern-azdo pat`); otherwise the Azure CLI's
//! sign-in, so nothing is stored at all.

use std::{
	sync::Mutex,
	time::{Duration, Instant},
};

use anyhow::{Context, bail};
use base64::Engine;

/// Azure DevOps' resource id for Entra tokens.
const ADO_RESOURCE: &str = "499b84ac-1321-427f-aa17-267ca6975798";
/// The credential store's service name.
const SERVICE: &str = "tern-azdo";
/// How long an az token is reused. Tokens last about an hour; this stays well inside it.
const AZ_TOKEN_TTL: Duration = Duration::from_secs(40 * 60);

fn entry(org: &str) -> anyhow::Result<keyring::Entry> {
	keyring::Entry::new(SERVICE, org).context("opening the credential store")
}

pub fn store_pat(org: &str, pat: &str) -> anyhow::Result<()> {
	entry(org)?.set_password(pat).context("saving the token")
}

pub fn forget_pat(org: &str) -> anyhow::Result<()> {
	match entry(org)?.delete_credential() {
		Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
		Err(e) => Err(e.into()),
	}
}

fn saved_pat(org: &str) -> Option<String> {
	std::env::var("AZDO_PAT").ok().filter(|p| !p.is_empty()).or_else(|| entry(org).ok()?.get_password().ok())
}

pub struct Auth {
	org:    String,
	cached: Mutex<Option<(String, Instant)>>,
}

impl Auth {
	pub fn new(org: &str) -> Self {
		Self { org: org.to_owned(), cached: Mutex::new(None) }
	}

	/// The header value: `Basic …` for a PAT, `Bearer …` for an az token.
	pub fn header(&self) -> anyhow::Result<String> {
		if let Some(pat) = saved_pat(&self.org) {
			return Ok(format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!(":{pat}"))));
		}
		let mut cached = self.cached.lock().map_err(|_| anyhow::anyhow!("token cache poisoned"))?;
		if let Some((token, at)) = cached.as_ref()
			&& at.elapsed() < AZ_TOKEN_TTL
		{
			return Ok(format!("Bearer {token}"));
		}
		let token = az_token()?;
		*cached = Some((token.clone(), Instant::now()));
		Ok(format!("Bearer {token}"))
	}

	/// Drops a cached az token after a 401, so the next call asks az again.
	pub fn invalidate(&self) {
		if let Ok(mut c) = self.cached.lock() {
			*c = None;
		}
	}
}

fn az_token() -> anyhow::Result<String> {
	// `az` is a .cmd script on Windows, which Command does not resolve on its own.
	let program = if cfg!(windows) { "az.cmd" } else { "az" };
	let out = std::process::Command::new(program)
		.args(["account", "get-access-token", "--resource", ADO_RESOURCE, "--query", "accessToken", "-o", "tsv"])
		.output()
		.context("running `az` (install the Azure CLI, or save a PAT with `tern-azdo pat`)")?;
	if !out.status.success() {
		let err = String::from_utf8_lossy(&out.stderr);
		bail!("az could not get a token. Run `az login`, or save a PAT with `tern-azdo pat`. ({})", err.trim().lines().last().unwrap_or_default());
	}
	let token = String::from_utf8_lossy(&out.stdout).trim().to_owned();
	if token.is_empty() {
		bail!("az returned no token; run `az login`");
	}
	Ok(token)
}
