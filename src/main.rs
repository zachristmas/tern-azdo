//! `tern-azdo`: the interactive pull request view.
//! `tern-azdo list [review|mine|all]`: the active pull requests as a table.
//! `tern-azdo org [ORG]`: show or set the organization.
//! `tern-azdo pat [--forget]`: save a personal access token (read from stdin) instead of using `az`.

mod api;
mod app;
mod auth;
mod clip;
mod config;
mod diff;
mod worker;

use std::io::{BufRead, IsTerminal};

use anyhow::Context;
use tern_sdk::{
	PrintOptions,
	ui::{View, html},
};

const CSS: &str = ".prs{border-spacing:0} .prs th{text-align:left;padding:0 14px 3px 0;color:var(--sf-c-muted);font-weight:500} .prs td{padding:1px 14px 1px 0} .prs .id{color:var(--sf-c-muted)}";

fn org_or_bail() -> anyhow::Result<String> {
	config::resolve().context("no organization set: run `tern-azdo org <name or URL>`, set AZDO_ORG, or start in a clone of an Azure DevOps repo")
}

fn list(which: &str) -> anyhow::Result<()> {
	let client = api::Client::new(&org_or_bail()?);
	let me = client.me()?;
	let mut prs: Vec<api::Pr> = client
		.active_prs()?
		.into_iter()
		.filter(|p| match which {
			"mine" => p.created_by.id == me.id,
			"review" => p.created_by.id != me.id && p.reviewers.iter().any(|r| r.id == me.id),
			_ => true,
		})
		.collect();
	prs.sort_by(|a, b| (&a.repository.project.name, &a.repository.name, b.pull_request_id).cmp(&(&b.repository.project.name, &b.repository.name, a.pull_request_id)));
	let head = html::tr().children(["Id", "Repo", "Title", "Author", "Branches"].map(html::th));
	let rows = prs.iter().map(|p| {
		html::tr()
			.child(html::td(format!("!{}", p.pull_request_id)).class("id"))
			.child(html::td(format!("{} / {}", p.repository.project.name, p.repository.name)))
			.child(html::td(format!("{}{}", if p.is_draft { "[draft] " } else { "" }, p.title)))
			.child(html::td(p.created_by.display_name.clone()))
			.child(html::td(format!("{} → {}", p.source(), p.target())))
	});
	let view: View = View::new().main([html::table().class("prs").child(html::thead().child(head)).child(html::tbody().children(rows))]);
	tern_sdk::print(view, PrintOptions::new().css(CSS))?;
	Ok(())
}

fn main() -> anyhow::Result<()> {
	let args: Vec<String> = std::env::args().skip(1).collect();
	match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
		[] => app::run(config::resolve()),
		["list"] => list("all"),
		["list", which @ ("review" | "mine" | "all")] => list(which),
		["org"] => {
			match config::resolve() {
				Some(o) => println!("{o}"),
				None => println!("not set"),
			}
			Ok(())
		},
		["org", value] => {
			let org = config::normalize(value).context("that does not look like an Azure DevOps organization")?;
			config::save(&config::Config { org: org.clone() })?;
			println!("{org}");
			Ok(())
		},
		["pat", "--forget"] => {
			auth::forget_pat(&org_or_bail()?)?;
			println!("forgot the saved token; tern-azdo will use `az` again");
			Ok(())
		},
		["pat"] => {
			let org = org_or_bail()?;
			if std::io::stdin().is_terminal() {
				eprintln!("Paste a personal access token for {org} (Code: Read & Write), then Enter:");
			}
			let mut pat = String::new();
			std::io::stdin().lock().read_line(&mut pat)?;
			let pat = pat.trim();
			anyhow::ensure!(!pat.is_empty(), "no token given");
			auth::store_pat(&org, pat)?;
			println!("saved in the system credential store");
			Ok(())
		},
		_ => {
			eprintln!("usage: tern-azdo [list [review|mine|all] | org [ORG] | pat [--forget]]");
			std::process::exit(2);
		},
	}
}
