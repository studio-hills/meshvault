//! Roblox content ids: rbxassetid://N and asset URLs.

/// Extracts the numeric asset id from the forms Roblox content ids take:
/// `rbxassetid://N`, `http(s)://www.roblox.com/asset/?id=N`,
/// `https://assetdelivery.roblox.com/v1/asset/?id=N`, or a bare `N`.
pub fn parse_asset_id(s: &str) -> Option<u64> {
	let s = s.trim();
	if s.is_empty() {
		return None;
	}
	if let Ok(id) = s.parse() {
		return Some(id);
	}
	if let Some(rest) = s.strip_prefix("rbxassetid://") {
		return rest.trim().parse().ok();
	}
	// any URL with an id= query parameter
	let query = s.split_once('?')?.1;
	query
		.split('&')
		.find_map(|kv| kv.strip_prefix("id=").or_else(|| kv.strip_prefix("ID=")))
		.and_then(|id| id.trim().parse().ok())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn asset_id_forms() {
		assert_eq!(
			parse_asset_id("rbxassetid://136440628894302"),
			Some(136440628894302)
		);
		assert_eq!(
			parse_asset_id("http://www.roblox.com/asset/?id=123 "),
			Some(123)
		);
		assert_eq!(parse_asset_id("http://roblox.com/asset/?id=55"), Some(55));
		assert_eq!(
			parse_asset_id("https://assetdelivery.roblox.com/v1/asset/?id=77"),
			Some(77)
		);
		assert_eq!(parse_asset_id("https://x/asset/?version=1&id=9"), Some(9));
		assert_eq!(parse_asset_id(" 42 "), Some(42));
		assert_eq!(parse_asset_id(""), None);
		assert_eq!(parse_asset_id("rbxasset://textures/foo.png"), None);
	}
}
