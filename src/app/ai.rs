//! "Review with AI": a modal that shows what the agent will be told before it starts, with the checkout folder, the agent and
//! the template one click away; the folder field; and the state of the buttons while an agent runs.

use std::{
	cell::RefCell,
	path::{Path, PathBuf},
	time::{Duration, Instant},
};

use tern_sdk::{
	keys::Key,
	ui::{self, html},
};

use super::{AiReq, App, ComposeKind, Composer, Msg, config, review};

/// How long a button that started an agent says so before it is ready again.
const RUN_SHOWS: Duration = Duration::from_secs(20);
/// Rows of folders the picker shows at once.
const ROWS: usize = 12;

/// What the modal shows in its body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Section {
	/// The prompt, filled in.
	Preview,
	/// The template, with the filled-in prompt under it.
	Template,
	/// The checkout folder.
	Folder,
	/// Which agent to run.
	Agent,
}

/// What a key did in the folder field.
enum Pick {
	Stay,
	Back,
	Use(PathBuf),
}

struct Row {
	name: String,
	path: PathBuf,
	up:   bool,
}

pub(super) struct AiModal {
	/// What to start; none when only the settings are open.
	req:      Option<AiReq>,
	kind:     review::Kind,
	key:      String,
	settings: config::Review,
	section:  Section,
	/// The template or agent being edited.
	editor:   Option<Composer>,
	/// The folder field: a path, where the part before the last separator is the folder listed and the rest filters it.
	pick:     String,
	pick_sel: usize,
	/// The folders of the last folder listed, so typing does not read the disk.
	cache:    RefCell<Option<(String, Vec<(String, PathBuf)>)>>,
}

fn is_sep(c: char) -> bool {
	c == '/' || c == '\\'
}

/// The folder part of a path (with its separator) and the rest.
fn split_path(text: &str) -> (&str, &str) {
	match text.rfind(is_sep) {
		Some(i) => text.split_at(i + 1),
		None => ("", text),
	}
}

fn run_key(kind: review::Kind, thread: Option<u32>) -> String {
	format!("{}:{}", kind.id(), thread.unwrap_or(0))
}

impl AiModal {
	fn template(&self) -> String {
		self.settings.prompts.get(self.kind.id()).filter(|t| !t.trim().is_empty()).cloned().unwrap_or_else(|| self.kind.default_prompt().to_owned())
	}

	fn dir_ok(&self) -> bool {
		Path::new(self.settings.dir.trim()).is_dir()
	}

	/// Show a section, with its editor.
	fn enter(&mut self, section: Section) {
		self.section = section;
		self.editor = match section {
			Section::Template => Some(Composer::new(ComposeKind::ReviewPrompt(self.kind), self.template())),
			Section::Agent => Some(Composer::new(ComposeKind::ReviewAgent, self.settings.agent.clone())),
			_ => None,
		};
		if section == Section::Folder {
			let dir = self.settings.dir.trim();
			if Path::new(dir).is_dir() {
				self.pick = dir.to_owned();
			}
			self.pick_sel = 0;
		}
	}

