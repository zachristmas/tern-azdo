//! The Azure DevOps REST calls tern-azdo makes, and the slices of their
//! responses it reads. Everything goes through ids, so project and repo names
//! with spaces never need escaping in API paths.

use std::{collections::HashMap, time::Duration};

use anyhow::{Context, bail};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::auth::Auth;

const API: &str = "api-version=7.1";
/// Blobs bigger than this are not diffed.
const MAX_BLOB: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
	pub display_name: String,
	pub id:           String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Project {
	pub id:   String,
	pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Repo {
	pub id:      String,
	pub name:    String,
	pub project: Project,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reviewer {
	pub display_name: String,
	pub id:           String,
	#[serde(default)]
	pub vote:         i32,
	#[serde(default)]
	pub is_required:  bool,
	#[serde(default)]
	pub is_container: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pr {
	pub pull_request_id: u32,
	pub title:           String,
	#[serde(default)]
	pub description:     Option<String>,
	pub created_by:      Identity,
	pub creation_date:   String,
	#[serde(default)]
	pub is_draft:        bool,
	pub source_ref_name: String,
	pub target_ref_name: String,
	pub repository:      Repo,
	#[serde(default)]
	pub reviewers:       Vec<Reviewer>,
	#[serde(default)]
	pub merge_status:    Option<String>,
}

impl Pr {
	pub fn source(&self) -> &str {
		branch(&self.source_ref_name)
	}

	pub fn target(&self) -> &str {
		branch(&self.target_ref_name)
	}

	/// The vote of reviewer `id`, if they are one.
	pub fn vote_of(&self, id: &str) -> Option<i32> {
		self.reviewers.iter().find(|r| r.id == id).map(|r| r.vote)
	}
}

/// `refs/heads/feature/x` as `feature/x`.
pub fn branch(r: &str) -> &str {
	r.strip_prefix("refs/heads/").unwrap_or(r)
}

#[derive(Clone, Debug, Deserialize)]
pub struct Iteration {
	pub id: u32,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeItem {
	#[serde(default)]
	pub object_id:          Option<String>,
	#[serde(default)]
	pub original_object_id: Option<String>,
	#[serde(default)]
	pub path:               Option<String>,
	#[serde(default)]
	pub is_folder:          bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
	#[serde(default)]
	pub item:          ChangeItem,
	#[serde(default)]
	pub change_type:   String,
	#[serde(default)]
	pub original_path: Option<String>,
}

impl Change {
	pub fn path(&self) -> &str {
		self.item.path.as_deref().or(self.original_path.as_deref()).unwrap_or("")
	}

	pub fn is(&self, kind: &str) -> bool {
		self.change_type.split(',').any(|k| k.trim() == kind)
	}

	/// One letter for the file list: A, D, R or M.
	pub fn letter(&self) -> &'static str {
		if self.is("add") {
			"A"
		} else if self.is("delete") {
			"D"
		} else if self.is("rename") {
			"R"
		} else {
			"M"
		}
	}

	/// The base and head blob ids (None where the file does not exist on that side).
	pub fn blobs(&self) -> (Option<&str>, Option<&str>) {
		let obj = self.item.object_id.as_deref();
		let orig = self.item.original_object_id.as_deref();
		if self.is("add") {
			(None, obj)
		} else if self.is("delete") {
			(orig.or(obj), None)
		} else {
			(orig, obj)
		}
	}
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pos {
	pub line: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadContext {
	#[serde(default)]
	pub file_path:        Option<String>,
	#[serde(default)]
	pub right_file_start: Option<Pos>,
	#[serde(default)]
	pub left_file_start:  Option<Pos>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
	#[serde(default)]
	pub author:       Identity,
	#[serde(default)]
	pub content:      Option<String>,
	#[serde(default)]
	pub comment_type: Option<String>,
	#[serde(default)]
	pub is_deleted:   bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
	pub id:             u32,
	#[serde(default)]
	pub status:         Option<String>,
	#[serde(default)]
	pub thread_context: Option<ThreadContext>,
	#[serde(default)]
	pub comments:       Vec<Comment>,
	#[serde(default)]
	pub is_deleted:     bool,
}

impl Thread {
	/// Comments people wrote (no system notes, nothing deleted).
	pub fn human(&self) -> impl Iterator<Item = &Comment> {
		self.comments.iter().filter(|c| !c.is_deleted && c.comment_type.as_deref() != Some("system"))
	}

	/// A thread worth showing: not deleted, with at least one human comment.
	pub fn shown(&self) -> bool {
		!self.is_deleted && self.human().next().is_some()
	}

	pub fn file(&self) -> Option<&str> {
		self.thread_context.as_ref()?.file_path.as_deref()
	}

	/// The line it is anchored to: the new side's, else the old side's.
	pub fn line(&self) -> Option<u32> {
		let c = self.thread_context.as_ref()?;
		c.right_file_start.as_ref().or(c.left_file_start.as_ref()).map(|p| p.line)
	}

	pub fn is_open(&self) -> bool {
		matches!(self.status.as_deref(), Some("active" | "pending") | None)
	}
}

#[derive(Clone, Debug)]
pub struct WorkItem {
	pub id:    u32,
	pub title: String,
	pub state: String,
	pub kind:  String,
}

/// Everything the PR page shows beyond the list row.
#[derive(Clone, Debug)]
pub struct Detail {
	pub changes:    Vec<Change>,
	pub threads:    Vec<Thread>,
	pub work_items: Vec<WorkItem>,
}

#[derive(Deserialize)]
struct List<T> {
	value: Vec<T>,
}

pub struct Client {
	pub org: String,
	auth:    Auth,
	agent:   ureq::Agent,
}

impl Client {
	pub fn new(org: &str) -> Self {
		let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout(Duration::from_secs(60)).build();
		Self { org: org.to_owned(), auth: Auth::new(org), agent }
	}

	fn call(&self, method: &str, url: &str, body: Option<&Value>) -> anyhow::Result<ureq::Response> {
		for attempt in 0..2 {
			let req = self.agent.request(method, url).set("Authorization", &self.auth.header()?).set("Accept", "application/json");
			let res = match body {
				Some(b) => req.send_json(b.clone()),
				None => req.call(),
			};
			match res {
				Ok(r) => return Ok(r),
				Err(ureq::Error::Status(401, _)) if attempt == 0 => self.auth.invalidate(),
				Err(ureq::Error::Status(code, r)) => {
					let text = r.into_string().unwrap_or_default();
					let msg = serde_json::from_str::<Value>(&text)
						.ok()
						.and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_owned))
						.unwrap_or_else(|| text.chars().take(200).collect());
					if code == 401 {
						bail!("Azure DevOps refused the sign-in (401). Run `az login`, or check the saved PAT. {msg}");
					}
					bail!("{method} failed with {code}: {msg}");
				},
				Err(e) => return Err(e).with_context(|| format!("{method} {}", url.split('?').next().unwrap_or(url))),
			}
		}
		bail!("Azure DevOps refused the sign-in (401). Run `az login`, or check the saved PAT.")
	}

	fn get<T: DeserializeOwned>(&self, url: &str) -> anyhow::Result<T> {
		self.call("GET", url, None)?.into_json().context("reading the response")
	}

	fn repo_url(&self, pr: &Pr) -> String {
		format!("{}/{}/_apis/git/repositories/{}", self.org, pr.repository.project.id, pr.repository.id)
	}

	fn pr_url(&self, pr: &Pr) -> String {
		format!("{}/pullRequests/{}", self.repo_url(pr), pr.pull_request_id)
	}

	/// The signed-in user.
	pub fn me(&self) -> anyhow::Result<Identity> {
		let v: Value = self.get(&format!("{}/_apis/connectionData", self.org))?;
		let u = v.get("authenticatedUser").context("no authenticatedUser in connectionData")?;
		Ok(Identity {
			display_name: u.get("providerDisplayName").and_then(Value::as_str).unwrap_or_default().to_owned(),
			id:           u.get("id").and_then(Value::as_str).unwrap_or_default().to_owned(),
		})
	}

	/// Every active PR in the organization, every project and repo.
	pub fn active_prs(&self) -> anyhow::Result<Vec<Pr>> {
		const PAGE: usize = 500;
		let mut all = Vec::new();
		loop {
			let url = format!("{}/_apis/git/pullrequests?searchCriteria.status=active&$top={PAGE}&$skip={}&{API}", self.org, all.len());
			let page: List<Pr> = self.get(&url)?;
			let n = page.value.len();
			all.extend(page.value);
			if n < PAGE {
				break;
			}
		}
		Ok(all)
	}

	/// One PR, fresh (reviewers and votes change).
	pub fn pr(&self, pr: &Pr) -> anyhow::Result<Pr> {
		self.get(&format!("{}?{API}", self.pr_url(pr)))
	}

	pub fn detail(&self, pr: &Pr) -> anyhow::Result<Detail> {
		let base = self.pr_url(pr);
		let iterations: List<Iteration> = self.get(&format!("{base}/iterations?{API}"))?;
		let iteration = iterations.value.into_iter().max_by_key(|i| i.id).context("the pull request has no iterations")?;
		let mut changes = Vec::new();
		loop {
			let url = format!("{base}/iterations/{}/changes?$compareTo=0&$top=2000&$skip={}&{API}", iteration.id, changes.len());
			let page: Value = self.get(&url)?;
			let entries: Vec<Change> = serde_json::from_value(page.get("changeEntries").cloned().unwrap_or(json!([])))?;
			let n = entries.len();
			changes.extend(entries.into_iter().filter(|c| !c.item.is_folder));
			if page.get("nextSkip").and_then(Value::as_u64).unwrap_or(0) == 0 || n == 0 {
				break;
			}
		}
		changes.sort_by_key(|c| c.path().to_lowercase());
		let threads: List<Thread> = self.get(&format!("{base}/threads?{API}"))?;
		let work_items = self.work_items(pr).unwrap_or_default();
		Ok(Detail { changes, threads: threads.value, work_items })
	}

	fn work_items(&self, pr: &Pr) -> anyhow::Result<Vec<WorkItem>> {
		let refs: Value = self.get(&format!("{}/workitems?{API}", self.pr_url(pr)))?;
		let ids: Vec<String> = refs
			.get("value")
			.and_then(Value::as_array)
			.map(|a| a.iter().filter_map(|w| w.get("id").and_then(|i| i.as_str().map(str::to_owned).or_else(|| i.as_u64().map(|n| n.to_string())))).collect())
			.unwrap_or_default();
		if ids.is_empty() {
			return Ok(Vec::new());
		}
		let url = format!(
			"{}/_apis/wit/workitems?ids={}&fields=System.Title,System.State,System.WorkItemType&errorPolicy=omit&{API}",
			self.org,
			ids.join(",")
		);
		let items: Value = self.get(&url)?;
		let field = |w: &Value, f: &str| w.get("fields").and_then(|x| x.get(f)).and_then(Value::as_str).unwrap_or_default().to_owned();
		Ok(items
			.get("value")
			.and_then(Value::as_array)
			.map(|a| {
				a.iter()
					.filter(|w| !w.is_null())
					.map(|w| WorkItem {
						id:    w.get("id").and_then(Value::as_u64).unwrap_or_default() as u32,
						title: field(w, "System.Title"),
						state: field(w, "System.State"),
						kind:  field(w, "System.WorkItemType"),
					})
					.collect()
			})
			.unwrap_or_default())
	}

	/// A file's text by blob id; `None` when it is binary or too big to diff.
	pub fn blob(&self, pr: &Pr, object_id: &str) -> anyhow::Result<Option<String>> {
		let url = format!("{}/blobs/{object_id}?$format=octetstream&{API}", self.repo_url(pr));
		let res = self.call("GET", &url, None)?;
		let mut bytes = Vec::new();
		std::io::Read::read_to_end(&mut std::io::Read::take(res.into_reader(), MAX_BLOB as u64 + 1), &mut bytes)?;
		if bytes.len() > MAX_BLOB || bytes.iter().take(8000).any(|b| *b == 0) {
			return Ok(None);
		}
		let text = String::from_utf8_lossy(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes)).replace("\r\n", "\n");
		Ok(Some(text))
	}

	/// Sets the signed-in user's vote: 10 approve, 5 with suggestions, 0 none, -5 wait, -10 reject.
	pub fn vote(&self, pr: &Pr, me: &str, vote: i32) -> anyhow::Result<()> {
		let url = format!("{}/reviewers/{me}?{API}", self.pr_url(pr));
		self.call("PUT", &url, Some(&json!({ "vote": vote })))?;
		Ok(())
	}

	/// A new top-level comment thread.
	pub fn comment(&self, pr: &Pr, text: &str) -> anyhow::Result<()> {
		let body = json!({ "comments": [{ "parentCommentId": 0, "content": text, "commentType": 1 }], "status": 1 });
		self.call("POST", &format!("{}/threads?{API}", self.pr_url(pr)), Some(&body))?;
		Ok(())
	}

	/// A reply in an existing thread.
	pub fn reply(&self, pr: &Pr, thread: u32, text: &str) -> anyhow::Result<()> {
		let body = json!({ "parentCommentId": 1, "content": text, "commentType": 1 });
		self.call("POST", &format!("{}/threads/{thread}/comments?{API}", self.pr_url(pr)), Some(&body))?;
		Ok(())
	}

	/// The PR's page in the browser.
	pub fn web_url(&self, pr: &Pr) -> String {
		let enc = |s: &str| urlencoding::encode(s).into_owned();
		format!("{}/{}/_git/{}/pullrequest/{}", self.org, enc(&pr.repository.project.name), enc(&pr.repository.name), pr.pull_request_id)
	}
}

/// How a vote reads.
pub fn vote_label(vote: i32) -> &'static str {
	match vote {
		10 => "Approved",
		5 => "Approved with suggestions",
		-5 => "Waiting for author",
		-10 => "Rejected",
		_ => "No vote",
	}
}

/// Counts of approvals, waits and rejections, ignoring groups that have not voted.
pub fn vote_counts(pr: &Pr) -> HashMap<i32, usize> {
	let mut m = HashMap::new();
	for r in &pr.reviewers {
		if r.vote != 0 {
			*m.entry(r.vote).or_default() += 1;
		}
	}
	m
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_change_entry_picks_its_blobs_by_kind() {
		let entry = |kind: &str| -> Change {
			serde_json::from_value(json!({
				"item": { "objectId": "NEW", "originalObjectId": "OLD", "path": "/a.cs" },
				"changeType": kind
			}))
			.unwrap()
		};
		assert_eq!(entry("edit").blobs(), (Some("OLD"), Some("NEW")));
		assert_eq!(entry("add").blobs(), (None, Some("NEW")));
		assert_eq!(entry("delete").blobs(), (Some("OLD"), None));
		assert_eq!(entry("edit, rename").letter(), "R");
		assert_eq!(entry("add").letter(), "A");
	}

	#[test]
	fn system_comments_do_not_make_a_thread_shown() {
		let t: Thread = serde_json::from_value(json!({
			"id": 1,
			"comments": [{ "commentType": "system", "content": "voted 10" }]
		}))
		.unwrap();
		assert!(!t.shown());
	}
}
