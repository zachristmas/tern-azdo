//! The file tree's filter: one line that people type and agents send.
//!
//! Terms are separated by commas, and a file shows when it matches any term (none left out by a `!` term):
//! `checkout` matches loosely, the way a fuzzy finder does (letters in order, anywhere in the path); `*.cs` or
//! `src/**/Tax*` is a glob; `/\.vue$/` is a regex between slashes (case-insensitive, and it may hold commas);
//! `!test` leaves out what matches it, and takes any of the three forms.

use regex::Regex;

use crate::review;

enum Term {
	Fuzzy(String),
	Pattern(Regex),
}

impl Term {
	fn matches(&self, path: &str) -> bool {
		match self {
			Term::Fuzzy(q) => review::fuzzy(q, path).is_some(),
			Term::Pattern(re) => re.is_match(path),
		}
	}
}

#[derive(Default)]
pub struct Filter {
	text:    String,
	include: Vec<Term>,
	exclude: Vec<Term>,
	error:   Option<String>,
}

/// The raw terms of a filter line.
fn split(text: &str) -> Vec<&str> {
	let b = text.as_bytes();
	let mut out = Vec::new();
	let mut i = 0;
	while i < b.len() {
		while i < b.len() && (b[i] == b',' || b[i].is_ascii_whitespace()) {
			i += 1;
		}
		if i >= b.len() {
			break;
		}
		let start = i;
		let body = if b[i] == b'!' { i + 1 } else { i };
		// A regex runs to the slash that ends the term, so commas inside it stay.
		let mut end = None;
		if body < b.len() && b[body] == b'/' {
			let mut j = body + 1;
			while j < b.len() {
				if b[j] == b'\\' {
					j += 2;
					continue;
				}
				if b[j] == b'/' {
					let rest = text[j + 1..].trim_start();
					if rest.is_empty() || rest.starts_with(',') {
						end = Some(j + 1);
						break;
					}
				}
				j += 1;
			}
		}
		let end = end.unwrap_or_else(|| text[i..].find(',').map_or(b.len(), |k| i + k));
		out.push(text[start..end].trim());
		i = end;
	}
	out
}

fn glob(g: &str) -> String {
	let mut re = String::from("(?i)(^|/)");
	let mut chars = g.chars().peekable();
	while let Some(c) = chars.next() {
		match c {
			'*' if chars.peek() == Some(&'*') => {
				chars.next();
				re.push_str(".*");
			},
			'*' => re.push_str("[^/]*"),
			'?' => re.push_str("[^/]"),
			c => re.push_str(&regex::escape(&c.to_string())),
		}
	}
	re.push('$');
	re
}

fn term(raw: &str) -> Result<Option<(bool, Term)>, String> {
	let (not, s) = match raw.strip_prefix('!') {
		Some(rest) => (true, rest.trim()),
		None => (false, raw),
	};
	if s.is_empty() {
		return Ok(None);
	}
	let compiled = if s.len() >= 2 && s.starts_with('/') && s.ends_with('/') {
		Term::Pattern(Regex::new(&format!("(?i){}", &s[1..s.len() - 1])).map_err(|e| format!("{s}: {}", e.to_string().lines().last().unwrap_or("bad pattern").trim()))?)
	} else if s.contains(['*', '?']) {
		Term::Pattern(Regex::new(&glob(s)).map_err(|e| format!("{s}: {e}"))?)
	} else {
		Term::Fuzzy(s.to_owned())
	};
	Ok(Some((not, compiled)))
}

impl Filter {
	pub fn parse(text: &str) -> Self {
		let mut f = Filter { text: text.to_owned(), ..Filter::default() };
		for raw in split(text) {
			match term(raw) {
				Ok(Some((true, t))) => f.exclude.push(t),
				Ok(Some((false, t))) => f.include.push(t),
				Ok(None) => {},
				Err(e) => f.error = Some(e),
			}
		}
		f
	}

	pub fn text(&self) -> &str {
		&self.text
	}

	/// What is wrong with the line (a pattern that does not compile), if anything. Terms that are fine still apply.
	pub fn error(&self) -> Option<&str> {
		self.error.as_deref()
	}

	/// Whether it narrows anything.
	pub fn is_active(&self) -> bool {
		!self.include.is_empty() || !self.exclude.is_empty()
	}

	/// Whether a changed file (its path, with or without the leading slash) shows.
	pub fn matches(&self, path: &str) -> bool {
		let path = path.trim_start_matches('/');
		(self.include.is_empty() || self.include.iter().any(|t| t.matches(path))) && !self.exclude.iter().any(|t| t.matches(path))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const FILES: [&str; 6] = [
		"/src/Checkout/OrderReview.vue",
		"/src/Checkout/TaxLine.vue",
		"/src/Checkout/billing/billing.proof.ts",
		"/config/appsettings.json",
		"/README.md",
		"/tests/TaxLineTests.cs",
	];

	fn shown(text: &str) -> Vec<&'static str> {
		let f = Filter::parse(text);
		FILES.into_iter().filter(|p| f.matches(p)).collect()
	}

	#[test]
	fn an_empty_filter_shows_everything() {
		assert!(!Filter::parse("").is_active());
		assert_eq!(shown(" , ").len(), FILES.len());
	}

	#[test]
	fn plain_terms_are_fuzzy_and_comma_separated_terms_are_or() {
		assert_eq!(shown("ordrvw"), ["/src/Checkout/OrderReview.vue"]);
		assert_eq!(shown("readme, appset"), ["/config/appsettings.json", "/README.md"]);
	}

	#[test]
	fn globs_and_regexes() {
		assert_eq!(shown("*.vue"), ["/src/Checkout/OrderReview.vue", "/src/Checkout/TaxLine.vue"]);
		assert_eq!(shown("src/**/*.ts"), ["/src/Checkout/billing/billing.proof.ts"]);
		assert_eq!(shown(r"/^src\/.*(Order|Tax)/"), ["/src/Checkout/OrderReview.vue", "/src/Checkout/TaxLine.vue"]);
		// Commas inside a regex do not split it.
		assert_eq!(shown("/^tests\\/.{1,2}Line/, readme"), ["/README.md"]);
		assert_eq!(shown("/^tests\\/.{1,3}Line/"), ["/tests/TaxLineTests.cs"]);
	}

	#[test]
	fn a_bang_leaves_files_out() {
		assert_eq!(shown("!tests, !/\\.json$/").len(), 4);
		assert_eq!(shown("tax, !*Tests.cs"), ["/src/Checkout/TaxLine.vue"]);
	}

	#[test]
	fn a_broken_regex_is_reported_and_the_rest_still_applies() {
		let f = Filter::parse("/(/, readme");
		assert!(f.error().is_some());
		assert!(f.matches("/README.md") && !f.matches("/src/a.vue"));
	}
}
