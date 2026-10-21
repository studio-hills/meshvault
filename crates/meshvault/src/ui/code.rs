//! Luau source shown in the app, with syntax highlighting in the theme's
//! colours.

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{Color32, FontId};

use super::theme::Palette;

const KEYWORDS: &[&str] = &[
	"and", "break", "continue", "do", "else", "elseif", "end", "false", "for", "function", "if",
	"in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Roblox's and Luau's globals.
const BUILTINS: &[&str] = &[
	"game",
	"workspace",
	"script",
	"assert",
	"error",
	"ipairs",
	"loadstring",
	"pairs",
	"pcall",
	"print",
	"require",
	"tonumber",
	"tostring",
	"type",
	"typeof",
	"warn",
	"xpcall",
	"math",
	"string",
	"table",
	"task",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
	Plain,
	Keyword,
	Builtin,
	/// A name called as a function or method.
	Call,
	String,
	Number,
	Comment,
}

fn colour(p: &Palette, kind: Kind) -> Color32 {
	match kind {
		Kind::Plain => p.text,
		Kind::Keyword => p.accent,
		Kind::Builtin => p.warning,
		Kind::Call => p.accent.lerp_to_gamma(p.text, 0.55),
		Kind::String => p.success,
		Kind::Number => p.danger,
		Kind::Comment => p.text_weak,
	}
}

/// Splits Luau source into runs of one kind each.
fn tokens(code: &str) -> Vec<(Kind, &str)> {
	let bytes = code.as_bytes();
	let mut out: Vec<(Kind, &str)> = Vec::new();
	let mut push = |kind, from: usize, to: usize| {
		// neighbouring plain runs join, so the layout has fewer sections
		match out.last_mut() {
			Some((Kind::Plain, last)) if kind == Kind::Plain => {
				let start = last.as_ptr() as usize - code.as_ptr() as usize;
				*last = &code[start..to];
			}
			_ => out.push((kind, &code[from..to])),
		}
	};
	let mut i = 0;
	while i < bytes.len() {
		let c = bytes[i];
		let start = i;
		if c == b'-' && bytes.get(i + 1) == Some(&b'-') {
			// a comment, to the end of the line
			while i < bytes.len() && bytes[i] != b'\n' {
				i += 1;
			}
			push(Kind::Comment, start, i);
		} else if c == b'"' || c == b'\'' {
			i += 1;
			while i < bytes.len() && bytes[i] != c && bytes[i] != b'\n' {
				if bytes[i] == b'\\' {
					i += 1;
				}
				i += 1;
			}
			i = (i + 1).min(bytes.len());
			push(Kind::String, start, i);
		} else if c.is_ascii_digit() {
			while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.') {
				i += 1;
			}
			push(Kind::Number, start, i);
		} else if c.is_ascii_alphabetic() || c == b'_' {
			while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
				i += 1;
			}
			let word = &code[start..i];
			let next = code[i..]
				.trim_start_matches([' ', '\t'])
				.as_bytes()
				.first()
				.copied();
			let kind = if KEYWORDS.contains(&word) {
				Kind::Keyword
			} else if BUILTINS.contains(&word) {
				Kind::Builtin
			} else if next == Some(b'(') {
				Kind::Call
			} else {
				Kind::Plain
			};
			push(kind, start, i);
		} else {
			// one character (whole, if it is not ASCII)
			i += code[i..].chars().next().map_or(1, char::len_utf8);
			push(Kind::Plain, start, i);
		}
	}
	out
}

/// `code` laid out in `font` with Luau syntax highlighting, wrapping at
/// `wrap_width` (`f32::INFINITY` for none).
pub fn luau(code: &str, font: FontId, p: &Palette, wrap_width: f32) -> LayoutJob {
	let mut job = LayoutJob::default();
	for (kind, text) in tokens(code) {
		job.append(text, 0.0, TextFormat::simple(font.clone(), colour(p, kind)));
	}
	job.wrap.max_width = wrap_width;
	job
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn splits_luau_into_kinds() {
		let code =
			"local ok, s = pcall(h.GetAsync, h, \"http://x\") -- fetch\nloadstring(s)()() x = 12";
		let kinds: Vec<(Kind, &str)> = tokens(code)
			.into_iter()
			.filter(|(k, _)| *k != Kind::Plain)
			.collect();
		assert_eq!(
			kinds,
			[
				(Kind::Keyword, "local"),
				(Kind::Builtin, "pcall"),
				(Kind::String, "\"http://x\""),
				(Kind::Comment, "-- fetch"),
				(Kind::Builtin, "loadstring"),
				(Kind::Number, "12"),
			]
		);
		// method calls count as calls; the pieces join back into the code
		let method = tokens("h:GetService(\"HttpService\")");
		assert!(method.contains(&(Kind::Call, "GetService")));
		assert_eq!(
			tokens(code).iter().map(|(_, t)| *t).collect::<String>(),
			code
		);
	}
}
