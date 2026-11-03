//! The interactive client: the org's active pull requests, and a page per
//! pull request with its overview, changed files, diffs and comments.

use std::{
	collections::{HashMap, HashSet},
	sync::{
		Arc,
		mpsc::{self, Receiver, Sender},
	},
	time::{Duration, Instant},
};

use tern_sdk::{
	Input, Options, Session, SurfaceOptions,
	keys::Key,
	ui::{self, Hunk, Side, Tone, View, html},
	wire::{self, Event, RevealAt, ScrollBy, feature},
};

use crate::{
	api::{self, Client, Detail, Identity, Pr, Thread},
	config,
	diff::{self, FileDiff},
	tree,
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
.who{font-weight:600}\
.head{font-family:Geist,system-ui,sans-serif;font-size:12.9px;padding:2px 4px;display:flex;gap:10px;align-items:center;white-space:nowrap;overflow:hidden}\
.title{font-weight:700;font-size:14px;overflow:hidden;text-overflow:ellipsis}\
.prompt{display:flex;gap:14px;align-items:center;height:28px;padding:0 10px;border:1px solid var(--sf-c-accent);border-radius:6px;font-family:Geist,system-ui,sans-serif;font-size:12.9px}\
.prompt .pl{color:var(--sf-c-accent);font-weight:600}\
.hsub{gap:8px;padding-top:4px;padding-bottom:6px}\
.br{font-family:\"Geist Mono\",ui-monospace,monospace;font-size:12px;padding:1px 6px;border-radius:4px;background:color-mix(in srgb,var(--sf-c-accent) 14%,transparent);color:var(--sf-c-accent)}\
.right{margin-left:auto}\
.acc{color:var(--sf-c-accent)}\
.ov{display:flex;gap:16px;height:100%;box-sizing:border-box;padding-top:8px;font-family:Geist,system-ui,sans-serif;font-size:12.9px;line-height:1.45;overflow:hidden}\
.ovmain{flex:1;min-width:0;display:flex;flex-direction:column;overflow-y:auto;padding-right:6px}\
.ovside{width:300px;flex-shrink:0;display:flex;flex-direction:column;overflow:hidden}\
.card{flex-shrink:0;margin-bottom:12px;border:1px solid color-mix(in srgb,var(--sf-c-muted) 32%,transparent);border-radius:8px;background:color-mix(in srgb,var(--sf-c-muted) 6%,transparent);padding:12px 16px}\
.card.cur{border-color:var(--sf-c-accent);background:color-mix(in srgb,var(--sf-c-accent) 7%,transparent)}\
.ch{font-weight:650;font-size:13.5px;margin-bottom:8px}\
.grh{color:var(--sf-c-muted);font-size:11.5px;font-weight:600;margin:8px 0 2px}\
.st{display:flex;align-items:center;gap:10px;padding:3px 0}\
.gl{width:16px;text-align:center;font-weight:700;flex-shrink:0}\
.ah{display:flex;align-items:baseline;gap:12px;flex-shrink:0;padding:4px 2px 10px}\
.at{font-weight:650;font-size:13.5px}\
.tli{display:flex;gap:12px;flex-shrink:0}\
.tlr{width:14px;flex-shrink:0;display:flex;flex-direction:column;align-items:center}\
.tld{width:10px;height:10px;border-radius:5px;margin-top:9px;flex-shrink:0;background:var(--sf-c-muted)}\
.tld.acc{background:var(--sf-c-accent)}.tld.ok{background:var(--sf-good,#3fb950)}.tld.wait{background:var(--sf-warn,#d29922)}.tld.bad{background:var(--sf-bad,#f85149)}\
.tls{flex:1;width:2px;margin-top:4px;background:color-mix(in srgb,var(--sf-c-muted) 28%,transparent)}\
.tlx{flex:1;min-width:0}\
.ev{display:flex;align-items:center;gap:8px;min-height:28px;margin-bottom:10px;padding:0 4px}\
.av{width:22px;height:22px;border-radius:11px;display:flex;align-items:center;justify-content:center;font-size:10px;font-weight:700;color:#fff;flex-shrink:0;line-height:1}\
.av0{background:#4f5bd5}.av1{background:#b4532a}.av2{background:#0f766e}.av3{background:#9d3fa8}.av4{background:#a16207}.av5{background:#2f6fd6}\
.name{font-weight:650;text-decoration:underline;text-decoration-color:color-mix(in srgb,var(--sf-c-accent) 60%,transparent)}\
.when{color:var(--sf-c-muted);font-size:11.5px;white-space:nowrap}\
.thd{display:flex;align-items:center;gap:10px;margin-bottom:10px;white-space:nowrap;overflow:hidden}\
.fname{font-weight:650}.fdir{overflow:hidden;text-overflow:ellipsis}.ln{color:var(--sf-c-accent);font-size:11.5px;white-space:nowrap}\
.chip.acc{border-color:var(--sf-c-accent);color:var(--sf-c-accent)}.chip.ok{border-color:var(--sf-good,#3fb950);color:var(--sf-good,#3fb950)}.chip.wait{border-color:var(--sf-warn,#d29922);color:var(--sf-warn,#d29922)}\
.snip{font-family:\"Geist Mono\",ui-monospace,monospace;font-size:12px;line-height:1.35;margin:0 0 4px;border-radius:6px;overflow:hidden;border:1px solid color-mix(in srgb,var(--sf-c-muted) 25%,transparent)}\
.cm{display:flex;gap:12px;padding:10px 0 4px}\
.cm.rep{border-top:1px solid color-mix(in srgb,var(--sf-c-muted) 22%,transparent)}\
.cmb{flex:1;min-width:0}\
.cmh{display:flex;align-items:center;gap:10px;margin-bottom:2px}\
.hint{color:var(--sf-c-muted);font-size:11.5px;padding-top:6px}\
.rv{display:flex;align-items:center;gap:10px;padding:4px 0}\
.vg{width:14px;text-align:center;font-weight:700;flex-shrink:0}\
.small{font-size:11.5px}\
.wi{padding:4px 0}.wk{color:var(--sf-c-accent);font-size:11.5px}\
.azfv{display:flex;gap:16px;height:100%;overflow:hidden;box-sizing:border-box;padding-top:10px;font-family:Geist,system-ui,sans-serif;font-size:12.9px}\
.aztree{max-height:100%;width:30%;min-width:240px;max-width:420px;flex-shrink:0;box-sizing:border-box;overflow-y:auto;padding:8px 6px;border:1px solid color-mix(in srgb,var(--sf-c-muted) 32%,transparent);border-radius:8px;background:color-mix(in srgb,var(--sf-c-muted) 4%,transparent)}\
.aztr{display:flex;align-items:center;gap:8px;height:26px;box-sizing:border-box;padding-right:10px;white-space:nowrap;border-radius:5px;cursor:pointer;overflow:hidden}\
.aztr:hover{background:color-mix(in srgb,var(--sf-c-muted) 14%,transparent)}\
.aztr.cur{background:color-mix(in srgb,var(--sf-c-accent) 24%,transparent)}\
.azdir{color:var(--sf-c-muted);font-weight:600}\
.azch{width:12px;text-align:center;flex-shrink:0}\
.azd0{padding-left:8px}.azd1{padding-left:24px}.azd2{padding-left:40px}.azd3{padding-left:56px}.azd4{padding-left:72px}.azd5{padding-left:88px}.azd6{padding-left:104px}.azd7{padding-left:120px}.azd8{padding-left:136px}.azd9{padding-left:152px}\
.azdp{max-height:100%;flex:1;min-width:0;display:flex;flex-direction:column;box-sizing:border-box;border:1px solid color-mix(in srgb,var(--sf-c-muted) 32%,transparent);border-radius:8px;overflow:hidden}\
.azdh{display:flex;align-items:center;gap:12px;padding:10px 14px;border-bottom:1px solid color-mix(in srgb,var(--sf-c-muted) 25%,transparent);white-space:nowrap;flex-shrink:0;overflow:hidden}\
.azds{flex:1;min-height:0;overflow-y:auto;padding:10px 12px 18px;font-family:\"Geist Mono\",ui-monospace,monospace;font-size:12px;line-height:1.35}\
.azic{margin:12px 8px 16px 44px;font-family:Geist,system-ui,sans-serif;font-size:12.9px;line-height:1.45}\
.azic .card{margin-bottom:0}\
.azmsg{padding:16px;color:var(--sf-c-muted)}\
.aztog{display:flex;flex-shrink:0;border:1px solid color-mix(in srgb,var(--sf-c-muted) 45%,transparent);border-radius:6px;overflow:hidden}\
.aztog span{padding:2px 10px;cursor:pointer;color:var(--sf-c-muted)}\
.aztog span.on{background:color-mix(in srgb,var(--sf-c-accent) 22%,transparent);color:var(--sf-c-accent)}\
.strip{height:100%;box-sizing:border-box;display:flex;flex-direction:column;overflow:hidden;padding-top:10px;font-family:Geist,system-ui,sans-serif;font-size:12.9px;line-height:1.45}";

#[derive(Clone, Debug)]
enum Msg {
	/// A list row was clicked.
	Pick(u32),
	/// A list row was double-clicked.
	Open(u32),
	ListTab(String),
	PrTab(String),
	File(usize),
	/// A folder row in the file tree: fold or unfold it.
	Folder(String),
	/// Side by side (true) or inline.
	Split(bool),
	/// The whole file (true) or the changes with context.
	Full(bool),
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
	/// The line an inline comment goes on.
	Line,
	/// An inline comment on a line of the selected file (the old side when `left`).
	Inline { line: u32, left: bool },
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
	/// Diffs side by side.
	split:   bool,
	/// Folders folded in the file tree.
	folded:  HashSet<String>,
	/// Whole files instead of hunks.
	full:    bool,
	/// The selected comment thread.
	thread:  Option<u32>,
	/// A thread to scroll the diff to once the selected file loads.
	pending: Option<u32>,
}

impl Page {
	fn new(pr: Pr) -> Self {
		Self { pr, tab: PrTab::Overview, detail: None, error: None, file: 0, files: HashMap::new(), split: false, folded: HashSet::new(), full: false, thread: None, pending: None }
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

	/// Everything that happened, newest first: comment threads (by their latest comment), system events and the creation.
	fn timeline(&self) -> Vec<Item<'_>> {
		let mut items: Vec<(i64, Item<'_>)> = Vec::new();
		if let Some(d) = &self.detail {
			for t in &d.threads {
				if t.shown() {
					items.push((stamp(t.latest()), Item::Thread(t)));
				} else if let Some(e) = t.event() {
					items.push((stamp(t.started()), Item::Event(e, t.started())));
				}
			}
		}
		items.push((stamp(&self.pr.creation_date), Item::Created));
		items.sort_by_key(|i| std::cmp::Reverse(i.0));
		items.into_iter().map(|(_, i)| i).collect()
	}

	/// Comment threads in timeline order.
	fn all_threads(&self) -> Vec<&Thread> {
		self.timeline().into_iter().filter_map(|i| if let Item::Thread(t) = i { Some(t) } else { None }).collect()
	}

	/// The code a thread is about (its first five lines at most), with two lines before it: from the diff when those lines changed, else from the file.
	fn snippet(&self, t: &Thread) -> Option<Vec<Hunk>> {
		let FileState::Ready { sides, diff } = self.files.get(t.file()?)? else { return None };
		let line = u64::from(t.line()?);
		let end = u64::from(t.line_end().unwrap_or(0)).clamp(line, line + 4);
		let left = t.on_left();
		let at = diff.lines.iter().position(|l| if left { l.old == Some(line) && l.tag != '+' } else { l.new == Some(line) && l.tag != '-' });
		if let Some(i) = at {
			let start = i.saturating_sub(2);
			return Some(diff.window(start, i - start + (end - line) as usize + 1));
		}
		let text = if left { sides.0.as_ref()? } else { sides.1.as_ref()? };
		let lines: Vec<&str> = text.lines().collect();
		let first = line.saturating_sub(2).max(1);
		let last = end.min(lines.len() as u64);
		if first > last {
			return None;
		}
		let body = (first..=last).map(|n| format!(" {}", lines[n as usize - 1])).collect();
		Some(vec![Hunk { old_start: Some(first), new_start: Some(first), lines: body }])
	}

	/// The selected file has line `n` on its new side (old side when `left`).
	fn has_line(&self, n: u32, left: bool) -> bool {
		let Some(c) = self.change() else { return false };
		match self.files.get(c.path()) {
			Some(FileState::Ready { sides, .. }) => {
				let side = if left { &sides.0 } else { &sides.1 };
				n > 0 && side.as_ref().is_some_and(|t| t.lines().count() >= n as usize)
			},
			_ => false,
		}
	}

	/// The change paths, in change-list order.
	fn paths(&self) -> Vec<&str> {
		self.detail.as_ref().map(|d| d.changes.iter().map(api::Change::path).collect()).unwrap_or_default()
	}

	/// Whether the selected file's diff is on screen (so its thread cards exist).
	fn file_ready(&self) -> bool {
		self.change().is_some_and(|c| matches!(self.files.get(c.path()), Some(FileState::Ready { .. })))
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
	/// A scroll for Tern to do after the next render.
	scroll_req: Option<ScrollReq>,
	/// A file tree row to bring into view after the next render.
	tree_req:   Option<String>,
	/// A `g` was pressed: a second one goes to the top.
	g_pending:  bool,
}

/// A vim motion.
#[derive(Clone, Copy)]
enum Motion {
	Top,
	Bottom,
	/// A share of a screen, down when positive.
	Screen(f64),
}

enum ScrollReq {
	By(ScrollBy),
	/// Lines down (up when negative).
	Lines(i64),
	/// Bring a node to the top of the overview.
	Reveal(String),
}

/// The overview's scroll container, and a node inside it (for `scroll`, which acts on a node's container).
const OV_MAIN: &str = "main.pbody.ov.ovm";
const OV_FIRST: &str = "main.pbody.ov.ovm.status";
/// The Files tab's file tree, its diff's scroll container, and the node at the diff's top.
const TREE: &str = "main.pbody.fv.tree";
const FILES_DS: &str = "main.pbody.fv.dp.ds";
const FILES_FIRST: &str = "main.pbody.fv.dp.ds.top";

/// A folder row's key: its path hashed, because node ids are dotted and paths can hold dots.
fn dir_key(path: &str) -> String {
	let h = path.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
	format!("d{h:x}")
}

/// A line of the overview's timeline.
enum Item<'a> {
	Thread(&'a Thread),
	/// A system event and when it happened.
	Event(api::Event, &'a str),
	Created,
}

/// Milliseconds since the epoch for an RFC 3339 time (0 when it does not parse).
fn stamp(when: &str) -> i64 {
	chrono::DateTime::parse_from_rfc3339(when).map_or(0, |t| t.timestamp_millis())
}

/// `just now`, `12m ago`, `5h ago`, `yesterday`, `3d ago`, then the date.
fn ago(when: &str) -> String {
	let Ok(t) = chrono::DateTime::parse_from_rfc3339(when) else { return String::new() };
	let secs = (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds().max(0);
	match secs {
		s if s < 60 => "just now".into(),
		s if s < 3600 => format!("{}m ago", s / 60),
		s if s < 86_400 => format!("{}h ago", s / 3600),
		s if s < 86_400 * 2 => "yesterday".into(),
		s if s < 86_400 * 30 => format!("{}d ago", s / 86_400),
		_ => t.with_timezone(&chrono::Local).format("%b %-d, %Y").to_string(),
	}
}

/// The full local time, shown on hover.
fn when_full(when: &str) -> String {
	chrono::DateTime::parse_from_rfc3339(when).map(|t| t.with_timezone(&chrono::Local).format("%a %b %-d, %Y at %-I:%M %p").to_string()).unwrap_or_default()
}

/// `PS` for Priya Shah.
fn initials(name: &str) -> String {
	let mut words = name.split_whitespace().filter_map(|w| w.chars().find(|c| c.is_alphanumeric()));
	let first = words.next();
	let last = words.next_back();
	[first, last].into_iter().flatten().flat_map(char::to_uppercase).collect()
}

/// A round badge with someone's initials, the same color for the same name everywhere.
fn avatar(name: &str) -> html::El<Msg> {
	let h = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
	html::span(initials(name)).class(format!("av av{}", h % 6)).title(name.to_owned())
}

fn person(name: &str) -> html::El<Msg> {
	html::span(name.to_owned()).class("name")
}

fn time(when: &str, extra: &str) -> html::El<Msg> {
	html::span(ago(when)).class(format!("when {extra}")).title(when_full(when))
}

/// A timeline entry: a dot and the line down to the next entry, then the entry.
fn tl_item(dot: &str, last: bool, body: html::El<Msg>) -> html::El<Msg> {
	let mut rail = html::div().class("tlr").child(html::div().class(format!("tld {dot}")));
	if !last {
		rail = rail.child(html::div().class("tls"));
	}
	html::div().class("tli").child(rail).child(html::div().class("tlx").child(body))
}

/// One line of the timeline: who did what, and when.
fn event_row(who: &str, what: &str, when: &str) -> html::El<Msg> {
	html::div().class("ev").child(avatar(who)).child(person(who)).child(html::span(what.to_owned())).child(time(when, "right"))
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
		_ => ("○", "dim"),
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
			scroll_req: None,
			g_pending: false,
			tree_req: None,
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
		if let Some(i) = self.page.as_ref().map(|p| p.file) {
			self.load_change(i);
		}
	}

	/// Loads every file that has comments, so the overview can show the code each one is about.
	fn prefetch_threads(&mut self) {
		let Some(d) = self.page.as_ref().and_then(|p| p.detail.as_ref()) else { return };
		let want: Vec<usize> =
			d.changes.iter().enumerate().filter(|(_, c)| d.threads.iter().any(|t| t.shown() && t.file() == Some(c.path()))).map(|(i, _)| i).collect();
		for i in want {
			self.load_change(i);
		}
	}

	/// Loads change `i`'s sides if they are not loaded yet.
	fn load_change(&mut self, i: usize) {
		let Some(p) = &self.page else { return };
		let Some(c) = p.detail.as_ref().and_then(|d| d.changes.get(i)) else { return };
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
					self.prefetch_threads();
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
					if current && let Some(id) = p.pending.take() {
						self.scroll_req = Some(ScrollReq::Reveal(format!("{FILES_DS}.t{id}")));
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

	/// The overview has room for the reviewers beside it.
	fn wide(&self) -> bool {
		self.cols >= 130.0
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
			let file = self.page.as_ref().and_then(Page::change).map(|c| c.path().rsplit('/').next().unwrap_or_default().to_owned()).unwrap_or_default();
			let placeholder: String = match c.kind {
				ComposeKind::Org => "Organization, e.g. Contoso or https://dev.azure.com/Contoso (Enter saves)".into(),
				ComposeKind::Filter => "Filter by title, author, repo, branch or id (Enter keeps, Esc clears)".into(),
				ComposeKind::Comment(None) => "New comment on the pull request (Enter sends, Shift+Enter new line, Esc cancels)".into(),
				ComposeKind::Comment(Some(_)) => "Reply (Enter sends, Shift+Enter new line, Esc cancels)".into(),
				ComposeKind::Line => format!("Line of {file} to comment on, as numbered in the diff; -12 for a removed line (Enter goes on, Esc cancels)"),
				ComposeKind::Inline { line, left } => {
					format!("Comment on {}line {line} of {file} (Enter sends, Shift+Enter new line, Esc cancels)", if left { "removed " } else { "" })
				},
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
		// A message takes the key hint's place: the bar drops what does not fit.
		if let Some(e) = &self.error {
			return s.child(ui::seg(first_line(e, 140)).tone(Tone::Error));
		}
		if !self.note.is_empty() {
			return s.child(ui::seg(self.note.clone()));
		}
		let files = self.page.as_ref().is_some_and(|p| p.tab == PrTab::Files);
		let keys = if files {
			"Esc back · Tab overview · j/k file · h/l fold · ^d/^u gg/G scroll · s split · x whole file · n next comment · c comment on a line · a reply"
		} else if self.page.is_some() {
			"Esc back · Tab files · j/k ^d/^u gg/G scroll · n next comment · a reply · c comment · v vote · o browser · r refresh"
		} else {
			"Enter open · j/k gg/G · 1-3 tabs · f filter · o browser · y copy link · r refresh · q quit"
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
		let sub = html::div()
			.class("head hsub")
			.child(avatar(&pr.created_by.display_name))
			.child(person(&pr.created_by.display_name))
			.child(html::span("wants to merge").class("dim"))
			.child(html::span(pr.source().to_owned()).class("br"))
			.child(html::span("into").class("dim"))
			.child(html::span(pr.target().to_owned()).class("br"))
			.child(html::span(format!("{} / {}", pr.repository.project.name, pr.repository.name)).class("dim right"))
			.child(html::span("·").class("dim"))
			.child(time(&pr.creation_date, ""));
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

	/// A comment thread as a card: where it is, the code it is about, then every comment in full.
	fn thread_card(&self, p: &Page, t: &Thread, with_code: bool) -> html::El<Msg> {
		let selected = p.thread == Some(t.id);
		let (label, tone) = api::status_label(t.status.as_deref());
		let mut head = html::div().class("thd");
		if let Some(path) = t.file() {
			let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
			head = head.child(html::span(file.to_owned()).class("fname")).child(html::span(dir.trim_start_matches('/').to_owned()).class("dim fdir"));
			if let (Some(a), Some(b)) = (t.line(), t.line_end()) {
				head = head.child(html::span(if b > a { format!("lines {a}-{b}") } else { format!("line {a}") }).class("ln"));
			}
		} else {
			head = head.child(html::span("Comment on the pull request").class("dim"));
		}
		head = head.child(html::span(label).class(format!("chip {tone} right")));
		let mut card = html::div().class(if selected { "card cur" } else { "card" }).on_click(Msg::Thread(t.id)).child(head);
		if with_code && let Some(hunks) = p.snippet(t) {
			let diff = ui::diff().key(format!("sn{}", t.id)).hunks(hunks).path(t.file().unwrap_or_default().to_owned()).mode(ui::DiffMode::Unified);
			card = card.child(html::div().class("snip").child(diff));
		}
		for (i, c) in t.human().enumerate() {
			let when = c.published_date.as_deref().unwrap_or("");
			let mut meta = html::div().class("cmh").child(person(&c.author.display_name)).child(time(when, ""));
			if c.last_content_updated_date.as_deref().is_some_and(|u| stamp(u) - stamp(when) > 60_000) {
				meta = meta.child(html::span("edited").class("when"));
			}
			let body = html::div().class("cmb").child(meta).child(ui::md(c.content.clone().unwrap_or_default()));
			card = card.child(html::div().class(if i == 0 { "cm" } else { "cm rep" }).child(avatar(&c.author.display_name)).child(body));
		}
		if selected {
			card = card.child(html::div().class("hint").child(html::span("a reply · n next · Esc done")));
		}
		card
	}

	/// Where the PR stands: votes, conflicts, open comments.
	fn status_card(&self, p: &Page) -> html::El<Msg> {
		let pr = &p.pr;
		let names = |pick: &dyn Fn(&api::Reviewer) -> bool| pr.reviewers.iter().filter(|r| pick(r)).map(|r| r.display_name.as_str()).collect::<Vec<_>>();
		let mut rows: Vec<(&str, &str, String)> = Vec::new();
		if pr.is_draft {
			rows.push(("✎", "dim", "Draft: not ready for review yet".into()));
		}
		let rejected = names(&|r| r.vote == -10);
		if !rejected.is_empty() {
			rows.push(("✗", "bad", format!("Rejected by {}", rejected.join(", "))));
		}
		let waiting = names(&|r| r.vote == -5);
		if !waiting.is_empty() {
			rows.push(("⏸", "wait", format!("{} {} waiting for the author", waiting.join(", "), if waiting.len() == 1 { "is" } else { "are" })));
		}
		let required = names(&|r| r.is_required && r.vote < 5);
		if !required.is_empty() {
			rows.push(("○", "wait", format!("Waiting on {} (required)", required.join(", "))));
		}
		rows.push(match pr.reviewers.iter().filter(|r| r.vote >= 5).count() {
			0 => ("○", "dim", "No approvals yet".into()),
			1 => ("✓", "ok", "1 reviewer approved".into()),
			n => ("✓", "ok", format!("{n} reviewers approved")),
		});
		rows.push(match pr.merge_status.as_deref() {
			Some("conflicts") => ("✗", "bad", "Merge conflicts with the target branch".into()),
			Some("failure") => ("✗", "bad", "The merge check failed".into()),
			Some("succeeded") => ("✓", "ok", "No merge conflicts".into()),
			_ => ("○", "dim", "Checking for merge conflicts".into()),
		});
		let threads = p.all_threads();
		let open = threads.iter().filter(|t| t.is_open()).count();
		if !threads.is_empty() {
			rows.push(if open == 0 {
				("✓", "ok", "All comments resolved".into())
			} else {
				("●", "acc", format!("{open} of {} comment threads active", threads.len()))
			});
		}
		html::div().class("card").children(
			rows.into_iter().map(|(g, class, text)| html::div().class("st").child(html::span(g).class(format!("gl {class}"))).child(html::span(text))),
		)
	}

	fn reviewers_card(&self, pr: &Pr) -> html::El<Msg> {
		let mut card = html::div().class("card").child(html::div().class("ch").child(html::span("Reviewers")));
		if pr.reviewers.is_empty() {
			return card.child(html::span("No reviewers").class("dim"));
		}
		let mut rs: Vec<&api::Reviewer> = pr.reviewers.iter().collect();
		rs.sort_by_key(|r| r.display_name.to_lowercase());
		for (label, required) in [("Required", true), ("Optional", false)] {
			let group: Vec<_> = rs.iter().filter(|r| r.is_required == required).collect();
			if group.is_empty() {
				continue;
			}
			card = card.child(html::div().class("grh").child(html::span(label)));
			for r in group {
				let (g, class) = vote_glyph(r.vote);
				let who = html::div().child(html::div().child(html::span(r.display_name.clone()))).child(html::div().class("dim small").child(html::span(api::vote_label(r.vote))));
				card = card.child(html::div().class("rv").child(html::span(g).class(format!("vg {class}"))).child(avatar(&r.display_name)).child(who));
			}
		}
		card
	}

	fn work_items_card(&self, p: &Page) -> Option<html::El<Msg>> {
		let d = p.detail.as_ref().filter(|d| !d.work_items.is_empty())?;
		let mut card = html::div().class("card").child(html::div().class("ch").child(html::span("Work items")));
		for w in &d.work_items {
			let tone = match w.state.as_str() {
				"Closed" | "Done" | "Resolved" | "Removed" => "ok",
				"New" | "To Do" => "dim",
				_ => "acc",
			};
			card = card.child(
				html::div()
					.class("wi")
					.child(html::div().child(html::span(format!("{} {}", w.kind, w.id)).class("wk")))
					.child(html::div().child(html::span(w.title.clone())))
					.child(html::div().class("small").child(html::span("● ").class(tone)).child(html::span(w.state.clone()).class("dim"))),
			);
		}
		Some(card)
	}

	fn overview(&self, p: &Page) -> html::El<Msg> {
		let pr = &p.pr;
		let wide = self.wide();
		let mut blocks: Vec<tern_sdk::Node<Msg>> = vec![self.status_card(p).key("status").into()];
		if !wide {
			blocks.push(self.reviewers_card(pr).into());
			if let Some(w) = self.work_items_card(p) {
				blocks.push(w.into());
			}
		}
		let desc = pr.description.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "_No description._".into());
		blocks.push(html::div().class("card").child(html::div().class("ch").child(html::span("Description"))).child(ui::md(desc)).into());
		blocks.push(
			html::div()
				.class("ah")
				.child(html::span("Activity").class("at"))
				.child(html::span("newest first · n jumps between comments · c adds one").class("dim small"))
				.into(),
		);
		let items = p.timeline();
		let n = items.len();
		for (i, item) in items.iter().enumerate() {
			let last = i + 1 == n;
			let node = match item {
				Item::Thread(t) => tl_item(if t.is_open() { "acc" } else { "ok" }, last, self.thread_card(p, t, true)).key(format!("t{}", t.id)),
				Item::Event(e, when) => {
					let dot = match e.vote {
						Some(10 | 5) => "ok",
						Some(-5) => "wait",
						Some(-10) => "bad",
						_ => "",
					};
					tl_item(dot, last, event_row(&e.who, &e.what, when))
				},
				Item::Created => tl_item("acc", last, event_row(&pr.created_by.display_name, "created the pull request", &pr.creation_date)),
			};
			blocks.push(node.into());
		}
		let mut ov = html::div().key("ov").class("ov").child(html::div().key("ovm").class("ovmain").children(blocks));
		if wide {
			ov = ov.child(html::div().class("ovside").child(self.reviewers_card(pr)).children(self.work_items_card(p)));
		}
		ov
	}

	fn files_view(&self, p: &Page) -> html::El<Msg> {
		let Some(d) = &p.detail else { return html::div().key("fv") };
		if d.changes.is_empty() {
			return html::div().key("fv").class("azmsg").child(html::span("No files changed."));
		}
		let counts: HashMap<&str, usize> = d.threads.iter().filter(|t| t.shown()).filter_map(Thread::file).fold(HashMap::new(), |mut m, f| {
			*m.entry(f).or_default() += 1;
			m
		});
		let paths = p.paths();
		let rows = tree::rows(&paths, &p.folded).into_iter().map(|r| -> tern_sdk::Node<Msg> {
			match r {
				tree::Row::Dir { path, label, depth, open } => html::div()
					.key(dir_key(&path))
					.class(format!("aztr azdir azd{}", depth.min(9)))
					.on_click(Msg::Folder(path))
					.child(html::span(if open { "▾" } else { "▸" }).class("azch"))
					.child(html::span(label))
					.into(),
				tree::Row::File { index, name, depth } => {
					let c = &d.changes[index];
					let mut row = html::div()
						.key(format!("f{index}"))
						.class(format!("aztr azd{}{}", depth.min(9), if index == p.file { " cur" } else { "" }))
						.on_click(Msg::File(index))
						.child(html::span(c.letter()).class(format!("fk {}", c.letter())))
						.child(html::span(name).class("fn"));
					if let Some(n) = counts.get(c.path()) {
						row = row.child(html::span(format!("💬{n}")).class("cc right"));
					}
					row.into()
				},
			}
		});
		html::div().key("fv").class("azfv").child(html::div().key("tree").class("aztree").children(rows)).child(self.diff_pane(p))
	}

	/// The selected file: its header, then its diff with each comment thread's card under the lines it is about.
	fn diff_pane(&self, p: &Page) -> html::El<Msg> {
		let Some(c) = p.change() else { return html::div().key("dp").class("azdp") };
		let path = c.path();
		let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
		let mut head = html::div()
			.class("azdh")
			.child(html::span(c.letter()).class(format!("fk {}", c.letter())))
			.child(html::span(name.to_owned()).class("fname"))
			.child(html::span(dir.trim_start_matches('/').to_owned()).class("dim fdir"));
		let threads = p.file_threads();
		let card = |t: &Thread| -> tern_sdk::Node<Msg> { html::div().key(format!("t{}", t.id)).class("azic").child(self.thread_card(p, t, false)).into() };
		let note = |text: &str| -> tern_sdk::Node<Msg> { html::div().class("azmsg").child(html::span(text.to_owned())).into() };
		let mut body: Vec<tern_sdk::Node<Msg>> = vec![html::div().key("top").into()];
		match p.files.get(path) {
			None | Some(FileState::Loading) => body.push(note("Loading…")),
			Some(FileState::Failed(e)) => body.push(note(e)),
			Some(FileState::Ready { sides, diff }) => {
				if sides.0.is_none() || sides.1.is_none() || diff.is_empty() {
					body.push(note(if sides.0.is_none() || sides.1.is_none() {
						"Binary or very large file: open it in the browser (o)."
					} else if c.is("rename") {
						"Renamed, no content changes."
					} else {
						"No content changes."
					}));
					body.extend(threads.iter().map(|t| card(t)));
				} else {
					head = head.child(html::span(format!("+{} −{}", diff.added(), diff.removed())).class("dim"));
					let mode = if p.split { ui::DiffMode::Split } else { ui::DiffMode::Unified };
					let mut placed: Vec<(usize, &Thread)> = threads
						.iter()
						.map(|t| {
							let at = t.line().map_or(0, |l| {
								let l = u64::from(l);
								diff.anchor(l, t.line_end().map_or(l, u64::from).max(l), t.on_left())
							});
							(at, *t)
						})
						.collect();
					placed.sort_by_key(|(at, t)| (*at, t.line()));
					let (mut from, mut k) = (0, 0);
					for (at, t) in placed {
						if at > from {
							body.push(ui::diff().key(format!("s{k}")).hunks(diff.window(from, at - from)).path(path.to_owned()).mode(mode.clone()).into());
							k += 1;
							from = at;
						}
						body.push(card(t));
					}
					if from < diff.len() {
						body.push(ui::diff().key(format!("s{k}")).hunks(diff.window(from, diff.len() - from)).path(path.to_owned()).mode(mode.clone()).into());
					}
				}
			},
		}
		let tog = |on: bool, label: &str, m: Msg| html::span(label.to_owned()).class(if on { "on" } else { "" }).on_click(m);
		head = head
			.child(html::div().class("aztog right").child(tog(!p.split, "Inline", Msg::Split(false))).child(tog(p.split, "Side by side", Msg::Split(true))))
			.child(html::div().class("aztog").child(tog(!p.full, "Changes", Msg::Full(false))).child(tog(p.full, "Whole file", Msg::Full(true))));
		html::div().key("dp").class("azdp").child(head).child(html::div().key("ds").class("azds").children(body))
	}

	// ---- input ----------------------------------------------------------------------------------

	/// Moves to the next (or previous) thread here; in Files, scrolls the diff to it.
	fn step_thread(&mut self, forward: bool) {
		let cycle = |at: Option<usize>, n: usize| match (at, forward) {
			(None, true) => 0,
			(None, false) => n - 1,
			(Some(a), true) => (a + 1) % n,
			(Some(a), false) => (a + n - 1) % n,
		};
		let Some(p) = &mut self.page else { return };
		if p.tab == PrTab::Overview {
			let ids: Vec<u32> = p.all_threads().iter().map(|t| t.id).collect();
			if ids.is_empty() {
				self.note = "no comments here".into();
				return;
			}
			let next = ids[cycle(p.thread.and_then(|t| ids.iter().position(|i| *i == t)), ids.len())];
			p.thread = Some(next);
			self.scroll_req = Some(ScrollReq::Reveal(format!("{OV_MAIN}.t{next}")));
			return;
		}
		// Files: every file's threads in tree order, then by line; stepping past a file's last opens the next.
		let Some(d) = &p.detail else { return };
		let paths = p.paths();
		let order = tree::order(&paths);
		let mut all: Vec<(usize, u32)> = Vec::new();
		for &i in &order {
			let mut ts: Vec<&Thread> = d.threads.iter().filter(|t| t.shown() && t.file() == Some(paths[i])).collect();
			ts.sort_by_key(|t| t.line().unwrap_or(0));
			all.extend(ts.into_iter().map(|t| (i, t.id)));
		}
		if all.is_empty() {
			self.note = "no comments on any file".into();
			return;
		}
		let pos = |i: usize| order.iter().position(|x| *x == i).unwrap_or(0);
		let at = p.thread.and_then(|t| all.iter().position(|(_, id)| *id == t));
		let next = match at {
			Some(_) => cycle(at, all.len()),
			// From no thread: the first one in or after this file (or before it, going back).
			None if forward => all.iter().position(|(i, _)| pos(*i) >= pos(p.file)).unwrap_or(0),
			None => all.iter().rposition(|(i, _)| pos(*i) <= pos(p.file)).unwrap_or(all.len() - 1),
		};
		let (file, id) = all[next];
		if file != p.file {
			self.pick_file(file);
		}
		let Some(p) = &mut self.page else { return };
		p.thread = Some(id);
		if p.file_ready() {
			self.scroll_req = Some(ScrollReq::Reveal(format!("{FILES_DS}.t{id}")));
		} else {
			p.pending = Some(id);
		}
	}

	/// The next (or previous) file in the tree, skipping folded folders.
	fn step_file(&mut self, forward: bool) {
		let Some(p) = &self.page else { return };
		let shown: Vec<usize> = tree::rows(&p.paths(), &p.folded)
			.into_iter()
			.filter_map(|r| match r {
				tree::Row::File { index, .. } => Some(index),
				tree::Row::Dir { .. } => None,
			})
			.collect();
		if shown.is_empty() {
			return;
		}
		let next = match shown.iter().position(|i| *i == p.file) {
			Some(a) if forward => (a + 1).min(shown.len() - 1),
			Some(a) => a.saturating_sub(1),
			None => 0,
		};
		self.pick_file(shown[next]);
	}

	/// Folds (or unfolds) the folder the selected file is in.
	fn fold_here(&mut self, fold: bool) {
		let Some(p) = &mut self.page else { return };
		let parents = tree::parents(&p.paths(), p.file);
		if fold {
			if let Some(inner) = parents.last() {
				p.folded.insert(inner.clone());
			}
		} else {
			for f in &parents {
				p.folded.remove(f);
			}
		}
	}

	fn scroll(&mut self, delta: i64) {
		if self.page.is_none() {
			return;
		}
		// Tern scrolls the overview and the diff itself. Its page step does nothing here, so a page is a screen's worth of line steps.
		let page = (self.page_body_lines() * 0.8) as i64;
		self.scroll_req = Some(match delta {
			d if d <= -10_000 => ScrollReq::By(ScrollBy::Start),
			d if d >= 10_000 => ScrollReq::By(ScrollBy::End),
			d if d < -1 => ScrollReq::Lines(-page),
			d if d > 1 => ScrollReq::Lines(page),
			d => ScrollReq::Lines(d * 3),
		});
	}

	fn pick_file(&mut self, i: usize) {
		let Some(p) = &mut self.page else { return };
		let n = p.detail.as_ref().map_or(0, |d| d.changes.len());
		if n == 0 {
			return;
		}
		p.file = i.min(n - 1);
		p.thread = None;
		p.pending = None;
		// Unfold the folders it is in, so its row can be shown.
		for f in tree::parents(&p.paths(), p.file) {
			p.folded.remove(&f);
		}
		self.tree_req = Some(format!("{TREE}.f{}", p.file));
		self.scroll_req = Some(ScrollReq::By(ScrollBy::Start));
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
			ComposeKind::Line => {
				let t = text.trim();
				let (left, num) = match t.strip_prefix('-') {
					Some(n) => (true, n.trim()),
					None => (false, t.trim_start_matches('+').trim()),
				};
				match num.parse::<u32>().ok().filter(|n| self.page.as_ref().is_some_and(|p| p.has_line(*n, left))) {
					Some(line) => self.compose = Some(Composer::new(ComposeKind::Inline { line, left }, String::new())),
					None => {
						self.note = format!("this file has no {}line {t}", if left { "old " } else { "" });
						self.compose = Some(Composer::new(ComposeKind::Line, text));
					},
				}
			},
			ComposeKind::Inline { line, left } => {
				if text.trim().is_empty() {
					return;
				}
				if let Some(p) = &self.page
					&& let Some(c) = p.change()
				{
					let at = api::LineRef {
						path: c.path().to_owned(),
						line,
						left,
						iteration: p.detail.as_ref().map_or(1, |d| d.iteration),
						tracking: c.change_tracking_id,
					};
					let job = Job::InlineComment { pr: p.pr.clone(), at, text: text.trim_end().to_owned() };
					self.note = "sending…".into();
					self.start(job);
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
		let multiline = matches!(kind, ComposeKind::Comment(_) | ComposeKind::Inline { .. });
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
		// Vim motions: gg and G, Ctrl+D / Ctrl+U (or d / u, as in less) for half a screen, Ctrl+F / Ctrl+B for a screen.
		let after_g = std::mem::take(&mut self.g_pending);
		let plain = !key.ctrl && !key.alt && !key.meta;
		let ctl = key.ctrl && !key.alt;
		let shift_g = plain && (key.name == "G" || (key.name == "g" && key.shift));
		if plain && key.name == "g" && !key.shift {
			if after_g {
				self.motion(Motion::Top);
			} else {
				self.g_pending = true;
			}
			return true;
		}
		let motion = match key.name.as_str() {
			_ if shift_g => Some(Motion::Bottom),
			"d" if plain || ctl => Some(Motion::Screen(0.5)),
			"u" if plain || ctl => Some(Motion::Screen(-0.5)),
			"f" if ctl => Some(Motion::Screen(1.0)),
			"b" if ctl => Some(Motion::Screen(-1.0)),
			_ => None,
		};
		if let Some(m) = motion {
			self.motion(m);
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

	/// A node inside the scroll container on screen: the overview's, or the diff's when a file shows.
	fn scroll_anchor(&self) -> Option<&'static str> {
		let p = self.page.as_ref()?;
		match p.tab {
			PrTab::Overview => p.detail.is_some().then_some(OV_FIRST),
			PrTab::Files => p.change().is_some().then_some(FILES_FIRST),
		}
	}

	/// Moves the list selection, the overview or the diff by a vim motion.
	fn motion(&mut self, m: Motion) {
		if self.page.is_none() {
			let cap = self.rows_in(self.list_lines()) as f64;
			match m {
				Motion::Top => self.move_sel(i64::MIN / 2),
				Motion::Bottom => self.move_sel(i64::MAX / 2),
				Motion::Screen(f) => self.move_sel(((cap * f.abs()).max(1.0) as i64) * f.signum() as i64),
			}
			return;
		}
		match m {
			Motion::Top => self.scroll(i64::MIN / 2),
			Motion::Bottom => self.scroll(i64::MAX / 2),
			// One Tern line step is about one text line, so a screen is the body's lines.
			Motion::Screen(f) => self.scroll_req = Some(ScrollReq::Lines((self.page_body_lines() * 0.9 * f) as i64)),
		}
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
			"down" | "j" if files => self.step_file(true),
			"up" | "k" if files => self.step_file(false),
			"h" | "left" if files => self.fold_here(true),
			"l" | "right" if files => self.fold_here(false),
			"s" if files => p.split = !p.split,
			"down" | "j" => self.scroll(1),
			"up" | "k" => self.scroll(-1),
			"page_down" | "space" | "f" => self.motion(Motion::Screen(1.0)),
			"page_up" => self.motion(Motion::Screen(-1.0)),
			"home" => self.scroll(i64::MIN / 2),
			"end" => self.scroll(i64::MAX / 2),
			"n" => self.step_thread(!key.shift),
			"p" | "N" => self.step_thread(false),
			"x" if files => {
				p.full = !p.full;
				self.rebuild_diffs();
			},
			"v" => self.voting = true,
			"c" if files => {
				if p.file_ready() {
					self.compose = Some(Composer::new(ComposeKind::Line, String::new()));
				} else {
					self.note = "the file is still loading".into();
				}
			},
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
			Msg::Folder(path) => {
				if let Some(p) = &mut self.page
					&& !p.folded.remove(&path)
				{
					p.folded.insert(path);
				}
			},
			Msg::Split(on) => {
				if let Some(p) = &mut self.page {
					p.split = on;
				}
			},
			Msg::Full(on) => {
				if let Some(p) = &mut self.page
					&& p.full != on
				{
					p.full = on;
					self.rebuild_diffs();
				}
			},
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

/// Tern can report Ctrl as meta; off macOS (where meta is Cmd) treat it as Ctrl so Ctrl shortcuts work.
fn ctrl_from_meta(mut key: Key) -> Key {
	if !cfg!(target_os = "macos") && key.meta && !key.ctrl {
		key.meta = false;
		key.ctrl = true;
	}
	key
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
		if let Some(req) = app.scroll_req.take()
			&& let Some(anchor) = app.scroll_anchor()
		{
			let done = match req {
				ScrollReq::Lines(n) => {
					let by = if n < 0 { ScrollBy::LineUp } else { ScrollBy::LineDown };
					(0..n.unsigned_abs()).try_for_each(|_| session.scroll(sf, anchor, by))
				},
				ScrollReq::By(by) => session.scroll(sf, anchor, by),
				ScrollReq::Reveal(id) => session.reveal(sf, &id, RevealAt::Start),
			};
			if let Err(e) = done {
				app.note = format!("could not scroll: {e}");
			}
		}
		if let Some(id) = app.tree_req.take()
			&& let Err(e) = session.reveal(sf, &id, RevealAt::Nearest)
		{
			app.note = format!("could not scroll the file list: {e}");
		}
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
				let key = ctrl_from_meta(key);
				// A note lasts until the next key.
				app.note.clear();
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
