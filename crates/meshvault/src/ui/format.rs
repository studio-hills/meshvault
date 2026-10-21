//! Text for numbers shown to the user: counts with their noun, byte sizes
//! and digit grouping.

/// `n` with the singular or plural noun: "1 mesh", "2 meshes".
pub fn plural(n: usize, one: &str, many: &str) -> String {
	format!("{n} {}", if n == 1 { one } else { many })
}

/// A byte size in B, KiB (one decimal) or MiB (two decimals).
pub fn format_bytes(n: usize) -> String {
	match n {
		0..1024 => format!("{n} B"),
		1024..1_048_576 => format!("{:.1} KiB", n as f64 / 1024.0),
		_ => format!("{:.2} MiB", n as f64 / 1_048_576.0),
	}
}

/// 12345 -> "12,345"
pub fn format_count(n: usize) -> String {
	let s = n.to_string();
	let mut out = String::new();
	for (i, c) in s.chars().enumerate() {
		if i > 0 && (s.len() - i).is_multiple_of(3) {
			out.push(',');
		}
		out.push(c);
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn plurals_pick_the_noun_by_count() {
		assert_eq!(plural(0, "mesh", "meshes"), "0 meshes");
		assert_eq!(plural(1, "mesh", "meshes"), "1 mesh");
		assert_eq!(plural(2, "model", "models"), "2 models");
	}

	#[test]
	fn sizes_switch_unit_at_1024() {
		assert_eq!(format_bytes(0), "0 B");
		assert_eq!(format_bytes(1023), "1023 B");
		assert_eq!(format_bytes(1024), "1.0 KiB");
		assert_eq!(format_bytes(1536), "1.5 KiB");
		assert_eq!(format_bytes(1_048_575), "1024.0 KiB");
		assert_eq!(format_bytes(1_048_576), "1.00 MiB");
		assert_eq!(format_bytes(5 * 1_048_576 + 1_048_576 / 4), "5.25 MiB");
	}

	#[test]
	fn formats_counts() {
		assert_eq!(format_count(0), "0");
		assert_eq!(format_count(999), "999");
		assert_eq!(format_count(1000), "1,000");
		assert_eq!(format_count(1234567), "1,234,567");
	}
}
