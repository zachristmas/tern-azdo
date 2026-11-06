//! Moving between pull requests: the rail beside the PR on wide panes, with projects that fold, and the picker (a search
//! field over a scrollable list, the same cards and filters) for narrow panes, a hidden rail and `Ctrl+K`.

use tern_sdk::{keys::Key, ui::html};

use super::{App, Msg, Pr, RAIL_MIN_COLS, SortMode, age, api, name_hue, short_name, vote_glyph};

/// The picker: what is typed, and the highlighted row.
pub(super) struct Nav {
	query: String,
	sel:   usize,
}

/// A row of the list: a project (when grouped by project) or a pull request.
enum NavRow {
	Head { name: String, count: usize, folded: bool },
	Pr(u32),
}

/// Where a row of the open picker is, for scrolling it into view.
fn row_path(i: usize) -> String {
	format!("main.pbody.navpick.npl.n{i}")
}

impl App {
	/// Whether the rail shows beside the PR: wide enough, and not hidden.
	pub(super) fn rail_shown(&self) -> bool {
		self.cols >= RAIL_MIN_COLS && !self.rail_hidden
	}

	/// The rows for what is typed: every PR matching each word, grouped by project (with folds) when sorting by project and
	/// nothing is typed, else a flat list in the chosen order.
	fn nav_rows(&self, query: &str) -> Vec<NavRow> {
		let words: Vec<String> = query.to_lowercase().split_whitespace().map(|w| w.trim_start_matches('!').to_owned()).collect();
		let list: Vec<&Pr> = self
			.visible()
			.into_iter()
			.filter(|p| {
				words.is_empty() || {
					let hay = format!("{} {} {} {} {} {}", p.pull_request_id, p.title, p.created_by.display_name, p.repository.name, p.repository.project.name, p.source()).to_lowercase();
					words.iter().all(|w| hay.contains(w.as_str()))
				}
			})
			.collect();
		let grouped = self.sort == SortMode::Project && words.is_empty();
		let mut rows = Vec::new();
		let mut last = "";
		for q in &list {
			let name = q.repository.project.name.as_str();
			if grouped && name != last {
				last = name;
				let count = list.iter().filter(|p| p.repository.project.name == name).count();
				rows.push(NavRow::Head { name: name.to_owned(), count, folded: self.folded.contains(name) });
			}
			if grouped && self.folded.contains(name) {
				continue;
			}
			rows.push(NavRow::Pr(q.pull_request_id));
		}
		rows
	}

	/// A project header that folds and unfolds its pull requests.
	fn head_row(&self, name: &str, count: usize, folded: bool, picked: bool, key: Option<String>) -> html::El<Msg> {
		let mut row = html::div()
			.class(if picked { "row ph pk" } else { "row ph" })
			.on_click(Msg::Fold(name.to_owned()))
			.child(html::span(if folded { "\u{25b8}" } else { "\u{25be}" }).class("az"))
			.child(html::span(name.to_owned()).class("nm"))
			.child(html::span(count.to_string()).class("n"));
		if let Some(k) = key {
			row = row.key(k);
		}
		row
	}

	/// A pull request as a card: the whole title, then id, author, age and votes; with its project and repo when the list is
	/// not grouped by project.
	fn pr_card(&self, q: &Pr, cur: Option<u32>, flat: bool, picked: bool, key: Option<String>) -> html::El<Msg> {
		let me = self.me_id();
		let id = q.pull_request_id;
		let name = q.repository.project.name.as_str();
		let mut meta = html::div().class("rm").child(html::span(format!("!{id}")));
		if q.is_draft {
			meta = meta.child(html::span("draft").class("chip"));
		}
		meta = meta.child(html::span(short_name(&q.created_by.display_name))).child(html::span(age(&q.creation_date)));
		let counts = api::vote_counts(q);
		for (vote, class, glyph) in [(10, "ok", "\u{2713}"), (5, "ok", "\u{2713}~"), (-5, "wait", "\u{23f8}"), (-10, "bad", "\u{2717}")] {
			if let Some(n) = counts.get(&vote) {
				meta = meta.child(html::span(format!("{glyph}{n}")).class(class));
			}
		}
		if let Some(v) = q.vote_of(me).filter(|v| *v != 0 && q.created_by.id != me) {
			let (g, class) = vote_glyph(v);
			meta = meta.child(html::span(format!("you {g}")).class(class));
		}
		let class = format!("rpr{}{}", if Some(id) == cur { " cur" } else { "" }, if picked { " pk" } else { "" });
		let mut card = html::div().class(class).on_click(Msg::Open(id));
		if let Some(k) = key {
			card = card.key(k);
		}
		if flat {
			card = card.child(
				html::div()
					.class("rpj")
					.child(html::span("").class(format!("pd av{}", name_hue(name))))
					.child(html::span(name.to_owned()))
					.child(html::span(format!("/ {}", q.repository.name)).class("rr")),
			);
		}
		card.child(html::div().class("rt").child(html::span(q.title.clone()))).child(meta)
	}

