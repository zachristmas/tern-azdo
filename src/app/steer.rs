//! The file tree's selection and filter: the box and buttons above the tree, checking files, and the commands that come from
//! outside (`tern-azdo files ...`, the MCP tools) through [`remote`].

use std::collections::HashSet;

use serde_json::{Value, json};
use tern_sdk::ui::{self, html};

use crate::config;

use super::{AiReq, App, ComposeKind, SizeKind, Composer, FILES_DS, JUMP_LEAD, Msg, OV_MAIN, Page, PrTab, ScrollReq, api, filter, remote, review, tree};

impl Page {
	/// A path as the filter and the commands see it: no leading slash.
	fn bare(path: &str) -> &str {
		path.trim_start_matches('/')
	}
}

impl App {
	/// Above the file tree: the filter, and what is checked with what to do with it.
	pub(super) fn tree_head(&self, p: &Page) -> html::El<Msg> {
		let editing = self.compose.as_ref().filter(|c| c.kind == ComposeKind::Files);
		let field: tern_sdk::Node<Msg> = match editing {
			Some(c) => html::div()
				.class("tfr")
				.child(
					ui::editor()
				.key("tfed")
				.text(c.draft.clone())
				.cursor(c.utf16_cursor())
				.placeholder("Filter: name, *.cs, /regex/, !leave out (Enter keeps, Esc clears)")
				.sendable(true)
				.max_lines(1u32)
				.on_send(|t| Msg::Send(t.to_owned()))
				.on_edit(|e| Msg::Edit(e.clone()))
				.on_focus(Msg::Focus),
				)
				.child(if c.draft.is_empty() { html::span("") } else { html::span("\u{2715}").class("aix").title("Clear the filter").on_click(Msg::FilesFilterClear) })
				.into(),
			None => {
				let text = p.filter.text().trim();
				let mut f = html::div().class("tff").title("Filter the files (F). Commas separate terms: fuzzy name, *.cs glob, /regex/, !leave out").on_click(Msg::FilesFilter).child(html::span("\u{2315}").class("dim"));
				f = f.child(if text.is_empty() { html::span("Filter files\u{2026}").class("dim") } else { html::span(text.to_owned()) });
				if !text.is_empty() {
					f = f.child(html::span("\u{2715}").class("aix").title("Clear the filter").on_click(Msg::FilesFilterClear));
				}
				f.into()
			},
		};
		let mut head = html::div().key("th").class("azth").child(field);
		if let Some(e) = p.filter.error() {
			head = head.child(html::div().class("tfe").child(html::span(e.to_owned())));
		}
		let paths = p.paths();
		let shown = p.matching();
		let n = p.checked.len();
		let on = shown.iter().filter(|i| p.checked.contains(paths[**i])).count();
		let mark = if !shown.is_empty() && on == shown.len() {
			"\u{2611}"
		} else if on > 0 {
			"\u{25a3}"
		} else {
			"\u{2610}"
		};
		let label = match (n, p.filter.is_active()) {
			(0, true) => format!("Select the {} shown", shown.len()),
			(0, false) => format!("Select all {}", shown.len()),
			_ => format!("{n} selected"),
		};
		head = head.child(
			html::div()
				.class("tsel")
				.child(html::span(mark).class("ck").title("Select or unselect every shown file (Ctrl+A)").on_click(Msg::CheckAll))
				.child(html::span(label).on_click(Msg::CheckAll))
				.child(self.size_menu()),
		);
		if n > 0 {
			let (r, c) = (review::Kind::ReviewFiles, review::Kind::ContextFiles);
			head = head.child(
				html::div()
					.class("tsa")
					.child(html::span(self.ai_label("Review selected", r, None)).class(self.ai_class("tsb", r, None)).title("Start an AI agent that reviews the selected files (Shift+R)").on_click(Msg::Ai(r, None)))
					.child(html::span(self.ai_label("Add selected as context", c, None)).class(self.ai_class("tsb sec", c, None)).title("Start an AI agent that has read the selected files and waits for your questions (Shift+T)").on_click(Msg::Ai(c, None)))
					.child(html::span("Clear").class("tsb sec").title("Unselect everything (Esc)").on_click(Msg::CheckClear)),
			);
		}
		head
	}