	/// The folders to show: with nothing after the last separator, everything in that folder (and a way up); else the ones
	/// that match what is typed, best first.
	fn rows(&self) -> Vec<Row> {
		let (scope, tail) = split_path(&self.pick);
		let mut cache = self.cache.borrow_mut();
		if cache.as_ref().is_none_or(|(s, _)| s != scope) {
			let dirs = if scope.is_empty() { Vec::new() } else { review::subdirs(Path::new(scope)).into_iter().map(|d| (d.name, d.path)).collect() };
			*cache = Some((scope.to_owned(), dirs));
		}
		let dirs = cache.as_ref().map(|(_, d)| d.as_slice()).unwrap_or_default();
		let mut rows = Vec::new();
		if tail.is_empty() {
			let trimmed = scope.trim_end_matches(is_sep);
			if let Some(parent) = Path::new(trimmed).parent().filter(|p| !p.as_os_str().is_empty()) {
				rows.push(Row { name: "..".into(), path: parent.to_path_buf(), up: true });
			}
			rows.extend(dirs.iter().take(400).map(|(n, p)| Row { name: n.clone(), path: p.clone(), up: false }));
		} else {
			let q = tail.to_lowercase();
			let mut scored: Vec<(i32, &(String, PathBuf))> = dirs
				.iter()
				.filter_map(|d| {
					let name = d.0.to_lowercase();
					let bonus = if name == q { 1000 } else if name.starts_with(&q) { 50 } else { 0 };
					review::fuzzy(tail, &d.0).map(|s| (s + bonus, d))
				})
				.collect();
			scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.0.to_lowercase().cmp(&b.1.0.to_lowercase())));
			rows.extend(scored.into_iter().take(400).map(|(_, (n, p))| Row { name: n.clone(), path: p.clone(), up: false }));
		}
		rows
	}

	/// The folder Enter would use: the highlighted match when something is typed after the last separator, else the folder
	/// listed.
	fn target(&self) -> Option<PathBuf> {
		let (scope, tail) = split_path(&self.pick);
		if tail.is_empty() {
			let p = PathBuf::from(scope);
			return p.is_dir().then_some(p);
		}
		let rows = self.rows();
		let picked = rows.get(self.pick_sel.min(rows.len().saturating_sub(1))).filter(|r| !r.up).map(|r| r.path.clone());
		picked.or_else(|| Some(PathBuf::from(&self.pick)).filter(|p| p.is_dir()))
	}

	/// Make the field show the folder inside `path`.
	fn go(&mut self, path: &Path) {
		let text = review::show(path);
		self.pick = format!("{}{}", text.trim_end_matches(is_sep), std::path::MAIN_SEPARATOR);
		self.pick_sel = 0;
	}

	/// Drop the last part of the path in the field.
	fn up(&mut self) {
		let (scope, tail) = split_path(&self.pick);
		let keep = if tail.is_empty() { split_path(scope.trim_end_matches(is_sep)).0 } else { scope };
		self.pick = keep.to_owned();
		self.pick_sel = 0;
	}

	fn pick_key(&mut self, key: &Key) -> Pick {
		let rows = self.rows();
		let last = rows.len().saturating_sub(1);
		let sel = self.pick_sel.min(last);
		match key.name.as_str() {
			"escape" => return Pick::Back,
			"enter" => {
				if let Some(p) = self.target() {
					return Pick::Use(p);
				}
			},
			"down" => self.pick_sel = (sel + 1).min(last),
			"up" => self.pick_sel = sel.saturating_sub(1),
			"page_down" => self.pick_sel = (sel + ROWS).min(last),
			"page_up" => self.pick_sel = sel.saturating_sub(ROWS),
			"tab" | "right" => {
				if let Some(r) = rows.get(sel) {
					self.go(&r.path.clone());
				}
			},
			"left" => self.up(),
			"backspace" if key.ctrl || key.alt => self.up(),
			"backspace" => {
				self.pick.pop();
				self.pick_sel = 0;
			},
			"paste" => {
				let text = key.text.clone().unwrap_or_default();
				self.pick = text.trim().trim_matches('"').to_owned();
				self.pick_sel = 0;
			},
			_ => {
				if let Some(t) = key.typed().filter(|_| !key.ctrl && !key.alt) {
					self.pick.push_str(&t.to_owned());
					self.pick_sel = 0;
				}
			},
		}
		Pick::Stay
	}
}

impl App {
	/// Whether a request like this one started an agent a moment ago.
	pub(super) fn ai_busy(&self, kind: review::Kind, thread: Option<u32>) -> bool {
		self.ai_run.get(&run_key(kind, thread)).is_some_and(|t| t.elapsed() < RUN_SHOWS)
	}

	/// A button's label: its own, or that it is running, for a while after it started an agent.
	pub(super) fn ai_label(&self, idle: &str, kind: review::Kind, thread: Option<u32>) -> String {
		if self.ai_busy(kind, thread) { "\u{25cf} Running\u{2026}".to_owned() } else { idle.to_owned() }
	}