	/// The navigation rail: the PRs the filters leave, as cards, grouped by project with each project foldable.
	pub(super) fn rail(&self) -> html::El<Msg> {
		let mut rail = html::div().class("rail");
		let cur = self.page.as_ref().map(|p| p.pr.pull_request_id).or(self.sel);
		let rows = self.nav_rows("");
		if rows.is_empty() {
			let msg = if self.loading && self.prs.is_empty() { "Loading pull requests\u{2026}" } else { "No pull requests match these filters." };
			return rail.child(html::div().class("row rl").child(html::span(msg)));
		}
		let flat = self.sort != SortMode::Project;
		for row in &rows {
			match row {
				NavRow::Head { name, count, folded } => rail = rail.child(self.head_row(name, *count, *folded, false, None)),
				NavRow::Pr(id) => {
					if let Some(q) = self.prs.iter().find(|p| p.pull_request_id == *id) {
						rail = rail.child(self.pr_card(q, cur, flat, false, None));
					}
				},
			}
		}
		rail
	}

	/// The bar over the PR when the rail is not showing: a button for the picker, previous and next, and the rail's own
	/// button when the pane is wide enough for it.
	pub(super) fn nav_bar(&self) -> html::El<Msg> {
		let n = self.visible().len();
		let mut bar = html::div().class("topbar nbar");
		// The same button, in the same place, as the one on the rail's top bar.
		if self.cols >= RAIL_MIN_COLS {
			bar = bar.child(self.rail_toggle());
		}
		bar = bar.child(
			html::div()
				.class("ddb")
				.title("Choose a pull request (Ctrl+K)")
				.on_click(Msg::NavOpen)
				.child(html::span("\u{2630}").class("az"))
				.child(html::span(format!("Pull requests ({n})"))),
		);
		if self.page.is_some() {
			bar = bar
				.child(html::div().class("ddb").title("Previous pull request in this project ([)").on_click(Msg::StepPr(false)).child(html::span("\u{2039}")))
				.child(html::div().class("ddb").title("Next pull request in this project (])").on_click(Msg::StepPr(true)).child(html::span("\u{203a}")));
		}
		bar
	}

	/// The one button that hides and shows the sidebar: first on the bar, whichever bar is up.
	pub(super) fn rail_toggle(&self) -> html::El<Msg> {
		let title = if self.rail_shown() { "Hide the sidebar" } else { "Show the sidebar" };
		html::div().class("ddb").title(title).on_click(Msg::RailToggle).child(html::span("\u{25e7}"))
	}

	/// Open the picker on the open PR.
	pub(super) fn open_nav(&mut self) {
		self.menu = None;
		self.rv_menu = false;
		let cur = self.page.as_ref().map(|p| p.pr.pull_request_id).or(self.sel);
		let sel = self.nav_rows("").iter().position(|r| matches!(r, NavRow::Pr(id) if Some(*id) == cur)).unwrap_or(0);
		self.nav = Some(Nav { query: String::new(), sel });
		self.tree_req = Some(row_path(sel));
	}

	/// The picker's search text, when it is open.
	pub(super) fn nav_query(&self) -> Option<String> {
		self.nav.as_ref().map(|n| n.query.clone())
	}

	/// Open the picker with `text` typed in, on its first pull request.
	pub(super) fn search_nav(&mut self, text: &str) {
		self.open_nav();
		let sel = self.nav_rows(text).iter().position(|r| matches!(r, NavRow::Pr(_))).unwrap_or(0);
		if let Some(n) = &mut self.nav {
			n.query = text.to_owned();
			n.sel = sel;
		}
		self.tree_req = Some(row_path(sel));
	}

	/// Fold or unfold a project.
	pub(super) fn fold(&mut self, name: String) {
		if !self.folded.remove(&name) {
			self.folded.insert(name);
		}
	}