	/// The Aa button over the diff and the menu of sizes it opens.
	pub(super) fn size_menu(&self) -> html::El<Msg> {
		let mut d = html::div().class("dd vmd").child(
			html::span("Aa").class(if self.view_menu { "rvg on" } else { "rvg" }).title("Sizes: code, text, file tree. Keys: + - code, < > tree").on_click(Msg::ViewMenu),
		);
		if self.view_menu {
			let row = |label: &str, value: String, k: SizeKind| {
				html::div()
					.class("ddi")
					.child(html::span(label.to_owned()).class("vl"))
					.child(html::span("\u{2212}").class("stp").on_click(Msg::Size(k, -1)))
					.child(html::span(value).class("n"))
					.child(html::span("+").class("stp").on_click(Msg::Size(k, 1)))
			};
			d = d.child(
				html::div()
					.class("ddm")
					.child(row("Diff text", format!("{:.1}", self.ui.code), SizeKind::Code))
					.child(row("Tree text", format!("{:.1}", self.ui.text), SizeKind::Text))
					.child(row("Tree width", format!("{:.0}", self.ui.tree), SizeKind::Tree))
					.child(html::div().class("ddi").on_click(Msg::SizeReset).child(html::span("Reset sizes"))),
			);
		}
		d
	}

	/// The stylesheet for the sizes, laid over the base one.
	pub(super) fn ui_css(u: &config::Ui) -> String {
		format!(
			// Tern's code widget sets its own font size and line height, so they are set on it; the gutters beside it follow the same height.
			".azfv{{font-size:{t}px}}.aztw{{width:{w}px;min-width:200px;max-width:65%}}.aztr{{height:{h}px}}\
			 .azds .azcc .sf-code{{--sf-lh:{l}px;line-height:{l}px;font-size:{c}px}}.azds .sf-ln{{height:{l}px;line-height:{l}px}}.azgl{{height:{l}px;line-height:{l}px}}",
			t = u.text,
			w = u.tree,
			h = (u.text * 2.0).round(),
			c = u.code,
			l = (u.code * 1.5).round(),
		)
	}

	/// One step bigger or smaller, saved.
	pub(super) fn resize(&mut self, which: SizeKind, dir: i8) {
		let d = f32::from(dir);
		match which {
			SizeKind::Code => self.ui.code = (self.ui.code + 0.5 * d).clamp(8.0, 22.0),
			SizeKind::Text => self.ui.text = (self.ui.text + 0.5 * d).clamp(9.0, 20.0),
			SizeKind::Tree => self.ui.tree = (self.ui.tree + 20.0 * d).clamp(200.0, 720.0),
		}
		self.ui_changed();
	}

	pub(super) fn ui_changed(&mut self) {
		self.sheet_dirty = true;
		if let Err(e) = config::save_ui(self.ui) {
			self.note = format!("could not save the sizes: {e}");
		}
	}

	/// Open the AI modal for the selected files.
	pub(super) fn open_selection(&mut self, kind: review::Kind) {
		self.open_ai(Some(AiReq { kind, thread: None, line: None }), kind, None);
	}

	pub(super) fn open_files_filter(&mut self) {
		let Some(p) = &self.page else { return };
		if p.tab != PrTab::Files {
			return;
		}
		self.compose = Some(Composer::new(ComposeKind::Files, p.filter.text().to_owned()));
	}

	/// Empty the file filter, and the box too when it is being typed in.
	pub(super) fn clear_files_filter(&mut self) {
		if self.compose.as_ref().is_some_and(|c| c.kind == ComposeKind::Files) {
			self.compose = Some(Composer::new(ComposeKind::Files, String::new()));
		}
		self.set_files_filter("");
	}

	pub(super) fn set_files_filter(&mut self, text: &str) {
		if let Some(p) = &mut self.page {
			p.filter = filter::Filter::parse(text);
		}
	}