	/// A button's classes, with `busy` while it is running.
	pub(super) fn ai_class(&self, base: &str, kind: review::Kind, thread: Option<u32>) -> String {
		if self.ai_busy(kind, thread) { format!("{base} busy") } else { base.to_owned() }
	}

	/// Forget buttons whose time is up. True when one came back.
	pub(super) fn ai_expire(&mut self) -> bool {
		let before = self.ai_run.len();
		self.ai_run.retain(|_, t| t.elapsed() < RUN_SHOWS);
		self.ai_run.len() != before
	}

	/// Open the modal for `req` (or just the settings), on `section` (default: the prompt, or the folder when there is none).
	pub(super) fn open_ai(&mut self, req: Option<AiReq>, kind: review::Kind, section: Option<Section>) {
		self.rv_menu = false;
		if let Some(r) = &req {
			if self.ai_busy(r.kind, r.thread) {
				self.note = "that one is running; the button is ready again in a moment".into();
				return;
			}
			if r.kind.is_selection() && self.page.as_ref().is_none_or(|p| p.checked.is_empty()) {
				self.note = "select files in the Files tab first (Space, or the boxes)".into();
				return;
			}
			if matches!(r.kind, review::Kind::Explain | review::Kind::Respond) && self.review_vars(r.thread, None).is_some_and(|v| v.thread == "(no comment)") {
				self.note = "this pull request has no comments yet".into();
				return;
			}
		}
		let Some((key, mut settings)) = self.review_settings() else { return };
		let repo = self.page.as_ref().map(|p| p.pr.repository.name.clone()).unwrap_or_default();
		if !Path::new(settings.dir.trim()).is_dir()
			&& let Some(dir) = review::find_checkout(&repo)
		{
			settings.dir = dir;
			if let Err(e) = config::save_review(&key, settings.clone()) {
				self.note = format!("could not save the AI settings: {e}");
			}
		}
		let start = if Path::new(settings.dir.trim()).is_dir() {
			settings.dir.trim().to_owned()
		} else {
			std::env::current_dir().ok().map(|d| review::show(&d)).unwrap_or_default()
		};
		let mut m = AiModal { req, kind, key, settings, section: Section::Preview, editor: None, pick: start, pick_sel: 0, cache: RefCell::new(None) };
		let section = section.unwrap_or(if m.dir_ok() { Section::Preview } else { Section::Folder });
		m.enter(section);
		self.ai = Some(m);
	}

	fn ai_persist(&mut self) -> bool {
		let Some(m) = &self.ai else { return false };
		match config::save_review(&m.key, m.settings.clone()) {
			Ok(()) => true,
			Err(e) => {
				self.note = format!("could not save the AI settings: {e}");
				false
			},
		}
	}

	/// Keep the template or agent that was being edited.
	pub(super) fn ai_save(&mut self, text: String) {
		let Some(m) = &mut self.ai else { return };
		match m.section {
			Section::Template => {
				if text.trim().is_empty() || text.trim() == m.kind.default_prompt() {
					m.settings.prompts.remove(m.kind.id());
				} else {
					m.settings.prompts.insert(m.kind.id().to_owned(), text.trim_end().to_owned());
				}
			},
			Section::Agent => m.settings.agent = if text.trim().is_empty() { review::DEFAULT_AGENT.into() } else { text.trim().to_owned() },
			_ => return,
		}
		if self.ai_persist()
			&& let Some(m) = &mut self.ai
		{
			m.enter(Section::Preview);
			self.note = "saved".into();
		}
	}

	/// Use the folder chosen in the field.
	fn ai_use_dir(&mut self, path: &Path) {
		let Some(m) = &mut self.ai else { return };
		m.settings.dir = review::show(path);
		if self.ai_persist()
			&& let Some(m) = &mut self.ai
		{
			m.enter(Section::Preview);
			self.note = if path.join(".git").exists() { "checkout folder saved".into() } else { "saved (that folder is not a git checkout)".into() };
		}
	}