	/// A key while the picker is open: type to search, the arrows (and Tab, Ctrl+N and Ctrl+P) to cycle, Left and Right to
	/// fold the project, Enter to open.
	pub(super) fn nav_key(&mut self, key: &Key) {
		if key.is("escape") && self.menu.is_some() {
			self.menu = None;
			return;
		}
		let Some(query) = self.nav.as_ref().map(|n| n.query.clone()) else { return };
		let rows = self.nav_rows(&query);
		let count = rows.len();
		let sel = self.nav.as_ref().map_or(0, |n| n.sel).min(count.saturating_sub(1));
		let head_of = |at: usize| if count == 0 { None } else { rows[..=at.min(count - 1)].iter().rposition(|r| matches!(r, NavRow::Head { .. })) };
		let next;
		match key.name.as_str() {
			"escape" => {
				if query.is_empty() {
					self.nav = None;
				} else if let Some(n) = &mut self.nav {
					n.query.clear();
					n.sel = 0;
				}
				return;
			},
			"enter" => {
				match rows.get(sel) {
					Some(NavRow::Pr(id)) => self.open(*id),
					Some(NavRow::Head { name, .. }) => self.fold(name.clone()),
					None => {},
				}
				return;
			},
			"down" | "tab" if !key.shift => next = if count == 0 { 0 } else { (sel + 1) % count },
			"up" | "tab" => next = if count == 0 { 0 } else { (sel + count - 1) % count },
			"n" if key.ctrl => next = if count == 0 { 0 } else { (sel + 1) % count },
			"p" if key.ctrl => next = if count == 0 { 0 } else { (sel + count - 1) % count },
			"page_down" => next = (sel + 6).min(count.saturating_sub(1)),
			"page_up" => next = sel.saturating_sub(6),
			"home" => next = 0,
			"end" => next = count.saturating_sub(1),
			"left" | "right" => {
				// Fold or unfold the project the highlighted row is in, and stay on its header.
				let want_fold = key.name == "left";
				if let Some(h) = head_of(sel)
					&& let Some(NavRow::Head { name, folded, .. }) = rows.get(h)
					&& *folded != want_fold
				{
					let name = name.clone();
					self.fold(name.clone());
					let at = self.nav_rows(&query).iter().position(|r| matches!(r, NavRow::Head { name: n, .. } if *n == name)).unwrap_or(0);
					if let Some(n) = &mut self.nav {
						n.sel = at;
					}
					self.tree_req = Some(row_path(at));
				}
				return;
			},
			"backspace" => {
				if let Some(n) = &mut self.nav {
					n.query.pop();
					n.sel = 0;
				}
				self.tree_req = Some(row_path(0));
				return;
			},
			"u" if key.ctrl => {
				if let Some(n) = &mut self.nav {
					n.query.clear();
					n.sel = 0;
				}
				return;
			},
			_ => {
				if let Some(t) = key.typed().filter(|_| !key.ctrl && !key.alt) {
					let t = t.to_owned();
					if let Some(n) = &mut self.nav {
						n.query.push_str(&t);
						n.sel = 0;
					}
					self.tree_req = Some(row_path(0));
				}
				return;
			},
		}
		if let Some(n) = &mut self.nav {
			n.sel = next;
		}
		self.tree_req = Some(row_path(next));
	}

	/// The picker: a search field over a scrollable list of the same cards and the same filters. A sheet over the whole pane
	/// on a narrow one.
	pub(super) fn nav_view(&self) -> Option<html::El<Msg>> {
		let n = self.nav.as_ref()?;
		let rows = self.nav_rows(&n.query);
		let sel = n.sel.min(rows.len().saturating_sub(1));
		let cur = self.page.as_ref().map(|p| p.pr.pull_request_id).or(self.sel);
		let flat = self.sort != SortMode::Project || !n.query.trim().is_empty();
		let mut list = html::div().key("npl").class("npl");
		for (i, row) in rows.iter().enumerate() {
			let key = Some(format!("n{i}"));
			match row {
				NavRow::Head { name, count, folded } => list = list.child(self.head_row(name, *count, *folded, i == sel, key)),
				NavRow::Pr(id) => {
					if let Some(q) = self.prs.iter().find(|p| p.pull_request_id == *id) {
						list = list.child(self.pr_card(q, cur, flat, i == sel, key));
					}
				},
			}
		}
		if rows.is_empty() {
			list = list.child(html::div().class("row rl").child(html::span("No pull request matches.")));
		}
		let field = if n.query.is_empty() { html::span("Search pull requests").class("dim") } else { html::span(format!("{}\u{2581}", n.query)) };
		Some(
			html::div()
				.key("navpick")
				.class(if self.cols >= RAIL_MIN_COLS { "navpick wide" } else { "navpick" })
				.child(html::div().class("nph").child(html::div().class("aps").child(field)).child(html::span("\u{2715}").class("aix").on_click(Msg::NavClose)))
				.child(self.topbar())
				.child(list)
				.child(html::div().class("apf").child(
					html::span("type to search \u{b7} \u{2191}\u{2193} cycle \u{b7} \u{2190}\u{2192} fold a project \u{b7} Enter opens \u{b7} Esc closes").class("dim"),
				)),
		)
	}
}
