//! The Files tab's tree: changed paths as folders and files, folders first,
//! with single-child folder chains joined into one row the way Azure DevOps
//! shows them.

use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
	/// A folder: its full path (the key for folding), what it shows, how deep it sits.
	Dir { path: String, label: String, depth: usize, open: bool },
	/// A file: its index in the change list, its name, how deep it sits.
	File { index: usize, name: String, depth: usize },
}

#[derive(Default)]
struct Dir {
	dirs:  BTreeMap<String, Dir>,
	files: Vec<(String, usize)>,
}

fn build(paths: &[&str]) -> Dir {
	let mut root = Dir::default();
	for (index, path) in paths.iter().enumerate() {
		let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
		let Some((name, dirs)) = parts.split_last() else { continue };
		let mut at = &mut root;
		for d in dirs {
			at = at.dirs.entry((*d).to_owned()).or_default();
		}
		at.files.push(((*name).to_owned(), index));
	}
	root
}

fn walk(dir: &Dir, prefix: &str, depth: usize, folded: &HashSet<String>, out: &mut Vec<Row>) {
	let mut dirs: Vec<(&String, &Dir)> = dir.dirs.iter().collect();
	dirs.sort_by_key(|(name, _)| name.to_lowercase());
	for (name, mut sub) in dirs {
		// Join a chain of folders that hold only one folder each.
		let mut label = name.clone();
		while sub.files.is_empty() && sub.dirs.len() == 1 {
			let (next, inner) = sub.dirs.iter().next().expect("one folder");
			label = format!("{label}/{next}");
			sub = inner;
		}
		let path = format!("{prefix}/{label}");
		let open = !folded.contains(&path);
		out.push(Row::Dir { path: path.clone(), label, depth, open });
		if open {
			walk(sub, &path, depth + 1, folded, out);
		}
	}
	let mut files: Vec<&(String, usize)> = dir.files.iter().collect();
	files.sort_by_key(|(name, _)| name.to_lowercase());
	for (name, index) in files {
		out.push(Row::File { index: *index, name: name.clone(), depth });
	}
}

/// The rows to draw, skipping what sits inside folded folders.
pub fn rows(paths: &[&str], folded: &HashSet<String>) -> Vec<Row> {
	let mut out = Vec::new();
	walk(&build(paths), "", 0, folded, &mut out);
	out
}

/// Every file's index in tree order, folded or not.
pub fn order(paths: &[&str]) -> Vec<usize> {
	rows(paths, &HashSet::new())
		.into_iter()
		.filter_map(|r| match r {
			Row::File { index, .. } => Some(index),
			Row::Dir { .. } => None,
		})
		.collect()
}

/// The folder rows a path sits under (their keys), outermost first.
pub fn parents(paths: &[&str], index: usize) -> Vec<String> {
	let all = rows(paths, &HashSet::new());
	let Some(at) = all.iter().position(|r| matches!(r, Row::File { index: i, .. } if *i == index)) else { return Vec::new() };
	let Row::File { depth, .. } = &all[at] else { return Vec::new() };
	let mut want = *depth;
	let mut out = Vec::new();
	for r in all[..at].iter().rev() {
		if want == 0 {
			break;
		}
		if let Row::Dir { path, depth, .. } = r
			&& *depth + 1 == want
		{
			out.push(path.clone());
			want -= 1;
		}
	}
	out.reverse();
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	const PATHS: [&str; 5] = [
		"/src/Checkout/OrderReview.vue",
		"/src/Checkout/TaxLine.vue",
		"/src/Checkout/billing/__proof__/billing.proof.ts",
		"/config/appsettings.json",
		"/README.md",
	];

	#[test]
	fn folders_come_first_and_lone_chains_join() {
		let r = rows(&PATHS, &HashSet::new());
		let shown: Vec<String> = r
			.iter()
			.map(|r| match r {
				Row::Dir { label, depth, .. } => format!("{depth} {label}/"),
				Row::File { name, depth, .. } => format!("{depth} {name}"),
			})
			.collect();
		assert_eq!(shown, [
			"0 config/",
			"1 appsettings.json",
			"0 src/Checkout/",
			"1 billing/__proof__/",
			"2 billing.proof.ts",
			"1 OrderReview.vue",
			"1 TaxLine.vue",
			"0 README.md",
		]);
		assert_eq!(order(&PATHS), [3, 2, 0, 1, 4]);
	}

	#[test]
	fn a_folded_folder_hides_what_is_inside() {
		let folded: HashSet<String> = ["/src/Checkout".to_owned()].into();
		let r = rows(&PATHS, &folded);
		assert!(r.contains(&Row::Dir { path: "/src/Checkout".into(), label: "src/Checkout".into(), depth: 0, open: false }));
		assert!(!r.iter().any(|r| matches!(r, Row::File { index: 0, .. })));
		assert_eq!(parents(&PATHS, 2), ["/src/Checkout", "/src/Checkout/billing/__proof__"]);
	}
}
