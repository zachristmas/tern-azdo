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

/// One row of a drawn diff side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell<'a> {
	Line(&'a Line),
	/// Blank, across from lines the other side does not have.
	Nil,
	/// Between hunks.
	Gap,
}

/// The rows of `lines` for one column (`split` false), or for the two sides of a side-by-side
/// view: unchanged lines on both, each run of removed lines beside the added lines after it.
pub fn cells(lines: &[Line], split: bool) -> (Vec<Cell<'_>>, Vec<Cell<'_>>) {
	let (mut left, mut right) = (Vec::new(), Vec::new());
	let mut i = 0;
	while i < lines.len() {
		if i > 0 && lines[i].hunk != lines[i - 1].hunk {
			left.push(Cell::Gap);
			if split {
				right.push(Cell::Gap);
			}
		}
		let l = &lines[i];
		if !split || l.tag == ' ' {
			left.push(Cell::Line(l));
			if split {
				right.push(Cell::Line(l));
			}
			i += 1;
			continue;
		}
		let hunk = l.hunk;
		let run = |from: usize, tag: char| lines[from..].iter().take_while(|x| x.tag == tag && x.hunk == hunk).count();
		let dels = run(i, '-');
		let adds = run(i + dels, '+');
		for k in 0..dels.max(adds) {
			left.push(if k < dels { Cell::Line(&lines[i + k]) } else { Cell::Nil });
			right.push(if k < adds { Cell::Line(&lines[i + dels + k]) } else { Cell::Nil });
		}
		i += dels + adds;
	}
	(left, right)
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

	/// Where each block of changed lines starts (the first line of every run of added or removed lines).
	pub fn changes(&self) -> Vec<usize> {
		(0..self.lines.len()).filter(|&i| self.lines[i].tag != ' ' && (i == 0 || self.lines[i - 1].tag == ' ')).collect()
	}

	/// Where the view starts when it jumps to the change at line `start`: `lead` lines above it, unless that would cut
	/// through another block of changes.
	pub fn jump_to(&self, start: usize, lead: usize) -> usize {
		let t = start.saturating_sub(lead);
		let safe = t == 0
			|| self.lines[t].tag == ' '
			|| self.lines[t - 1].tag == ' '
			|| (self.lines[t - 1].tag == '+' && self.lines[t].tag == '-');
		if safe { t } else { start }
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
	fn changes_start_each_block_and_jumps_keep_a_lead() {
		let old = (1..=20).map(|n| format!("l{n}\n")).collect::<String>();
		let new = old.replace("l10\n", "X\n").replace("l12\n", "Y\n").replace("l18\n", "Z\n");
		let d = build(&old, &new, usize::MAX / 4);
		let starts = d.changes();
		// l10 -> X is one block; l12 -> Y another (l11 between); l18 -> Z a third.
		assert_eq!(starts, [9, 12, 19]);
		assert_eq!(d.jump_to(9, 5), 4);
		assert_eq!(d.jump_to(12, 5), 7);
		assert_eq!(d.jump_to(2, 5), 0);
	}

	#[test]
	fn side_by_side_pairs_removed_with_added_lines() {
		let d = build("a\nb\nc\nd\n", "a\nB\nd\n", 1);
		let (l, r) = cells(&d.lines, true);
		let show = |c: &[Cell<'_>]| {
			c.iter()
				.map(|c| match c {
					Cell::Line(l) => format!("{}{}", l.tag, l.text),
					Cell::Nil => "~".into(),
					Cell::Gap => "...".into(),
				})
				.collect::<Vec<_>>()
		};
		assert_eq!(show(&l), [" a", "-b", "-c", " d"]);
		assert_eq!(show(&r), [" a", "+B", "~", " d"]);
		let (one, none) = cells(&d.lines, false);
		assert_eq!(one.len(), d.len());
		assert!(none.is_empty());
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