	/// Check or uncheck one file (its place in the change list).
	pub(super) fn check_file(&mut self, i: usize) {
		let Some(p) = &mut self.page else { return };
		let Some(path) = p.paths().get(i).map(|s| (*s).to_owned()) else { return };
		if !p.checked.remove(&path) {
			p.checked.insert(path);
		}
	}

	/// Check the shown files under a folder, or uncheck them when they all are.
	pub(super) fn check_dir(&mut self, dir: &str) {
		let Some(p) = &mut self.page else { return };
		let prefix = format!("{dir}/");
		let under: Vec<String> = {
			let paths = p.paths();
			p.matching().into_iter().filter(|i| paths[*i].starts_with(&prefix)).map(|i| paths[i].to_owned()).collect()
		};
		toggle(&mut p.checked, under);
	}

	/// Check every shown file, or uncheck them when they all are.
	pub(super) fn check_all(&mut self) {
		let Some(p) = &mut self.page else { return };
		let all: Vec<String> = {
			let paths = p.paths();
			p.matching().into_iter().map(|i| paths[i].to_owned()).collect()
		};
		toggle(&mut p.checked, all);
	}

	// ---- from outside ------------------------------------------------------------------------

	/// Act on queued commands, and write what the view shows when it changed. True when something was done.
	pub(super) fn steer_poll(&mut self) -> bool {
		let mut changed = false;
		// A move to another pull request finishes once its details are in.
		if let Some(c) = self.steer_wait.take() {
			match self.page.as_ref() {
				Some(p) if p.detail.is_some() && !p.reloading => {
					self.steer_go(&c);
					self.steer_done(&c);
					changed = true;
				},
				Some(p) if p.error.is_none() => self.steer_wait = Some(c),
				_ => {
					self.steer_done(&c);
					changed = true;
				},
			}
		}
		for c in remote::take() {
			if self.steer_apply(&c) {
				self.steer_done(&c);
			}
			changed = true;
		}
		let state = self.steer_state();
		let text = state.to_string();
		if text != self.steer_last {
			remote::write_state(&state);
			self.steer_last = text;
		}
		changed
	}

	fn steer_done(&mut self, c: &Value) {
		if let Some(id) = c["id"].as_u64() {
			self.steer_applied = self.steer_applied.max(id);
		}
	}

	/// Show the Files tab of the open pull request.
	fn show_files(&mut self) {
		let Some(p) = &mut self.page else { return };
		if p.tab != PrTab::Files {
			p.tab = PrTab::Files;
			p.thread = None;
			if p.detail.is_some() {
				self.load_file();
			}
		}
	}

	/// Make the file's row findable: a filter that hides it goes.
	fn unhide(&mut self, index: usize) {
		let Some(p) = &mut self.page else { return };
		if p.paths().get(index).is_some_and(|path| !p.filter.matches(path)) {
			p.filter = filter::Filter::default();
			self.compose = self.compose.take().filter(|c| c.kind != ComposeKind::Files);
		}
	}

	/// Scroll the diff to a line of the new side, showing the whole file when the line is not in a hunk.
	pub(super) fn goto_line(&mut self, n: u32) {
		let Some(p) = &mut self.page else { return };
		if !p.file_ready() {
			p.at_line = Some(n);
			return;
		}
		let find = |p: &Page| p.diff().and_then(|d| d.lines.iter().position(|l| l.new == Some(u64::from(n)) && l.tag != '-'));
		let mut at = find(p);
		if at.is_none() && !p.full {
			p.full = true;
			self.rebuild_diffs();
			at = self.page.as_ref().and_then(find);
		}
		let Some(p) = &mut self.page else { return };
		let Some(row) = at.and_then(|i| p.diff().map(|d| d.jump_to(i, JUMP_LEAD))) else {
			self.note = format!("line {n} is not in this file");
			return;
		};
		p.change = None;
		p.thread = None;
		self.scroll_req = Some(ScrollReq::Reveal(format!("{FILES_DS}.s{row}")));
	}