	/// Put the template back to the default.
	pub(super) fn ai_reset(&mut self) {
		if let Some(m) = &mut self.ai {
			m.settings.prompts.remove(m.kind.id());
		}
		if self.ai_persist()
			&& let Some(m) = &mut self.ai
		{
			m.enter(Section::Template);
			self.note = "back to the default prompt".into();
		}
	}

	pub(super) fn ai_section(&mut self, section: Section) {
		if let Some(m) = &mut self.ai {
			m.enter(section);
		}
	}

	pub(super) fn ai_pick_go(&mut self, path: &Path) {
		if let Some(m) = &mut self.ai {
			m.go(path);
		}
	}

	/// The folder the field would use.
	pub(super) fn ai_pick_use(&mut self) {
		if let Some(p) = self.ai.as_ref().and_then(AiModal::target) {
			self.ai_use_dir(&p);
		} else {
			self.note = "that is not a folder".into();
		}
	}

	pub(super) fn ai_edit(&mut self, e: &tern_sdk::wire::Edit) {
		if let Some(c) = self.ai.as_mut().and_then(|m| m.editor.as_mut()) {
			c.apply(e);
		}
	}

	/// Copy the prompt as shown, for an agent that is already running.
	pub(super) fn ai_copy(&mut self) {
		let Some(m) = &self.ai else { return };
		let (thread, line) = m.req.as_ref().map_or((None, None), |r| (r.thread, r.line.clone()));
		let Some(vars) = self.review_vars(thread, line.as_ref()) else { return };
		let prompt = review::render(&m.template(), &vars);
		self.copy(&prompt, "the prompt");
		self.ai = None;
	}

	/// Start the agent with the prompt as shown.
	pub(super) fn ai_start(&mut self) {
		let Some(m) = &self.ai else { return };
		let Some(req) = m.req.clone() else {
			self.ai = None;
			return;
		};
		if !m.dir_ok() {
			self.note = "pick the folder of the clone first".into();
			if let Some(m) = &mut self.ai {
				m.enter(Section::Folder);
			}
			return;
		}
		let Some(vars) = self.review_vars(req.thread, req.line.as_ref()) else { return };
		let prompt = review::render(&m.template(), &vars);
		let result = review::launch(m.settings.dir.trim(), &review::argv(&m.settings.agent, &prompt));
		match result {
			Ok(msg) => {
				self.ai_run.insert(run_key(req.kind, req.thread), Instant::now());
				self.note = msg;
				self.ai = None;
			},
			Err(e) => self.note = format!("AI request failed: {e}"),
		}
	}

	/// A key while the modal is open.
	pub(super) fn ai_key(&mut self, key: &Key) {
		enum Act {
			None,
			Close,
			Start,
			Copy,
			Enter(Section),
			Save(String),
			Use(PathBuf),
		}
		let act = {
			let Some(m) = &mut self.ai else { return };
			match m.section {
				Section::Folder => match m.pick_key(key) {
					Pick::Stay => Act::None,
					Pick::Back => Act::Enter(Section::Preview),
					Pick::Use(p) => Act::Use(p),
				},
				Section::Template | Section::Agent => {
					let multiline = m.section == Section::Template;
					match m.editor.as_mut() {
						None => Act::None,
						Some(_) if key.is("escape") => Act::Enter(Section::Preview),
						Some(c) if key.name == "enter" && multiline && (key.shift || key.alt) => {
							c.insert("\n");
							Act::None
						},
						Some(c) if key.name == "enter" => Act::Save(c.draft.clone()),
						Some(c) => {
							c.key(key);
							Act::None
						},
					}
				},
				Section::Preview => match key.name.as_str() {
					"escape" | "q" => Act::Close,
					"enter" => Act::Start,
					"c" => Act::Copy,
					"e" | "t" => Act::Enter(Section::Template),
					"f" => Act::Enter(Section::Folder),
					"a" => Act::Enter(Section::Agent),
					_ => Act::None,
				},
			}
		};
		match act {
			Act::None => {},
			Act::Close => self.ai = None,
			Act::Start => self.ai_start(),
			Act::Copy => self.ai_copy(),
			Act::Enter(s) => self.ai_section(s),
			Act::Save(t) => self.ai_save(t),
			Act::Use(p) => self.ai_use_dir(&p),
		}
	}

