//! The interactive client: the org's active pull requests, and a page per
//! pull request with its overview, changed files, diffs and comments.

use std::{
	collections::HashMap,
	sync::{
		Arc,
		mpsc::{self, Receiver, Sender},
	},
	time::{Duration, Instant},
};

use tern_sdk::{
	Input, Options, Session, SurfaceOptions,
	keys::Key,
	ui::{self, Side, Tone, View, html},
	wire::{self, Event, feature},
};

use crate::{
	api::{self, Client, Detail, Identity, Pr, Thread},
	config,
	diff::{self, FileDiff},
	worker::{self, Job, Reply, Sides},
};

/// CSS pixels per unit of the cell height Tern reports (a text line is about 15.5px at the default size).
const CSS_PER_CELL_H: f64 = 0.78;
/// The height of one list row, in CSS pixels (matches `.row` below).
const ROW_PX: f64 = 22.0;
/// Context lines around each change; `x` shows the whole file.
const CONTEXT: usize = 3;
const COMPOSER: &str = "dock.ed";

const CSS: &str = "\
.box{height:100%;box-sizing:border-box;width:100%;overflow:hidden;padding:4px;border:1px solid color-mix(in srgb,var(--sf-c-muted) 45%,transparent);border-radius:6px;font-family:Geist,system-ui,sans-serif;font-size:12.9px}\
.scroll{overflow:auto;padding:6px 10px}\
.row{display:flex;align-items:center;gap:10px;height:22px;box-sizing:border-box;padding:0 8px;white-space:nowrap;border-radius:4px;cursor:pointer}\
.row:hover{background:color-mix(in srgb,var(--sf-c-muted) 14%,transparent)}\
.row.cur{background:color-mix(in srgb,var(--sf-c-accent) 24%,transparent)}\
.grp{color:var(--sf-c-muted);font-weight:600;cursor:default}.grp:hover{background:none}\
.pr{padding-left:24px}\
.id{color:var(--sf-c-muted);min-width:52px;font-variant-numeric:tabular-nums}\
.t{flex:1;overflow:hidden;text-overflow:ellipsis}\
.m{color:var(--sf-c-muted);overflow:hidden;text-overflow:ellipsis;max-width:48%}\
.ok{color:var(--sf-good,#3fb950)}.bad{color:var(--sf-bad,#f85149)}.wait{color:var(--sf-warn,#d29922)}.dim{color:var(--sf-c-muted)}\
.chip{border:1px solid color-mix(in srgb,var(--sf-c-muted) 60%,transparent);border-radius:8px;padding:0 6px;font-size:11px;color:var(--sf-c-muted)}\
.files{width:34%;min-width:220px;max-width:460px;flex-shrink:0}\
.fl{gap:6px}.fk{width:14px;text-align:center;font-weight:700}.fk.A{color:var(--sf-good,#3fb950)}.fk.D{color:var(--sf-bad,#f85149)}.fk.R{color:var(--sf-warn,#d29922)}.fk.M{color:var(--sf-c-accent)}\
.fd{color:var(--sf-c-muted);overflow:hidden;text-overflow:ellipsis;flex:1}\
.fn{overflow:hidden;text-overflow:ellipsis}\
.cc{color:var(--sf-c-accent);font-size:11px}\
.th{padding:3px 8px;border-radius:4px;white-space:normal;line-height:1.35}\
.th.cur{background:color-mix(in srgb,var(--sf-c-accent) 18%,transparent)}\
.who{font-weight:600}\
.head{font-family:Geist,system-ui,sans-serif;font-size:12.9px;padding:2px 4px;display:flex;gap:10px;align-items:center;white-space:nowrap;overflow:hidden}\
.title{font-weight:700;font-size:14px;overflow:hidden;text-overflow:ellipsis}\
.prompt{display:flex;gap:14px;align-items:center;height:28px;padding:0 10px;border:1px solid var(--sf-c-accent);border-radius:6px;font-family:Geist,system-ui,sans-serif;font-size:12.9px}\
.prompt .pl{color:var(--sf-c-accent);font-weight:600}";

#[derive(Clone, Debug)]
enum Msg {
	/// A list row was clicked.
	Pick(u32),
	/// A list row was double-clicked.
	Open(u32),
	ListTab(String),
	PrTab(String),
	File(usize),
	Thread(u32),
	Send(String),
	/// Native editing changed the composer's text.
	Edit(wire::Edit),
	Focus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListTab {
	Review,
	Mine,
	All,
}

impl ListTab {
	const ALL: [Self; 3] = [Self::Review, Self::Mine, Self::All];

	fn id(self) -> &'static str {
		match self {
			Self::Review => "review",
			Self::Mine => "mine",
			Self::All => "all",
		}
	}

