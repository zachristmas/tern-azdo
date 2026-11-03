//! Network work off the UI thread: each job runs on its own thread and
//! posts one reply, which the UI picks up while it polls.

use std::sync::{Arc, mpsc::Sender};

use crate::{
	api::{Client, Detail, Identity, Pr},
	demo,
};

pub enum Job {
	/// Who am I, and every active PR in the org.
	Load,
	/// The PR again (fresh votes) and its files, threads and work items.
	Detail(Pr),
	/// Both sides of one file.
	File { pr: Pr, path: String, base: Option<String>, head: Option<String> },
	Vote { pr: Pr, me: String, vote: i32 },
	/// A new thread when `thread` is none, else a reply.
	Comment { pr: Pr, thread: Option<u32>, text: String },
}

/// A file's two sides; `None` for a side that is binary or too big.
pub type Sides = (Option<String>, Option<String>);

pub enum Reply {
	Loaded(Result<(Identity, Vec<Pr>), String>),
	Detail(u32, Result<Box<(Pr, Detail)>, String>),
	File(u32, String, Result<Sides, String>),
	/// A write finished: what it was, for the status line.
	Wrote(u32, &'static str, Result<(), String>),
}

fn text(e: anyhow::Error) -> String {
	format!("{e:#}")
}

/// The demo's answers, a beat late so loading states still show.
fn demo(job: Job) -> Reply {
	std::thread::sleep(std::time::Duration::from_millis(250));
	match job {
		Job::Load => Reply::Loaded(Ok((demo::me(), demo::prs()))),
		Job::Detail(pr) => Reply::Detail(pr.pull_request_id, Ok(Box::new((pr.clone(), demo::detail(&pr))))),
		Job::File { pr, path, base, head } => {
			let side = |b: Option<String>| b.map_or(Some(String::new()), |b| demo::blob(&b));
			Reply::File(pr.pull_request_id, path, Ok((side(base), side(head))))
		},
		Job::Vote { pr, .. } => Reply::Wrote(pr.pull_request_id, "vote (demo, not sent)", Ok(())),
		Job::Comment { pr, .. } => Reply::Wrote(pr.pull_request_id, "comment (demo, not sent)", Ok(())),
	}
}

pub fn start(client: Arc<Client>, job: Job, tx: Sender<Reply>) {
	std::thread::spawn(move || {
		if client.demo {
			let _ = tx.send(demo(job));
			return;
		}
		let reply = match job {
			Job::Load => Reply::Loaded((|| Ok::<_, anyhow::Error>((client.me()?, client.active_prs()?)))().map_err(text)),
			Job::Detail(pr) => {
				let id = pr.pull_request_id;
				Reply::Detail(id, (|| Ok::<_, anyhow::Error>(Box::new((client.pr(&pr)?, client.detail(&pr)?))))().map_err(text))
			},
			Job::File { pr, path, base, head } => {
				let get = |blob: Option<String>| -> anyhow::Result<Option<String>> {
					match blob {
						Some(b) => client.blob(&pr, &b),
						None => Ok(Some(String::new())),
					}
				};
				Reply::File(pr.pull_request_id, path, (|| Ok::<_, anyhow::Error>((get(base)?, get(head)?)))().map_err(text))
			},
			Job::Vote { pr, me, vote } => Reply::Wrote(pr.pull_request_id, "vote", client.vote(&pr, &me, vote).map_err(text)),
			Job::Comment { pr, thread, text: body } => {
				let r = match thread {
					Some(t) => client.reply(&pr, t, &body),
					None => client.comment(&pr, &body),
				};
				Reply::Wrote(pr.pull_request_id, if thread.is_some() { "reply" } else { "comment" }, r.map_err(text))
			},
		};
		let _ = tx.send(reply);
	});
}