	/// The editor to focus while the modal is open.
	pub(super) fn ai_focus(&self) -> Option<String> {
		self.ai.as_ref()?.editor.as_ref().map(|_| "main.pbody.aimodal.aied".to_owned())
	}

	/// The modal: what the agent will be told, where it will run, which agent, and which template, with buttons for each.
	pub(super) fn ai_modal(&self) -> Option<html::El<Msg>> {
		let m = self.ai.as_ref()?;
		let p = self.page.as_ref()?;
		let btn = |label: &str, pri: bool, msg: Msg| html::button(label.to_owned()).class(if pri { "azbtn pri" } else { "azbtn" }).on_click(msg);
		let repo = format!("{} / {}", p.pr.repository.project.name, p.pr.repository.name);
		let title = if m.req.is_some() { m.kind.title().to_owned() } else { "AI settings".to_owned() };
		let draft = match (&m.editor, m.section) {
			(Some(c), Section::Template) => c.draft.clone(),
			_ => m.template(),
		};
		let vars = self.review_vars(m.req.as_ref().and_then(|r| r.thread), m.req.as_ref().and_then(|r| r.line.as_ref()));
		let preview = vars.map(|v| review::render(&draft, &v)).unwrap_or_default();

		let setting = |label: &str, value: String, ok: bool, change: &str, to: Section| {
			html::div()
				.class("ais")
				.child(html::span(label.to_owned()).class("dim"))
				.child(html::span(value).class(if ok { "aiv" } else { "aiv bad" }))
				.child(html::span(change.to_owned()).class("ail").on_click(Msg::AiSection(to)))
		};
		let summary = html::div()
			.class("aisum")
			.child(setting("Folder", if m.dir_ok() { m.settings.dir.trim().to_owned() } else { "not set".to_owned() }, m.dir_ok(), "Change", Section::Folder))
			.child(setting("Agent", m.settings.agent.clone(), true, "Change", Section::Agent))
			.child(setting(
				"Template",
				if m.settings.prompts.contains_key(m.kind.id()) { "custom for this repo".to_owned() } else { "default".to_owned() },
				true,
				"Edit",
				Section::Template,
			));

		let editor = |max: u32| {
			m.editor.as_ref().map(|c| {
				ui::editor()
					.key("aied")
					.text(c.draft.clone())
					.cursor(c.utf16_cursor())
					.placeholder("")
					.sendable(true)
					.max_lines(max)
					.on_send(|t| Msg::AiSend(t.to_owned()))
					.on_edit(|e| Msg::AiEdit(e.clone()))
					.on_focus(Msg::Focus)
			})
		};
		let mut body = html::div().class("aibody");
		let mut foot = html::div().class("aif");
		match m.section {
			Section::Preview => {
				body = body.child(html::div().class("apt").child(html::span(if m.req.is_some() { "The agent will be told:" } else { "A prompt, filled in for this pull request:" })));
				body = body.child(html::div().class("pvx big").child(html::span(preview)));
				foot = foot.child(html::span("Enter starts \u{b7} C copies the prompt \u{b7} E template \u{b7} F folder \u{b7} A agent \u{b7} Esc closes").class("dim")).child(btn("Cancel", false, Msg::AiClose));
				foot = if m.req.is_some() { foot.child(btn("Copy prompt", false, Msg::AiCopy)).child(btn("Start agent", true, Msg::AiStart)) } else { foot.child(btn("Done", true, Msg::AiClose)) };
			},
			Section::Template => {
				body = body.child(html::div().class("apt").child(html::span(format!("The {} template for {repo}", m.kind.label()))));
				body = body.child(html::div().class("dim aph").child(html::span(format!("Variables: {}", review::NAMES))));
				body = body.children(editor(14));
				body = body.child(html::div().class("apt").child(html::span("Result")));
				body = body.child(html::div().class("pvx big").child(html::span(preview)));
				foot = foot
					.child(html::span("Enter saves \u{b7} Shift+Enter new line \u{b7} Esc goes back").class("dim"))
					.child(btn("Reset to default", false, Msg::AiReset))
					.child(btn("Back", false, Msg::AiSection(Section::Preview)))
					.child(btn("Save template", true, Msg::AiSave));
			},
			Section::Agent => {
				body = body.child(html::div().class("apt").child(html::span(format!("The agent for {repo}"))));
				body = body.child(html::div().class("dim aph").child(html::span("claude, codex, or a command line; {prompt} marks where the prompt goes (else it is the last argument)")));
				body = body.children(editor(2));
				foot = foot
					.child(html::span("Enter saves").class("dim"))
					.child(btn("Back", false, Msg::AiSection(Section::Preview)))
					.child(btn("Save agent", true, Msg::AiSave));
			},
			Section::Folder => {
				let rows = m.rows();
				let cur = m.pick_sel.min(rows.len().saturating_sub(1));
				let first = cur.saturating_sub(ROWS - 1).min(rows.len().saturating_sub(ROWS));
				let (scope, tail) = split_path(&m.pick);
				let mut list = html::div().class("apl");
				for (i, r) in rows.iter().enumerate().skip(first).take(ROWS) {
					let hint = if r.up {
						"up".to_owned()
					} else if r.path == Path::new(m.settings.dir.trim()) {
						"current".to_owned()
					} else if r.path.join(".git").exists() {
						"git".to_owned()
					} else {
						String::new()
					};
					list = list.child(
						html::div()
							.class(if i == cur { "row cur" } else { "row" })
							.on_click(Msg::AiPickGo(r.path.clone()))
							.child(html::span(if r.up { "\u{2191}" } else { "\u{25b8}" }).class("id"))
							.child(html::span(r.name.clone()).class("t"))
							.child(html::span(hint).class("pj")),
					);
				}
				if rows.is_empty() {
					list = list.child(html::div().class("row").child(html::span(if Path::new(scope).is_dir() { "No folder matches." } else { "No such folder." }).class("dim")));
				}
				let target = m.target();
				let name = target.as_ref().and_then(|t| t.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
				body = body.child(html::div().class("apt").child(html::span(format!("Where is the {} clone?", p.pr.repository.name))));
				body = body.child(html::div().class("dim aph").child(html::span("Type a path. Everything up to the last \\ or / is the folder listed; what follows filters it.")));
				body = body.child(html::div().class("aps").child(html::span(format!("{}\u{2581}", m.pick))));
				body = body.child(list);
				let _ = tail;
				foot = foot
					.child(html::span("\u{2191}\u{2193} choose \u{b7} Tab or click opens a folder \u{b7} \u{2190} goes up \u{b7} Enter uses it").class("dim"))
					.child(btn("Back", false, Msg::AiSection(Section::Preview)))
					.child(if target.is_some() { btn(&format!("Use {name}"), true, Msg::AiPickUse) } else { html::button("Use folder").class("azbtn off") });
			},
		}
		Some(
			html::div()
				.key("aimodal")
				.class("aimodal")
				.child(
					html::div()
						.class("aih")
						.child(html::div().class("ait").child(html::span(title)).child(html::span(format!("!{}  {}", p.pr.pull_request_id, p.pr.title)).class("dim")))
						.child(html::span("\u{2715}").class("aix").on_click(Msg::AiClose)),
				)
				.child(summary)
				.child(body)
				.child(foot),
		)
	}
}

impl AiModal {
	/// What the template or agent editor holds now.
	pub(super) fn editor_text(&self) -> Option<String> {
		self.editor.as_ref().map(|c| c.draft.clone())
	}
}