	/// Go to a comment thread: on the diff under its line, or in the overview.
	fn go_thread(&mut self, id: u32, overview: bool) {
		let Some(p) = &mut self.page else { return };
		let Some(d) = &p.detail else { return };
		let Some(t) = d.threads.iter().find(|t| t.id == id && t.shown()) else {
			self.note = format!("no comment thread {id} on this pull request");
			return;
		};
		let file = t.file().map(str::to_owned);
		match file.filter(|_| !overview) {
			Some(path) => {
				self.show_files();
				let Some(index) = self.page.as_ref().and_then(|p| p.paths().iter().position(|x| *x == path)) else { return };
				self.unhide(index);
				if self.page.as_ref().is_some_and(|p| p.file != index) {
					self.pick_file(index);
				}
				let Some(p) = &mut self.page else { return };
				p.thread = Some(id);
				if p.file_ready() {
					self.scroll_req = Some(ScrollReq::Reveal(format!("{FILES_DS}.t{id}")));
				} else {
					p.pending = Some(id);
				}
			},
			None => {
				p.tab = PrTab::Overview;
				p.thread = Some(id);
				self.scroll_req = Some(ScrollReq::Reveal(format!("{OV_MAIN}.t{id}")));
			},
		}
	}

	/// Carry out a move on the open pull request: tab, file filter, file, line, comment.
	fn steer_go(&mut self, c: &Value) {
		self.menu = None;
		self.nav = None;
		self.ai = None;
		self.rv_menu = false;
		if self.page.is_none() {
			return;
		}
		match c["tab"].as_str() {
			Some("overview") => {
				if let Some(p) = &mut self.page {
					p.tab = PrTab::Overview;
				}
			},
			Some("files") => self.show_files(),
			_ => {},
		}
		if let Some(text) = c["filter"].as_str() {
			self.compose = self.compose.take().filter(|c| c.kind != ComposeKind::Files);
			self.show_files();
			self.set_files_filter(text);
		}
		if let Some(id) = c["thread"].as_u64().and_then(|i| u32::try_from(i).ok()) {
			self.go_thread(id, c["view"].as_str() == Some("overview"));
			return;
		}
		if let Some(want) = c["file"].as_str() {
			self.show_files();
			let Some(p) = &self.page else { return };
			let paths = p.paths();
			let bare = want.trim_start_matches('/');
			// An exact path (or its end) first, else the first file the text matches in tree order.
			let exact = paths.iter().position(|x| Page::bare(x) == bare).or_else(|| paths.iter().position(|x| Page::bare(x).ends_with(bare)));
			let f = filter::Filter::parse(want);
			let loose = || tree::order(&paths).into_iter().find(|i| f.matches(paths[*i]));
			match exact.or_else(loose) {
				Some(index) => {
					self.unhide(index);
					self.pick_file(index);
				},
				None => self.note = format!("no changed file matches {want}"),
			}
		}
		if let Some(n) = c["line"].as_u64().and_then(|n| u32::try_from(n).ok()) {
			self.show_files();
			self.goto_line(n);
		}
	}

