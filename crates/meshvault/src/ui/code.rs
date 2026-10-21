//! Luau source shown in the app, with syntax highlighting in the theme's
//! colours.

use std::ops::Range;

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

/// The level of a long bracket (`[[`, `[=[`, ..) opening at `i`: how many
/// `=` it has.
fn long_bracket(bytes: &[u8], i: usize) -> Option<usize> {
	if bytes.get(i) != Some(&b'[') {
		return None;
	}
	let level = bytes[i + 1..].iter().take_while(|&&b| b == b'=').count();
	(bytes.get(i + 1 + level) == Some(&b'[')).then_some(level)
}

/// Where a long string or comment whose text starts at `from` ends: just
/// after its closing bracket of the same level, or the end of the code.
fn long_bracket_end(bytes: &[u8], from: usize, level: usize) -> usize {
	let mut i = from;
	while i < bytes.len() {
		if bytes[i] == b']' {
			let equals = bytes[i + 1..].iter().take_while(|&&b| b == b'=').count();
			if equals == level && bytes.get(i + 1 + level) == Some(&b']') {
				return i + level + 2;
			}
		}
		i += 1;
	}
	bytes.len()
}

/// Splits Luau source into runs of one kind each.
fn tokens(code: &str) -> Vec<(Kind, &str)> {
	let bytes = code.as_bytes();
	// byte ranges into `code`, sliced once at the end
	let mut runs: Vec<(Kind, Range<usize>)> = Vec::new();
	let mut push = |kind, from: usize, to: usize| {
		// neighbouring plain runs join, so the layout has fewer sections
		match runs.last_mut() {
			Some((Kind::Plain, last)) if kind == Kind::Plain => last.end = to,
			_ => runs.push((kind, from..to)),
		}
	};
	let mut i = 0;
	while i < bytes.len() {
		let c = bytes[i];
		let start = i;
		if c == b'-' && bytes.get(i + 1) == Some(&b'-') {
			if let Some(level) = long_bracket(bytes, i + 2) {
				// a block comment, `--[[ .. ]]`, over as many lines as it takes
				i = long_bracket_end(bytes, i + level + 4, level);
			} else {
				// a comment, to the end of the line
				while i < bytes.len() && bytes[i] != b'\n' {
					i += 1;
				}
			}
			push(Kind::Comment, start, i);
		} else if let Some(level) = long_bracket(bytes, i) {
			// a long string, `[[ .. ]]`
			i = long_bracket_end(bytes, i + level + 2, level);
			push(Kind::String, start, i);
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
			// a decimal exponent may have a sign (`1e-3`); in hex, E is a digit
			let hex = matches!(bytes.get(i + 1), Some(b'x' | b'X')) && c == b'0';
			while i < bytes.len() {
				let b = bytes[i];
				let signed =
					matches!(b, b'+' | b'-') && !hex && matches!(bytes[i - 1], b'e' | b'E');
				if !(b.is_ascii_alphanumeric() || b == b'.' || signed) {
					break;
				}
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
	runs.into_iter()
		.map(|(kind, range)| (kind, &code[range]))
		.collect()
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

	/// Today's output for escapes, non-ASCII text and numbers, pinned so a
	/// change to the lexer shows up here. An exponent keeps its sign.
	#[test]
	fn pins_escapes_unicode_and_numbers() {
		assert_eq!(
			tokens(r#"s = "a\"b""#),
			[(Kind::Plain, "s = "), (Kind::String, r#""a\"b""#)]
		);
		assert_eq!(
			tokens("print(\"é\") -- café\né"),
			[
				(Kind::Builtin, "print"),
				(Kind::Plain, "("),
				(Kind::String, "\"é\""),
				(Kind::Plain, ") "),
				(Kind::Comment, "-- café"),
				(Kind::Plain, "\né"),
			]
		);
		assert_eq!(
			tokens("x = 1e-3"),
			[(Kind::Plain, "x = "), (Kind::Number, "1e-3")]
		);
		assert_eq!(
			tokens("x = 2.5E+10-1"),
			[
				(Kind::Plain, "x = "),
				(Kind::Number, "2.5E+10"),
				(Kind::Plain, "-"),
				(Kind::Number, "1"),
			]
		);
		// in hex, E is a digit: the minus is a subtraction
		assert_eq!(
			tokens("0xE-1"),
			[
				(Kind::Number, "0xE"),
				(Kind::Plain, "-"),
				(Kind::Number, "1"),
			]
		);
		assert_eq!(
			tokens("y = 0xFF + 1.5"),
			[
				(Kind::Plain, "y = "),
				(Kind::Number, "0xFF"),
				(Kind::Plain, " + "),
				(Kind::Number, "1.5"),
			]
		);
	}

	/// Block comments and long strings run over lines to their closing
	/// bracket of the same level (or the end of the code).
	#[test]
	fn block_comments_and_long_strings_span_lines() {
		assert_eq!(
			tokens("--[[ one\ntwo ]] x"),
			[(Kind::Comment, "--[[ one\ntwo ]]"), (Kind::Plain, " x")]
		);
		assert_eq!(
			tokens("--[==[ a ]] ]=] b\n]==]local"),
			[
				(Kind::Comment, "--[==[ a ]] ]=] b\n]==]"),
				(Kind::Keyword, "local"),
			]
		);
		assert_eq!(
			tokens("s = [[a\n\"b\"]] .. [=[c]=]"),
			[
				(Kind::Plain, "s = "),
				(Kind::String, "[[a\n\"b\"]]"),
				(Kind::Plain, " .. "),
				(Kind::String, "[=[c]=]"),
			]
		);
		// never closed: to the end
		assert_eq!(
			tokens("--[[ open\nend"),
			[(Kind::Comment, "--[[ open\nend")]
		);
		// not long brackets: a line comment, and indexing
		assert_eq!(
			tokens("--[ x\nt[i]"),
			[(Kind::Comment, "--[ x"), (Kind::Plain, "\nt[i]")]
		);
		assert_eq!(
			tokens("--[= x\n"),
			[(Kind::Comment, "--[= x"), (Kind::Plain, "\n")]
		);
		assert_eq!(
			tokens("t[ [[k]] ]"),
			[
				(Kind::Plain, "t[ "),
				(Kind::String, "[[k]]"),
				(Kind::Plain, " ]"),
			]
		);
	}
}
