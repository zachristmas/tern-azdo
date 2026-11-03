//! A file's diff as numbered lines, so the view can page through it and jump
//! to a comment's line, and hand Tern only the hunks that fit on screen.

use similar::{ChangeTag, TextDiff};
use tern_sdk::ui::Hunk;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
	/// `' '`, `'-'` or `'+'`.
	pub tag:  char,
	pub text: String,
	/// Line number on the old side (none for an added line).
	pub old:  Option<u64>,
	/// Line number on the new side (none for a removed line).
	pub new:  Option<u64>,
	/// Which hunk of the full diff the line belongs to.
	pub hunk: usize,
}

#[derive(Clone, Debug, Default)]
pub struct FileDiff {
	pub lines: Vec<Line>,
}

/// The diff of `old` to `new` with `context` lines around each change.
pub fn build(old: &str, new: &str, context: usize) -> FileDiff {
	let d = TextDiff::from_lines(old, new);
	let mut lines = Vec::new();
	for (hunk, group) in d.grouped_ops(context).iter().enumerate() {
		for op in group {
			for change in d.iter_changes(op) {
				let tag = match change.tag() {
					ChangeTag::Equal => ' ',
					ChangeTag::Delete => '-',
					ChangeTag::Insert => '+',
				};
				lines.push(Line {
					tag,
					text: change.value().trim_end_matches(['\n', '\r']).to_owned(),
					old: change.old_index().map(|i| i as u64 + 1),
					new: change.new_index().map(|i| i as u64 + 1),
					hunk,
				});
			}
		}
	}
	FileDiff { lines }
}

impl FileDiff {
	pub fn len(&self) -> usize {
		self.lines.len()
	}

	pub fn is_empty(&self) -> bool {
		self.lines.is_empty()
	}

	pub fn added(&self) -> usize {
		self.lines.iter().filter(|l| l.tag == '+').count()
	}

	pub fn removed(&self) -> usize {
		self.lines.iter().filter(|l| l.tag == '-').count()
	}

	/// Where a hunk that starts at line `i` begins on each side.
	fn starts(&self, i: usize) -> (u64, u64) {
		let hunk = self.lines[i].hunk;
		let same = |l: &&Line| l.hunk == hunk;
		let side = |pick: fn(&Line) -> Option<u64>| {
			// This line's number, else the next one in the hunk, else one past the previous one.
			self.lines[i..]
				.iter()
				.take_while(same)
				.find_map(pick)
				.or_else(|| self.lines[..i].iter().rev().take_while(same).find_map(pick).map(|n| n + 1))
				.unwrap_or(0)
		};
		(side(|l| l.old), side(|l| l.new))
	}

	/// Where a comment on lines `line..=end` sits in the diff: after the range's last line on its side
	/// (the old side for `left`), else before the first line past the range, else at the end.
	pub fn anchor(&self, line: u64, end: u64, left: bool) -> usize {
		let side = |l: &Line| if left { l.old.filter(|_| l.tag != '+') } else { l.new.filter(|_| l.tag != '-') };
		if let Some(i) = self.lines.iter().rposition(|l| side(l) == Some(end)) {
			return i + 1;
		}
		self.lines.iter().position(|l| side(l).is_some_and(|n| n >= line)).unwrap_or(self.lines.len())
	}

	/// Lines `offset..offset + height` as hunks Tern can draw.
	pub fn window(&self, offset: usize, height: usize) -> Vec<Hunk> {
		let end = (offset + height).min(self.lines.len());
		let mut out: Vec<Hunk> = Vec::new();
		let mut current: Option<usize> = None;
		for i in offset.min(end)..end {
			let l = &self.lines[i];
			if current != Some(l.hunk) {
				let (o, n) = self.starts(i);
				out.push(Hunk { old_start: Some(o), new_start: Some(n), lines: Vec::new() });
				current = Some(l.hunk);
			}
			if let Some(h) = out.last_mut() {
				h.lines.push(format!("{}{}", l.tag, l.text));
			}
		}
		out
	}

}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_window_keeps_line_numbers_right() {
		let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
		let new = "a\nb\nc\nD\ne\nf\ng\nh\nI\n";
		let d = build(old, new, 1);
		assert_eq!(d.added(), 2);
		assert_eq!(d.removed(), 1);
		// Two hunks: around d -> D, and the added I at the end.
		let all = d.window(0, 100);
		assert_eq!(all.len(), 2);
		assert_eq!((all[0].old_start, all[0].new_start), (Some(3), Some(3)));
		assert_eq!(all[0].lines, vec![" c", "-d", "+D", " e"]);
		assert_eq!((all[1].old_start, all[1].new_start), (Some(8), Some(8)));
		// A window starting on the added D line begins at new line 4, old line 5 (after the removed d).
		let part = d.window(2, 2);
		assert_eq!(part[0].lines, vec!["+D", " e"]);
		assert_eq!((part[0].old_start, part[0].new_start), (Some(5), Some(4)));
	}

	#[test]
	fn a_comment_sits_after_the_lines_it_is_about() {
		let d = build("a
b
c
d
e
", "a
b
C
d
e
", 1);
		// Lines: " b", "-c", "+C", " d" (new 2, -, 3, 4).
		assert_eq!(d.anchor(3, 3, false), 3);
		assert_eq!(d.anchor(3, 3, true), 2);
		// Line 1 is outside the hunk: before the first line past it.
		assert_eq!(d.anchor(1, 1, false), 0);
		assert_eq!(d.anchor(9, 9, false), 4);
	}

	#[test]
	fn a_new_file_is_all_additions_from_line_one() {
		let d = build("", "x\ny\n", 3);
		let w = d.window(0, 10);
		assert_eq!(w[0].lines, vec!["+x", "+y"]);
		assert_eq!(w[0].new_start, Some(1));
	}
}