	/// Act on one command. False when it waits for a pull request to load.
	fn steer_apply(&mut self, c: &Value) -> bool {
		match c["cmd"].as_str().unwrap_or_default() {
			"dismiss" => {
				self.nav = None;
				self.ai = None;
				self.menu = None;
				self.rv_menu = false;
			},
			"search" => {
				self.search_nav(c["text"].as_str().unwrap_or_default());
			},
			"go" => {
				if let Some(id) = c["pr"].as_u64().and_then(|i| u32::try_from(i).ok())
					&& self.page.as_ref().is_none_or(|p| p.pr.pull_request_id != id)
				{
					if !self.prs.iter().any(|p| p.pull_request_id == id) {
						self.note = format!("pull request {id} is not in the list");
						return true;
					}
					self.open(id);
					self.steer_wait = Some(c.clone());
					return false;
				}
				if self.page.as_ref().is_some_and(|p| p.detail.is_none()) {
					self.steer_wait = Some(c.clone());
					return false;
				}
				self.steer_go(c);
			},
			"refresh" => {
				self.refresh();
				if let Some(p) = &mut self.page {
					p.reloading = true;
					// Whatever else it names (a file, a line, a comment) happens once the new data is in.
					self.steer_wait = Some(c.clone());
					return false;
				}
			},
			"filter" => {
				self.compose = self.compose.take().filter(|c| c.kind != ComposeKind::Files);
				self.show_files();
				self.set_files_filter(c["text"].as_str().unwrap_or_default());
			},
			"select" => {
				self.show_files();
				let Some(p) = &mut self.page else { return true };
				let all: Vec<String> = p.paths().into_iter().map(str::to_owned).collect();
				let wanted: Vec<String> = if let Some(list) = c["paths"].as_array() {
					let want: HashSet<&str> = list.iter().filter_map(Value::as_str).map(Page::bare).collect();
					all.iter().filter(|a| want.contains(Page::bare(a))).cloned().collect()
				} else if let Some(text) = c["text"].as_str() {
					let f = filter::Filter::parse(text);
					all.iter().filter(|a| f.matches(a)).cloned().collect()
				} else {
					// No text: what the file tree shows.
					let paths = p.paths();
					p.matching().into_iter().map(|i| paths[i].to_owned()).collect()
				};
				match c["mode"].as_str().unwrap_or("set") {
					"add" => p.checked.extend(wanted),
					"remove" => {
						for w in &wanted {
							p.checked.remove(w);
						}
					},
					"clear" => p.checked.clear(),
					_ => p.checked = wanted.into_iter().collect(),
				}
			},
			_ => {},
		}
		true
	}

	/// What the view shows, for `tern-azdo view` and the MCP tools: where the user is, the pull requests, and the open one's
	/// files and comments.
	fn steer_state(&self) -> Value {
		let mut v = json!({ "applied": self.steer_applied, "pr": Value::Null, "picker": { "open": self.nav.is_some(), "query": self.nav_query() } });
		v["prs"] = Value::Array(
			self.prs
				.iter()
				.take(400)
				.map(|q| json!({ "id": q.pull_request_id, "title": q.title, "project": q.repository.project.name, "repo": q.repository.name, "author": q.created_by.display_name, "source": q.source(), "target": q.target(), "draft": q.is_draft }))
				.collect(),
		);
		let Some(p) = &self.page else { return v };
		let pr = &p.pr;
		v["pr"] = json!({ "id": pr.pull_request_id, "title": pr.title, "project": pr.repository.project.name, "repo": pr.repository.name, "source": pr.source(), "target": pr.target() });
		v["tab"] = json!(if p.tab == PrTab::Files { "files" } else { "overview" });
		v["filter"] = json!(p.filter.text());
		v["filter_error"] = json!(p.filter.error());
		v["file"] = json!(p.change().map(|c| Page::bare(c.path())));
		v["thread"] = json!(p.thread);
		let shown: HashSet<usize> = p.matching().into_iter().collect();
		let files: Vec<Value> = p.detail.as_ref().map_or_else(Vec::new, |d| {
			d.changes
				.iter()
				.enumerate()
				.map(|(i, c)| json!({ "path": Page::bare(c.path()), "status": c.letter(), "shown": shown.contains(&i), "selected": p.checked.contains(c.path()) }))
				.collect()
		});
		v["files"] = Value::Array(files);
		let threads: Vec<Value> = p.detail.as_ref().map_or_else(Vec::new, |d| {
			d.threads
				.iter()
				.filter(|t| t.shown())
				.map(|t| {
					let first = t.human().next();
					json!({
						"id": t.id,
						"file": t.file().map(Page::bare),
						"line": t.line(),
						"status": api::status_label(t.status.as_deref()).0,
						"author": first.map_or("", |c| c.author.display_name.as_str()),
						"text": super::first_line(first.and_then(|c| c.content.as_deref()).unwrap_or_default(), 240),
						"replies": t.human().count().saturating_sub(1),
					})
				})
				.collect()
		});
		v["threads"] = Value::Array(threads);
		v
	}
}

/// Add `files` to `set`, or take them out when they are all in already.
fn toggle(set: &mut HashSet<String>, files: Vec<String>) {
	if files.is_empty() {
		return;
	}
	if files.iter().all(|f| set.contains(f)) {
		for f in &files {
			set.remove(f);
		}
	} else {
		set.extend(files);
	}
}