	fn label(self) -> &'static str {
		match self {
			Self::Review => "Reviewing",
			Self::Mine => "Created by me",
			Self::All => "All active",
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrTab {
	Overview,
	Files,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComposeKind {
	/// The organization URL, on first run.
	Org,
	Filter,
	/// A new thread when none, else a reply in that thread.
	Comment(Option<u32>),
}

struct Composer {
	kind:   ComposeKind,
	draft:  String,
	/// In chars.
	cursor: usize,
}

impl Composer {
	fn new(kind: ComposeKind, draft: String) -> Self {
		let cursor = draft.chars().count();
		Self { kind, draft, cursor }
	}

	fn byte(&self, chars: usize) -> usize {
		self.draft.char_indices().nth(chars).map_or(self.draft.len(), |(i, _)| i)
	}

	fn insert(&mut self, text: &str) {
		let at = self.byte(self.cursor);
		self.draft.insert_str(at, text);
		self.cursor += text.chars().count();
	}

	/// Plain editing keys. False when the key is not one of them.
	fn key(&mut self, key: &Key) -> bool {
		match key.name.as_str() {
			"backspace" if self.cursor > 0 => {
				let at = self.byte(self.cursor - 1);
				self.draft.remove(at);
				self.cursor -= 1;
			},
			"delete" if self.cursor < self.draft.chars().count() => {
				let at = self.byte(self.cursor);
				self.draft.remove(at);
			},
			"backspace" | "delete" => {},
			"left" => self.cursor = self.cursor.saturating_sub(1),
			"right" => self.cursor = (self.cursor + 1).min(self.draft.chars().count()),
			"home" => self.cursor = 0,
			"end" => self.cursor = self.draft.chars().count(),
			"paste" => self.insert(&key.text.clone().unwrap_or_default().replace("\r\n", "\n")),
			_ => match key.typed() {
				Some(t) if !key.ctrl => {
					let t = t.to_owned();
					self.insert(&t);
				},
				_ => return false,
			},
		}
		true
	}

	/// The char index at UTF-16 offset `u`.
	fn char_at(&self, u: usize) -> usize {
		let mut units = 0;
		for (i, c) in self.draft.chars().enumerate() {
			if units >= u {
				return i;
			}
			units += c.len_utf16();
		}
		self.draft.chars().count()
	}

	/// Applies a native edit. False when Tern's copy of the text has a different length.
	fn apply(&mut self, e: &wire::Edit) -> bool {
		if e.len != self.draft.encode_utf16().count() {
			return false;
		}
		let (a, b) = (self.byte(self.char_at(e.from.min(e.to))), self.byte(self.char_at(e.from.max(e.to))));
		self.draft.replace_range(a..b, &e.text);
		self.cursor = self.char_at(e.cursor);
		true
	}

	fn utf16_cursor(&self) -> usize {
		self.draft.chars().take(self.cursor).map(char::len_utf16).sum()
	}
}

/// One file's loaded sides and its built diff.
enum FileState {
	Loading,
	Failed(String),
	Ready { sides: Sides, diff: FileDiff },
}

struct Page {
	pr:      Pr,
	tab:     PrTab,
	detail:  Option<Detail>,
	error:   Option<String>,
	file:    usize,
	files:   HashMap<String, FileState>,
	/// First diff line shown.
	offset:  usize,
	/// Whole files instead of hunks.
	full:    bool,
	/// The selected comment thread.
	thread:  Option<u32>,
	/// Overview scroll, in blocks.
	ov_skip: usize,
	/// A thread line to scroll to once the selected file's diff arrives.
	pending: Option<u32>,
}

impl Page {
	fn new(pr: Pr) -> Self {
		Self { pr, tab: PrTab::Overview, detail: None, error: None, file: 0, files: HashMap::new(), offset: 0, full: false, thread: None, ov_skip: 0, pending: None }
	}

	fn change(&self) -> Option<&api::Change> {
		self.detail.as_ref()?.changes.get(self.file)
	}

	/// Threads on the selected file, by line.
	fn file_threads(&self) -> Vec<&Thread> {
		let (Some(d), Some(c)) = (self.detail.as_ref(), self.change()) else { return Vec::new() };
		let mut t: Vec<&Thread> = d.threads.iter().filter(|t| t.shown() && t.file() == Some(c.path())).collect();
		t.sort_by_key(|t| t.line().unwrap_or(0));
		t
	}

	/// Threads not on a file.
	fn general_threads(&self) -> Vec<&Thread> {
		self.detail.as_ref().map(|d| d.threads.iter().filter(|t| t.shown() && t.file().is_none()).collect()).unwrap_or_default()
	}

	fn threads_here(&self) -> Vec<&Thread> {
		if self.tab == PrTab::Files { self.file_threads() } else { self.general_threads() }
	}

	fn diff(&self) -> Option<&FileDiff> {
		match self.files.get(self.change()?.path())? {
			FileState::Ready { diff, .. } => Some(diff),
			_ => None,
		}
	}
}

struct App {
	client:  Option<Arc<Client>>,
	me:      Option<Identity>,
	prs:     Vec<Pr>,
	loading: bool,
	error:   Option<String>,
	tab:     ListTab,
	filter:  String,
	sel:     Option<u32>,
	page:    Option<Page>,
	compose: Option<Composer>,
	voting:  bool,
	note:    String,
	rows:    f64,
	cell_h:  f64,
	cols:    f64,
	tx:      Sender<Reply>,
	rx:      Receiver<Reply>,
	busy:    usize,
}

/// `3d`, `5h`, `12m` since an RFC 3339 time.
fn age(when: &str) -> String {
	let Ok(t) = chrono::DateTime::parse_from_rfc3339(when) else { return String::new() };
	let secs = (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds().max(0);
	match secs {
		s if s < 3600 => format!("{}m", s / 60),
		s if s < 86_400 => format!("{}h", s / 3600),
		s if s < 86_400 * 60 => format!("{}d", s / 86_400),
		s => format!("{}mo", s / (86_400 * 30)),
	}
}

fn vote_glyph(vote: i32) -> (&'static str, &'static str) {
	match vote {
		10 => ("✓", "ok"),
		5 => ("✓", "ok"),
		-5 => ("⏸", "wait"),
		-10 => ("✗", "bad"),
		_ => ("·", "dim"),
	}
}

/// The first line of a comment, cut to `n` chars.
fn first_line(s: &str, n: usize) -> String {
	let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim();
	if line.chars().count() > n { format!("{}…", line.chars().take(n).collect::<String>()) } else { line.to_owned() }
}

fn open_url(url: &str) -> anyhow::Result<()> {
	#[cfg(windows)]
	let mut cmd = {
		let mut c = std::process::Command::new("rundll32");
		c.args(["url.dll,FileProtocolHandler", url]);
		c
	};
	#[cfg(target_os = "macos")]
	let mut cmd = {
		let mut c = std::process::Command::new("open");
		c.arg(url);
		c
	};
	#[cfg(all(unix, not(target_os = "macos")))]
	let mut cmd = {
		let mut c = std::process::Command::new("xdg-open");
		c.arg(url);
		c
	};
	cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn()?;
	Ok(())
}

/// The pane's height in lines, from `tern inspect` (the surface is not told when it changes).
fn pane_rows() -> Option<f64> {
	let id: u64 = std::env::var("TERN_PANE").ok()?.parse().ok()?;
	let out = std::process::Command::new("tern").args(["inspect", "--json"]).output().ok()?;
	let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
	let mut stack = vec![&v];
	while let Some(n) = stack.pop() {
		match n {
			serde_json::Value::Object(m) => {
				if m.get("id").and_then(serde_json::Value::as_u64) == Some(id)
					&& let Some(r) = m.get("rows").and_then(serde_json::Value::as_f64)
				{
					return Some(r);
				}
				stack.extend(m.values());
			},
			serde_json::Value::Array(a) => stack.extend(a),
			_ => {},
		}
	}
	None
}

impl App {
	fn new(org: Option<String>, demo: bool) -> Self {
		let (tx, rx) = mpsc::channel();
		let mut app = Self {
			client: None,
			me: None,
			prs: Vec::new(),
			loading: false,
			error: None,
			tab: ListTab::Review,
			filter: String::new(),
			sel: None,
			page: None,
			compose: None,
			voting: false,
			note: String::new(),
			rows: pane_rows().unwrap_or(40.0),
			cell_h: 20.0,
			cols: 160.0,
			tx,
			rx,
			busy: 0,
		};
		if demo {
			app.client = Some(Arc::new(Client::demo()));
			app.refresh();
		} else {
			match org {
				Some(o) => app.connect(&o),
				None => app.compose = Some(Composer::new(ComposeKind::Org, String::new())),
			}
		}
		app
	}

	fn connect(&mut self, org: &str) {
		self.client = Some(Arc::new(Client::new(org)));
		self.refresh();
	}

	fn start(&mut self, job: Job) {
		if let Some(c) = &self.client {
			self.busy += 1;
			worker::start(c.clone(), job, self.tx.clone());
		}
	}

	fn refresh(&mut self) {
		self.loading = true;
		self.start(Job::Load);
		if let Some(p) = &self.page {
			let pr = p.pr.clone();
			self.start(Job::Detail(pr));
		}
	}

	fn me_id(&self) -> &str {
		self.me.as_ref().map_or("", |m| m.id.as_str())
	}

	fn in_tab(&self, pr: &Pr, tab: ListTab) -> bool {
		let me = self.me_id();
		match tab {
			ListTab::Review => pr.reviewers.iter().any(|r| r.id == me) && pr.created_by.id != me,
			ListTab::Mine => pr.created_by.id == me,
			ListTab::All => true,
		}
	}

	/// The current tab's PRs after the filter, by project, repo, then newest first.
	fn visible(&self) -> Vec<&Pr> {
		let needle = self.filter.to_lowercase();
		let mut v: Vec<&Pr> = self
			.prs
			.iter()
			.filter(|p| self.in_tab(p, self.tab))
			.filter(|p| {
				needle.is_empty()
					|| [p.title.as_str(), p.created_by.display_name.as_str(), p.repository.name.as_str(), p.repository.project.name.as_str(), p.source()]
						.iter()
						.any(|s| s.to_lowercase().contains(&needle))
					|| p.pull_request_id.to_string() == needle.trim_start_matches('!')
			})
			.collect();
		v.sort_by(|a, b| {
			(a.repository.project.name.to_lowercase(), a.repository.name.to_lowercase())
				.cmp(&(b.repository.project.name.to_lowercase(), b.repository.name.to_lowercase()))
				.then(b.pull_request_id.cmp(&a.pull_request_id))
		});
		v
	}

	fn selected_pr(&self) -> Option<&Pr> {
		if let Some(p) = &self.page {
			return Some(&p.pr);
		}
		let id = self.sel?;
		self.prs.iter().find(|p| p.pull_request_id == id)
	}

	fn fix_selection(&mut self) {
		let ids: Vec<u32> = self.visible().iter().map(|p| p.pull_request_id).collect();
		if !self.sel.is_some_and(|s| ids.contains(&s)) {
			self.sel = ids.first().copied();
		}
	}

	fn move_sel(&mut self, delta: i64) {
		let ids: Vec<u32> = self.visible().iter().map(|p| p.pull_request_id).collect();
		if ids.is_empty() {
			return;
		}
		let at = self.sel.and_then(|s| ids.iter().position(|i| *i == s)).unwrap_or(0) as i64;
		let next = (at + delta).clamp(0, ids.len() as i64 - 1) as usize;
		self.sel = Some(ids[next]);
	}

	fn open(&mut self, id: u32) {
		let Some(pr) = self.prs.iter().find(|p| p.pull_request_id == id).cloned() else { return };
		self.sel = Some(id);
		self.page = Some(Page::new(pr.clone()));
		self.start(Job::Detail(pr));
	}

	/// Loads the selected file's sides if they are not loaded yet.
	fn load_file(&mut self) {
		let Some(p) = &self.page else { return };
		let Some(c) = p.change() else { return };
		let path = c.path().to_owned();
		if p.files.contains_key(&path) {
			return;
		}
		let (base, head) = c.blobs();
		let job = Job::File { pr: p.pr.clone(), path: path.clone(), base: base.map(str::to_owned), head: head.map(str::to_owned) };
		if let Some(p) = &mut self.page {
			p.files.insert(path, FileState::Loading);
		}
		self.start(job);
	}

	fn rebuild_diffs(&mut self) {
		if let Some(p) = &mut self.page {
			let context = if p.full { usize::MAX / 4 } else { CONTEXT };
			for state in p.files.values_mut() {
				if let FileState::Ready { sides: (Some(a), Some(b)), diff } = state {
					*diff = diff::build(a, b, context);
				}
			}
			p.offset = 0;
		}
	}

	/// Handles finished jobs. True when something changed.
	fn poll(&mut self) -> bool {
		let mut changed = false;
		while let Ok(reply) = self.rx.try_recv() {
			changed = true;
			self.busy = self.busy.saturating_sub(1);
			match reply {
				Reply::Loaded(Ok((me, prs))) => {
					let first = self.me.is_none();
					self.me = Some(me);
					self.prs = prs;
					self.loading = false;
					self.error = None;
					if first && !self.prs.iter().any(|p| self.in_tab(p, ListTab::Review)) {
						self.tab = if self.prs.iter().any(|p| self.in_tab(p, ListTab::Mine)) { ListTab::Mine } else { ListTab::All };
					}
					self.fix_selection();
				},
				Reply::Loaded(Err(e)) => {
					self.loading = false;
					self.error = Some(e);
				},
				Reply::Detail(id, r) => {
					let Some(p) = self.page.as_mut().filter(|p| p.pr.pull_request_id == id) else { continue };
					match r {
						Ok(b) => {
							let (pr, detail) = *b;
							if let Some(slot) = self.prs.iter_mut().find(|x| x.pull_request_id == id) {
								*slot = pr.clone();
							}
							p.pr = pr;
							p.file = p.file.min(detail.changes.len().saturating_sub(1));
							p.detail = Some(detail);
							p.error = None;
							// Comments may have changed; file sides have not (same iteration or not, they reload on demand).
							p.files.clear();
						},
						Err(e) => p.error = Some(e),
					}
					if self.page.as_ref().is_some_and(|p| p.tab == PrTab::Files) {
						self.load_file();
					}
				},
				Reply::File(id, path, r) => {
					let Some(p) = self.page.as_mut().filter(|p| p.pr.pull_request_id == id) else { continue };
					let context = if p.full { usize::MAX / 4 } else { CONTEXT };
					let state = match r {
						Ok((Some(a), Some(b))) => {
							let diff = diff::build(&a, &b, context);
							FileState::Ready { sides: (Some(a), Some(b)), diff }
						},
						Ok(sides) => FileState::Ready { sides, diff: FileDiff::default() },
						Err(e) => FileState::Failed(e),
					};
					let current = p.change().is_some_and(|c| c.path() == path);
					p.files.insert(path, state);
					if current && let Some(line) = p.pending.take() {
						let h = self.diff_height();
						if let Some(p) = &mut self.page
							&& let Some(i) = p.diff().and_then(|d| d.find(u64::from(line)))
						{
							p.offset = i.saturating_sub(h / 3);
						}
					}
				},
				Reply::Wrote(id, what, r) => match r {
					Ok(()) => {
						self.note = format!("{what} sent");
						if let Some(p) = self.page.as_ref().filter(|p| p.pr.pull_request_id == id) {
							let pr = p.pr.clone();
							self.start(Job::Detail(pr));
						}
					},
					Err(e) => self.note = format!("{what} failed: {e}"),
				},
			}
		}
		changed
	}

	// ---- layout numbers -----------------------------------------------------------------------

	/// Lines for the main region (the dock takes the rest).
	fn avail(&self) -> f64 {
		let dock = 2.0 + if self.compose.is_some() || self.voting { 2.2 } else { 0.0 };
		(self.rows - dock - 0.5).max(6.0)
	}

	/// How many 22px rows fit in `lines` lines.
	fn rows_in(&self, lines: f64) -> usize {
		((lines * self.cell_h * CSS_PER_CELL_H - 10.0) / ROW_PX).floor().max(1.0) as usize
	}

	fn list_lines(&self) -> f64 {
		self.avail() - 2.8
	}

	fn page_body_lines(&self) -> f64 {
		self.avail() - 2.0 - 2.8
	}

	/// Lines of diff that fit: the body less the thread strip under it.
	fn diff_height(&self) -> usize {
		let Some(p) = &self.page else { return 10 };
		let threads = if p.file_threads().is_empty() { 0.0 } else { self.thread_strip_lines() };
		// A diff line is a text line; hunk headers and the frame take a few.
		(self.page_body_lines() - threads - 3.0).max(3.0) as usize
	}

	/// Chars a diff line holds before it wraps: the pane less the file list and the gutter.
	fn diff_width(&self) -> usize {
		(self.cols * 0.64 - 10.0).max(20.0) as usize
	}

	/// Diff lines that fit from the current offset.
	fn diff_fit(&self) -> usize {
		let Some(p) = &self.page else { return 1 };
		p.diff().map_or(1, |d| d.fit(p.offset, self.diff_height(), self.diff_width()))
	}

	fn thread_strip_lines(&self) -> f64 {
		(self.page_body_lines() * 0.3).clamp(4.0, 12.0)
	}

	// ---- views ----------------------------------------------------------------------------------

	fn view(&self) -> View<Msg> {
		let main = if self.client.is_none() {
			ui::col().child(ui::md(
				"**Azure DevOps pull requests**\n\nEnter your organization below: its name, a `dev.azure.com` URL, or any repo URL in it. \
				 Sign-in uses the Azure CLI (`az login`), or a PAT saved with `tern-azdo pat`.",
			))
		} else if let Some(p) = &self.page {
			self.page_view(p)
		} else {
			self.list_view()
		};
		let mut dock: Vec<tern_sdk::Node<Msg>> = Vec::new();
		if self.voting {
			let mut row = html::div().class("prompt").child(html::span("Vote").class("pl"));
			for (k, label) in [("1", "Approve"), ("2", "Approve with suggestions"), ("3", "Wait for author"), ("4", "Reject"), ("0", "Reset")] {
				row = row.child(html::span(format!("{k} {label}")));
			}
			dock.push(row.child(html::span("Esc cancels").class("dim")).into());
		}
		if let Some(c) = &self.compose {
			let placeholder = match c.kind {
				ComposeKind::Org => "Organization, e.g. Contoso or https://dev.azure.com/Contoso (Enter saves)",
				ComposeKind::Filter => "Filter by title, author, repo, branch or id (Enter keeps, Esc clears)",
				ComposeKind::Comment(None) => "New comment (Enter sends, Shift+Enter new line, Esc cancels)",
				ComposeKind::Comment(Some(_)) => "Reply (Enter sends, Shift+Enter new line, Esc cancels)",
			};
			dock.push(
				ui::editor()
					.key("ed")
					.text(c.draft.clone())
					.cursor(c.utf16_cursor())
					.placeholder(placeholder)
					.sendable(true)
					.max_lines(6u32)
					.on_send(|t| Msg::Send(t.to_owned()))
					.on_edit(|e| Msg::Edit(e.clone()))
					.on_focus(Msg::Focus)
					.into(),
			);
		}
		dock.push(self.status().into());
		View::new().main(main).dock(dock)
	}

	fn status(&self) -> ui::StatusBar<Msg> {
		let org = self.client.as_ref().map_or("not set", |c| config::short(&c.org)).to_owned();
		let mut s = ui::status().key("bar").child(ui::seg(org).icon("git-pull-request"));
		if let Some(me) = &self.me {
			s = s.child(ui::seg(me.display_name.clone()).tone(Tone::Muted));
		}
		if self.busy > 0 {
			s = s.child(ui::seg("loading…").tone(Tone::Muted));
		}
		if let Some(e) = &self.error {
			s = s.child(ui::seg(first_line(e, 140)).tone(Tone::Error));
		} else if !self.note.is_empty() {
			s = s.child(ui::seg(self.note.clone()).tone(Tone::Muted));
		}
		let keys = if self.page.is_some() {
			"Esc back · Tab files · v vote · c comment · a reply · o browser · r refresh"
		} else {
			"Enter open · 1-3 tabs · f filter · o browser · y copy link · r refresh · q quit"
		};
		s.child(ui::seg(keys).side(Side::Right).tone(Tone::Muted))
	}

	fn list_view(&self) -> ui::Col<Msg> {
		let mut tabs = ui::tabs().key("tabs").active(self.tab.id()).on_select(|id| Msg::ListTab(id.to_owned()));
		for t in ListTab::ALL {
			let n = self.prs.iter().filter(|p| self.in_tab(p, t)).count();
			tabs = tabs.tab(t.id(), format!("{} ({n})", t.label()));
		}
		let visible = self.visible();
		// Group headers and PR rows, then the window that keeps the selection on screen.
		let mut rows: Vec<(Option<&Pr>, String)> = Vec::new();
		let mut last = String::new();
		for p in &visible {
			let group = format!("{} / {}", p.repository.project.name, p.repository.name);
			if group != last {
				rows.push((None, group.clone()));
				last = group;
			}
			rows.push((Some(p), String::new()));
		}
		let cap = self.rows_in(self.list_lines());
		let at = rows.iter().position(|(p, _)| p.is_some_and(|p| Some(p.pull_request_id) == self.sel)).unwrap_or(0);
		let first = at.saturating_sub(cap.saturating_sub(2)).min(rows.len().saturating_sub(cap));
		let me = self.me_id();
		let body = rows.iter().skip(first).take(cap).map(|(p, group)| -> tern_sdk::Node<Msg> {
			let Some(p) = p else { return html::div().class("row grp").child(html::span(group.clone())).into() };
			let id = p.pull_request_id;
			let counts = api::vote_counts(p);
			let mut row = html::div()
				.class(if Some(id) == self.sel { "row pr cur" } else { "row pr" })
				.on_click(Msg::Pick(id))
				.on_dblclick(Msg::Open(id))
				.child(html::span(format!("!{id}")).class("id"));
			if p.is_draft {
				row = row.child(html::span("draft").class("chip"));
			}
			row = row.child(html::span(p.title.clone()).class("t"));
			for (vote, class, glyph) in [(10, "ok", "✓"), (5, "ok", "✓~"), (-5, "wait", "⏸"), (-10, "bad", "✗")] {
				if let Some(n) = counts.get(&vote) {
					row = row.child(html::span(format!("{glyph}{n}")).class(class));
				}
			}
			if let Some(v) = p.vote_of(me).filter(|v| *v != 0 && p.created_by.id != me) {
				let (g, class) = vote_glyph(v);
				row = row.child(html::span(format!("you {g}")).class(class));
			}
			row.child(html::span(format!("{} · {} · {} → {}", p.created_by.display_name, age(&p.creation_date), p.source(), p.target())).class("m"))
				.into()
		});
		let empty = if self.loading && self.prs.is_empty() {
			Some("Loading pull requests…".to_owned())
		} else if visible.is_empty() {
			Some(if self.filter.is_empty() { "Nothing here.".to_owned() } else { format!("Nothing matches \u{201c}{}\u{201d}.", self.filter) })
		} else {
			None
		};
		let mut list = html::div().class("box").children(body);
		if let Some(e) = empty {
			list = list.child(html::div().class("row grp").child(html::span(e)));
		}
		let lines = self.list_lines();
		ui::col()
			.child(tabs)
			.child(ui::col().key("list").min(ui::Bound::h(ui::Extent::Lines(lines))).max(ui::Bound::h(ui::Extent::Lines(lines))).child(list))
	}

	fn page_view(&self, p: &Page) -> ui::Col<Msg> {
		let pr = &p.pr;
		let mut head = html::div().class("head").child(html::span(format!("!{}", pr.pull_request_id)).class("id")).child(html::span(pr.title.clone()).class("title"));
		if pr.is_draft {
			head = head.child(html::span("draft").class("chip"));
		}
		if let Some(m) = pr.merge_status.as_deref().filter(|m| *m == "conflicts" || *m == "failure") {
			head = head.child(html::span(if m == "conflicts" { "conflicts" } else { "merge failed" }).class("chip bad"));
		}
		let sub = html::div().class("head").child(html::span(format!(
			"{} wants to merge {} into {} · {} / {} · {}",
			pr.created_by.display_name,
			pr.source(),
			pr.target(),
			pr.repository.project.name,
			pr.repository.name,
			age(&pr.creation_date)
		)).class("dim"));
		let files = p.detail.as_ref().map_or(0, |d| d.changes.len());
		let tabs = ui::tabs()
			.key("ptabs")
			.tab("overview", "Overview")
			.tab("files", format!("Files ({files})"))
			.active(if p.tab == PrTab::Files { "files" } else { "overview" })
			.on_select(|id| Msg::PrTab(id.to_owned()));
		let lines = self.page_body_lines();
		let body: tern_sdk::Node<Msg> = if let Some(e) = &p.error {
			html::div().class("box scroll").child(ui::md(format!("Could not load this pull request.\n\n{e}"))).into()
		} else if p.detail.is_none() {
			html::div().class("box scroll").child(html::span("Loading…").class("dim")).into()
		} else if p.tab == PrTab::Files {
			self.files_view(p).into()
		} else {
			self.overview(p).into()
		};
		ui::col()
			.child(head)
			.child(sub)
			.child(tabs)
			.child(ui::col().key("pbody").min(ui::Bound::h(ui::Extent::Lines(lines))).max(ui::Bound::h(ui::Extent::Lines(lines))).child(body))
	}

	fn thread_node(&self, t: &Thread, selected: bool, with_line: bool) -> html::El<Msg> {
		let mut node = html::div().class(if selected { "th cur" } else { "th" }).on_click(Msg::Thread(t.id));
		let mut head = html::div();
		if with_line && let Some(l) = t.line() {
			head = head.child(html::span(format!("L{l}  ")).class("cc"));
		}
		head = head.child(html::span(t.status.clone().unwrap_or_else(|| "active".into())).class(if t.is_open() { "chip" } else { "chip dim" }));
		node = node.child(head);
		let comments: Vec<_> = t.human().collect();
		let shown: Vec<_> = if selected { comments.clone() } else { comments.iter().take(1).copied().collect() };
		for c in shown {
			let text = c.content.clone().unwrap_or_default();
			node = node.child(
				html::div()
					.child(html::span(format!("{}: ", c.author.display_name)).class("who"))
					.child(html::span(if selected { text } else { first_line(&text, 160) })),
			);
		}
		if !selected && comments.len() > 1 {
			node = node.child(html::span(format!("{} more", comments.len() - 1)).class("dim"));
		}
		node
	}

	fn overview(&self, p: &Page) -> html::El<Msg> {
		let pr = &p.pr;
		let d = p.detail.as_ref();
		let mut blocks: Vec<tern_sdk::Node<Msg>> = Vec::new();
		let mut reviewers = ui::kv();
		let mut rs = pr.reviewers.clone();
		rs.sort_by_key(|r| (!r.is_required, r.is_container, r.display_name.to_lowercase()));
		for r in &rs {
			let (g, _) = vote_glyph(r.vote);
			let who = format!("{}{}", r.display_name, if r.is_required { " (required)" } else { "" });
			reviewers = reviewers.item(who, format!("{g} {}", api::vote_label(r.vote)));
		}
		blocks.push(ui::section().head(format!("Reviewers ({})", pr.reviewers.len())).child(reviewers).into());
		if let Some(d) = d.filter(|d| !d.work_items.is_empty()) {
			let mut wi = ui::kv();
			for w in &d.work_items {
				wi = wi.item(format!("{} {}", w.kind, w.id), format!("{} ({})", w.title, w.state));
			}
			blocks.push(ui::section().head(format!("Work items ({})", d.work_items.len())).child(wi).into());
		}
		let desc = pr.description.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "_No description._".into());
		blocks.push(ui::section().head("Description").child(ui::md(desc)).into());
		let threads = p.general_threads();
		if !threads.is_empty() {
			let mut sec = ui::section().head(format!("Comments ({})", threads.len()));
			for t in threads {
				sec = sec.child(self.thread_node(t, p.thread == Some(t.id), false));
			}
			blocks.push(sec.into());
		}
		let file_threads = d.map_or(0, |d| d.threads.iter().filter(|t| t.shown() && t.file().is_some()).count());
		if file_threads > 0 {
			blocks.push(html::div().child(html::span(format!("{file_threads} comment threads on files: see the Files tab.")).class("dim")).into());
		}
		let skip = p.ov_skip.min(blocks.len().saturating_sub(1));
		html::div().class("box scroll").children(blocks.into_iter().skip(skip))
	}

	fn files_view(&self, p: &Page) -> ui::Row<Msg> {
		let Some(d) = &p.detail else { return ui::row() };
		let counts: HashMap<&str, usize> = d.threads.iter().filter(|t| t.shown()).filter_map(Thread::file).fold(HashMap::new(), |mut m, f| {
			*m.entry(f).or_default() += 1;
			m
		});
		let cap = self.rows_in(self.page_body_lines());
		let first = p.file.saturating_sub(cap.saturating_sub(2)).min(d.changes.len().saturating_sub(cap));
		let rows = d.changes.iter().enumerate().skip(first).take(cap).map(|(i, c)| {
			let path = c.path();
			let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
			let mut row = html::div()
				.class(if i == p.file { "row fl cur" } else { "row fl" })
				.on_click(Msg::File(i))
				.child(html::span(c.letter()).class(format!("fk {}", c.letter())))
				.child(html::span(name.to_owned()).class("fn"))
				.child(html::span(dir.trim_start_matches('/').to_owned()).class("fd"));
			if let Some(n) = counts.get(path) {
				row = row.child(html::span(format!("💬{n}")).class("cc"));
			}
			row
		});
		let list = html::div().class("box").children(rows);
		let lines = self.page_body_lines();
		let mut right = ui::col().key("right").grow(1.0).shrink(1.0);
		let Some(c) = p.change() else {
			return ui::row().child(html::div().class("box").child(html::span("No files changed.").class("dim")));
		};
		let diff_node: tern_sdk::Node<Msg> = match p.files.get(c.path()) {
			None | Some(FileState::Loading) => html::div().class("box scroll").child(html::span("Loading…").class("dim")).into(),
			Some(FileState::Failed(e)) => html::div().class("box scroll").child(html::span(e.clone()).class("bad")).into(),
			Some(FileState::Ready { sides, diff }) => {
				if sides.0.is_none() || sides.1.is_none() {
					html::div().class("box scroll").child(html::span("Binary or very large file: open it in the browser (o).").class("dim")).into()
				} else if diff.is_empty() {
					html::div().class("box scroll").child(html::span(if c.is("rename") { "Renamed, no content changes." } else { "No content changes." }).class("dim")).into()
				} else {
					let shown = p.offset.min(diff.len().saturating_sub(1));
					let h = diff.fit(shown, self.diff_height(), self.diff_width());
					let end = (shown + h).min(diff.len());
					let header = html::div().class("head").child(html::span(c.path().trim_start_matches('/').to_owned()).class("who")).child(html::span(format!(
						"+{} −{} · lines {}-{} of {}{}",
						diff.added(),
						diff.removed(),
						shown + 1,
						end,
						diff.len(),
						if p.full { " · whole file (x)" } else { "" }
					)).class("dim"));
					ui::col().child(header).child(ui::diff().key("d").hunks(diff.window(shown, h)).path(c.path().to_owned()).mode(ui::DiffMode::Unified)).into()
				}
			},
		};
		right = right.child(diff_node);
		let threads = p.file_threads();
		if !threads.is_empty() {
			let tl = self.thread_strip_lines();
			let strip = html::div().class("box scroll").children(threads.iter().map(|t| self.thread_node(t, p.thread == Some(t.id), true)));
			right = right.child(ui::col().key("threads").min(ui::Bound::h(ui::Extent::Lines(tl))).max(ui::Bound::h(ui::Extent::Lines(tl))).child(strip));
		}
		ui::row()
			.align(ui::Align::Stretch)
			.min(ui::Bound::h(ui::Extent::Lines(lines)))
			.max(ui::Bound::h(ui::Extent::Lines(lines)))
			.child(html::div().class("files").child(list))
			.child(right)
	}

	// ---- input ----------------------------------------------------------------------------------

	/// Moves to the next (or previous) thread here; in Files, scrolls the diff to it.
	fn step_thread(&mut self, forward: bool) {
		let h = self.diff_height();
		let Some(p) = &mut self.page else { return };
		let ids: Vec<(u32, Option<u32>)> = p.threads_here().iter().map(|t| (t.id, t.line())).collect();
		let at = p.thread.and_then(|t| ids.iter().position(|(i, _)| *i == t));
		// In Files, going past this file's last comment moves to the next file that has one.
		if p.tab == PrTab::Files && forward && at.is_none_or(|a| a + 1 >= ids.len()) {
			let Some(d) = &p.detail else { return };
			let has = |i: usize| d.changes.get(i).is_some_and(|c| d.threads.iter().any(|t| t.shown() && t.file() == Some(c.path())));
			let n = d.changes.len();
			let start = if ids.is_empty() || at.is_some() { p.file + 1 } else { p.file };
			let Some(next) = (0..n).map(|k| (start + k) % n).find(|&i| has(i)) else {
				self.note = "no comments on any file".into();
				return;
			};
			if next != p.file || at.is_some() {
				self.pick_file(next);
				let Some(p) = &mut self.page else { return };
				if let Some(first) = p.file_threads().first().map(|t| (t.id, t.line())) {
					p.thread = Some(first.0);
					p.pending = first.1;
					if let Some(line) = first.1
						&& let Some(i) = p.diff().and_then(|d| d.find(u64::from(line)))
					{
						p.offset = i.saturating_sub(h / 3);
						p.pending = None;
					}
				}
				return;
			}
		}
		if ids.is_empty() {
			self.note = "no comments here".into();
			return;
		}
		let next = match (at, forward) {
			(None, true) => 0,
			(None, false) => ids.len() - 1,
			(Some(a), true) => (a + 1) % ids.len(),
			(Some(a), false) => (a + ids.len() - 1) % ids.len(),
		};
		p.thread = Some(ids[next].0);
		if p.tab == PrTab::Files
			&& let Some(line) = ids[next].1
			&& let Some(i) = p.diff().and_then(|d| d.find(u64::from(line)))
		{
			p.offset = i.saturating_sub(h / 3);
		}
	}

	fn scroll(&mut self, delta: i64) {
		let h = self.diff_height();
		let Some(p) = &mut self.page else { return };
		if p.tab == PrTab::Overview {
			p.ov_skip = (p.ov_skip as i64 + delta.signum()).max(0) as usize;
			return;
		}
		let len = p.diff().map_or(0, FileDiff::len);
		let max = len.saturating_sub(h.min(len) / 2);
		p.offset = (p.offset as i64 + delta).clamp(0, max as i64) as usize;
	}

	fn pick_file(&mut self, i: usize) {
		if let Some(p) = &mut self.page {
			let n = p.detail.as_ref().map_or(0, |d| d.changes.len());
			if n == 0 {
				return;
			}
			p.file = i.min(n - 1);
			p.offset = 0;
			p.thread = None;
		}
		self.load_file();
	}

	fn copy(&mut self, text: &str, what: &str) {
		self.note = match crate::clip::set(text) {
			Ok(()) => format!("copied {what}"),
			Err(e) => format!("copy failed: {e}"),
		};
	}

	fn send(&mut self, text: String) {
		let Some(c) = self.compose.take() else { return };
		match c.kind {
			ComposeKind::Org => match config::normalize(&text) {
				Some(org) => {
					if let Err(e) = config::save(&config::Config { org: org.clone() }) {
						self.note = format!("could not save the config: {e}");
					}
					self.connect(&org);
				},
				None => {
					self.compose = Some(Composer::new(ComposeKind::Org, text));
					self.note = "that does not look like an Azure DevOps organization".into();
				},
			},
			ComposeKind::Filter => {
				self.filter = text.trim().to_owned();
				self.fix_selection();
			},
			ComposeKind::Comment(thread) => {
				if text.trim().is_empty() {
					return;
				}
				if let Some(p) = &self.page {
					let pr = p.pr.clone();
					self.note = "sending…".into();
					self.start(Job::Comment { pr, thread, text: text.trim_end().to_owned() });
				}
			},
		}
	}

	/// A key while the composer is up. True when it closed.
	fn compose_key(&mut self, key: &Key) -> bool {
		let Some(kind) = self.compose.as_ref().map(|c| c.kind) else { return false };
		if key.is("escape") {
			if kind == ComposeKind::Filter {
				self.filter.clear();
				self.fix_selection();
			}
			// Without an organization there is nothing to go back to.
			if kind == ComposeKind::Org && self.client.is_none() {
				return false;
			}
			self.compose = None;
			return true;
		}
		let Some(c) = &mut self.compose else { return false };
		let multiline = matches!(kind, ComposeKind::Comment(_));
		if key.name == "enter" {
			if multiline && (key.shift || key.alt) {
				c.insert("\n");
				return false;
			}
			let text = std::mem::take(&mut c.draft);
			self.send(text);
			return true;
		}
		c.key(key);
		if c.kind == ComposeKind::Filter {
			self.filter = c.draft.clone();
			self.fix_selection();
		}
		false
	}

	/// A key with nothing else taking it. False to quit.
	fn key(&mut self, key: &Key) -> bool {
		if self.voting {
			self.voting = false;
			let vote = match key.name.as_str() {
				"1" => Some(10),
				"2" => Some(5),
				"3" => Some(-5),
				"4" => Some(-10),
				"0" => Some(0),
				_ => None,
			};
			if let (Some(v), Some(p), Some(me)) = (vote, &self.page, &self.me) {
				let job = Job::Vote { pr: p.pr.clone(), me: me.id.clone(), vote: v };
				self.note = format!("voting {}…", api::vote_label(v));
				self.start(job);
			}
			return true;
		}
		if key.is("ctrl+q") {
			return false;
		}
		if key.is("ctrl+f") && self.page.is_none() {
			self.compose = Some(Composer::new(ComposeKind::Filter, self.filter.clone()));
			return true;
		}
		let url = || self.selected_pr().and_then(|p| Some(self.client.as_ref()?.web_url(p)));
		match key.name.as_str() {
			"r" if !key.ctrl => self.refresh(),
			"o" => {
				if let Some(u) = url() {
					self.note = match open_url(&u) {
						Ok(()) => "opened in the browser".into(),
						Err(e) => format!("could not open the browser: {e}"),
					};
				}
			},
			"y" => {
				if let Some(u) = url() {
					self.copy(&u, "link");
				}
			},
			"b" => {
				if let Some(b) = self.selected_pr().map(|p| p.source().to_owned()) {
					self.copy(&b, "branch name");
				}
			},
			_ if self.page.is_some() => self.page_key(key),
			_ => return self.list_key(key),
		}
		true
	}

	fn list_key(&mut self, key: &Key) -> bool {
		let cap = self.rows_in(self.list_lines()) as i64;
		match key.name.as_str() {
			"q" => return false,
			"down" | "j" => self.move_sel(1),
			"up" | "k" => self.move_sel(-1),
			"page_down" => self.move_sel(cap - 2),
			"page_up" => self.move_sel(-(cap - 2)),
			"home" => self.move_sel(i64::MIN / 2),
			"end" => self.move_sel(i64::MAX / 2),
			"enter" => {
				if let Some(id) = self.sel {
					self.open(id);
				}
			},
			"1" | "2" | "3" => {
				self.tab = ListTab::ALL[key.name.parse::<usize>().unwrap_or(1) - 1];
				self.fix_selection();
			},
			"tab" => {
				let i = ListTab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
				self.tab = ListTab::ALL[(i + if key.shift { 2 } else { 1 }) % 3];
				self.fix_selection();
			},
			// Tern can keep `/` for itself, so `f` and Ctrl+F open the filter too.
			"/" | "f" => self.compose = Some(Composer::new(ComposeKind::Filter, self.filter.clone())),
			"escape" if !self.filter.is_empty() => {
				self.filter.clear();
				self.fix_selection();
			},
			_ => {},
		}
		true
	}

	fn page_key(&mut self, key: &Key) {
		let h = self.diff_height() as i64;
		let Some(p) = &mut self.page else { return };
		let files = p.tab == PrTab::Files;
		match key.name.as_str() {
			"escape" | "backspace" | "q" => {
				if p.thread.is_some() {
					p.thread = None;
				} else {
					self.page = None;
					self.fix_selection();
				}
			},
			"tab" | "1" | "2" => {
				p.tab = match key.name.as_str() {
					"1" => PrTab::Overview,
					"2" => PrTab::Files,
					_ if files => PrTab::Overview,
					_ => PrTab::Files,
				};
				p.thread = None;
				if p.tab == PrTab::Files {
					self.load_file();
				}
			},
			"down" | "j" if files => {
				let i = p.file + 1;
				self.pick_file(i);
			},
			"up" | "k" if files => {
				let i = p.file.saturating_sub(1);
				self.pick_file(i);
			},
			"down" | "j" => self.scroll(1),
			"up" | "k" => self.scroll(-1),
			"page_down" | "space" | "f" => {
				let step = if files { self.diff_fit() as i64 - 1 } else { h - 2 };
				self.scroll(step.max(1));
			},
			"page_up" | "u" => self.scroll(-(h - 2)),
			"home" => self.scroll(i64::MIN / 2),
			"end" => self.scroll(i64::MAX / 2),
			"n" => self.step_thread(!key.shift),
			"p" | "N" => self.step_thread(false),
			"x" if files => {
				p.full = !p.full;
				self.rebuild_diffs();
			},
			"v" => self.voting = true,
			"c" => self.compose = Some(Composer::new(ComposeKind::Comment(None), String::new())),
			"a" => match p.thread {
				Some(t) => self.compose = Some(Composer::new(ComposeKind::Comment(Some(t)), String::new())),
				None => self.note = "pick a comment first (n / p, or click one)".into(),
			},
			_ => {},
		}
	}

	fn msg(&mut self, msg: Msg) {
		match msg {
			Msg::Pick(id) => self.sel = Some(id),
			Msg::Open(id) => self.open(id),
			Msg::ListTab(id) => {
				if let Some(t) = ListTab::ALL.into_iter().find(|t| t.id() == id) {
					self.tab = t;
					self.fix_selection();
				}
			},
			Msg::PrTab(id) => {
				if let Some(p) = &mut self.page {
					p.tab = if id == "files" { PrTab::Files } else { PrTab::Overview };
					p.thread = None;
				}
				self.load_file();
			},
			Msg::File(i) => self.pick_file(i),
			Msg::Thread(t) => {
				if let Some(p) = &mut self.page {
					p.thread = if p.thread == Some(t) { None } else { Some(t) };
				}
			},
			Msg::Send(text) => self.send(text),
			Msg::Edit(e) => {
				if let Some(c) = &mut self.compose {
					if !c.apply(&e) {
						self.note = "the comment box got out of step; press Esc and start again".into();
					}
					if c.kind == ComposeKind::Filter {
						self.filter = c.draft.clone();
						self.fix_selection();
					}
				}
			},
			Msg::Focus => {},
		}
	}
}

pub fn run(org: Option<String>, demo: bool) -> anyhow::Result<()> {
	let Some(mut session) = Session::<Msg>::connect(Options::new().feature(feature::SEND).feature(feature::EDIT))? else {
		anyhow::bail!("the interactive view needs Tern; use `tern-azdo list` elsewhere");
	};
	let sf = session.open(SurfaceOptions::screen().title("Pull Requests").role("azdo"))?;
	session.stylesheet(sf, "azdo", Some(CSS))?;
	let mut app = App::new(org, demo);
	let mut focused = false;
	let mut rows_checked = Instant::now();
	// TERN_AZDO_TRACE=file logs every input, for seeing what Tern sends.
	let mut trace = std::env::var_os("TERN_AZDO_TRACE").and_then(|p| std::fs::File::create(p).ok());
	loop {
		session.render(sf, app.view())?;
		// After the render, so the composer exists when it takes focus.
		let want_focus = app.compose.is_some();
		if want_focus != focused {
			session.focus(sf, want_focus.then_some(COMPOSER))?;
			focused = want_focus;
		}
		let wait = Duration::from_millis(if app.busy > 0 { 100 } else { 1000 });
		let input = loop {
			match session.next(Some(wait)) {
				Ok(Some(i)) => break Some(i),
				Ok(None) => {
					if app.poll() {
						break None;
					}
					if rows_checked.elapsed() > Duration::from_millis(1500) {
						rows_checked = Instant::now();
						if let Some(r) = pane_rows().filter(|r| (*r - app.rows).abs() > 0.1) {
							app.rows = r;
							break None;
						}
					}
				},
				Err(tern_sdk::Error::InputClosed) => return session.close().map_err(Into::into),
				Err(e) => return Err(e.into()),
			}
		};
		app.poll();
		let Some(input) = input else { continue };
		if let Some(log) = &mut trace {
			let _ = std::io::Write::write_all(log, format!("{input:?}
").as_bytes());
		}
		match input {
			Input::Key(key) => {
				if app.compose.is_some() {
					app.compose_key(&key);
				} else if !app.key(&key) {
					break;
				}
			},
			Input::Msg(m, _) => app.msg(m),
			Input::Event(Event::Resize(r)) => {
				app.rows = pane_rows().unwrap_or(app.rows);
				if let Some(c) = r.cell {
					app.cell_h = c.h;
				}
				app.cols = f64::from(r.cols);
			},
			_ => {},
		}
	}
	session.close()?;
	Ok(())
}
