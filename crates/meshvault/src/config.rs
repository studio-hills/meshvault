//! App settings. Non-secret settings live in a JSON file in the user's
//! config folder; credentials live in the OS credential store
//! (Windows Credential Manager; macOS Keychain; on Linux the kernel keyring,
//! which keeps them only until reboot).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ui::theme::ThemeMode;
use crate::workspace::download::{AuthMethod, DEFAULT_CONCURRENCY};

/// Name used for the config folder and the credential store.
const APP_ID: &str = "meshvault";
/// The name used before the app was called Meshvault; settings and
/// credentials saved under it are migrated on first use.
const LEGACY_APP_ID: &str = "rbx_mesh_gui";
/// How many recent workspaces the welcome screen lists.
const MAX_RECENT: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
	pub recent_workspaces: Vec<PathBuf>,
	pub auth_method: AuthMethod,
	pub place_id: Option<u64>,
	pub concurrency: usize,
	pub theme: ThemeMode,
	/// How the 3D view is lit.
	pub lighting: crate::render::lighting::Lighting,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			recent_workspaces: Vec::new(),
			auth_method: AuthMethod::ApiKey,
			place_id: None,
			concurrency: DEFAULT_CONCURRENCY,
			theme: ThemeMode::Dark,
			lighting: Default::default(),
		}
	}
}

fn config_path_for(app_id: &str) -> Option<PathBuf> {
	Some(dirs::config_dir()?.join(app_id).join("config.json"))
}

fn config_path() -> Option<PathBuf> {
	config_path_for(APP_ID)
}

fn read_config(path: Option<PathBuf>) -> Option<Config> {
	let text = std::fs::read_to_string(path?).ok()?;
	serde_json::from_str(&text).ok()
}

impl Config {
	pub fn load() -> Self {
		Self::load_from(config_path(), config_path_for(LEGACY_APP_ID))
	}

	/// Reads the settings at `primary`, or on the first run after the
	/// rename carries those at `legacy` over, saving them to `primary`.
	fn load_from(primary: Option<PathBuf>, legacy: Option<PathBuf>) -> Self {
		if let Some(config) = read_config(primary.clone()) {
			return config;
		}
		match read_config(legacy) {
			Some(config) => {
				if let Some(path) = &primary {
					// not reported yet (bug B08); the old settings still load
					let _ = config.save_to(path);
				}
				config
			}
			None => Self::default(),
		}
	}

	pub fn save(&self) -> Result<(), String> {
		let path = config_path().ok_or("Could not find the user's config folder")?;
		self.save_to(&path)
	}

	/// Writes the settings to `path`, making its folder first. The write is
	/// tried even when the folder cannot be made, and its error is the one
	/// reported.
	fn save_to(&self, path: &Path) -> Result<(), String> {
		if let Some(dir) = path.parent() {
			let _ = std::fs::create_dir_all(dir);
		}
		let json = serde_json::to_string_pretty(self)
			.map_err(|e| format!("Could not save the settings: {e}"))?;
		std::fs::write(path, json)
			.map_err(|e| format!("Could not save the settings to {}: {e}", path.display()))
	}

	/// Takes a workspace off the recent list. Its folder is not touched.
	pub fn remove_recent(&mut self, root: &Path) {
		self.recent_workspaces.retain(|p| p != root);
		// a failed save is not reported yet (bug B08)
		let _ = self.save();
	}

	pub fn add_recent(&mut self, root: PathBuf) {
		self.push_recent(root);
		// a failed save is not reported yet (bug B08)
		let _ = self.save();
	}

	/// Puts a workspace first on the recent list, once, keeping the newest
	/// `MAX_RECENT`.
	fn push_recent(&mut self, root: PathBuf) {
		self.recent_workspaces.retain(|p| p != &root);
		self.recent_workspaces.insert(0, root);
		self.recent_workspaces.truncate(MAX_RECENT);
	}

	/// The workspaces folder suggested for new workspaces.
	pub fn default_workspaces_dir() -> PathBuf {
		dirs::document_dir()
			.or_else(dirs::home_dir)
			.unwrap_or_else(|| PathBuf::from("."))
			.join("Meshvault")
	}
}

fn keyring_user(method: AuthMethod) -> &'static str {
	match method {
		AuthMethod::ApiKey => "open_cloud_api_key",
		AuthMethod::Cookie => "roblosecurity_cookie",
	}
}

/// The credential saved under `service`. An empty one counts as none; a
/// credential store that cannot be read is an error.
fn read_secret(service: &str, method: AuthMethod) -> Result<Option<String>, String> {
	let read = keyring::Entry::new(service, keyring_user(method)).and_then(|e| e.get_password());
	match read {
		Ok(secret) => Ok(Some(secret).filter(|s| !s.is_empty())),
		Err(keyring::Error::NoEntry) => Ok(None),
		Err(e) => Err(format!(
			"Could not read from the system credential store: {e}"
		)),
	}
}

pub fn load_secret(method: AuthMethod) -> Option<String> {
	// TODO(release): a credential store that cannot be read looks the same
	// as no credential here (bug B07)
	if let Ok(Some(secret)) = read_secret(APP_ID, method) {
		return Some(secret);
	}
	// copy a credential saved under the old name. The old entry is left in
	// place so builds from before the rename keep working; Forget removes both.
	let secret = read_secret(LEGACY_APP_ID, method).ok().flatten()?;
	let _ = save_secret(method, &secret);
	Some(secret)
}

pub fn save_secret(method: AuthMethod, secret: &str) -> Result<(), String> {
	keyring::Entry::new(APP_ID, keyring_user(method))
		.and_then(|e| e.set_password(secret.trim()))
		.map_err(|e| format!("Could not save to the system credential store: {e}"))
}

/// Removes the credential under both the current and the old name. Both
/// are tried even if one fails; one that was never saved is not an error.
pub fn delete_secret(method: AuthMethod) -> Result<(), String> {
	let mut failed = Vec::new();
	for service in [APP_ID, LEGACY_APP_ID] {
		match keyring::Entry::new(service, keyring_user(method)).and_then(|e| e.delete_credential())
		{
			Ok(()) | Err(keyring::Error::NoEntry) => {}
			Err(e) => failed.push(e.to_string()),
		}
	}
	if failed.is_empty() {
		Ok(())
	} else {
		Err(format!(
			"Could not remove from the system credential store: {}",
			failed.join("; ")
		))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::test_support::TempDir;

	fn json(config: &Config) -> serde_json::Value {
		serde_json::to_value(config).unwrap()
	}

	#[test]
	fn recent_workspaces_come_newest_first_once_each() {
		let mut config = Config::default();
		config.push_recent("a".into());
		config.push_recent("b".into());
		config.push_recent("a".into());
		assert_eq!(config.recent_workspaces, [PathBuf::from("a"), "b".into()]);
		for i in 0..MAX_RECENT + 2 {
			config.push_recent(format!("w{i}").into());
		}
		assert_eq!(config.recent_workspaces.len(), MAX_RECENT);
		assert_eq!(
			config.recent_workspaces[0],
			PathBuf::from(format!("w{}", MAX_RECENT + 1))
		);
		assert_eq!(
			config.recent_workspaces[MAX_RECENT - 1],
			PathBuf::from("w2")
		);
	}

	#[test]
	fn saved_settings_load_back() {
		let dir = TempDir::new("config");
		let path = dir.join("meshvault").join("config.json");
		let mut config = Config {
			place_id: Some(42),
			concurrency: 7,
			theme: ThemeMode::Light,
			..Default::default()
		};
		config.push_recent("ws".into());
		// the folder is made
		config.save_to(&path).unwrap();
		let loaded = Config::load_from(Some(path), None);
		assert_eq!(json(&loaded), json(&config));
	}

	#[test]
	fn a_failed_save_says_why() {
		let dir = TempDir::new("config");
		// a folder where the file should be cannot be written over
		let path = dir.join("config.json");
		std::fs::create_dir_all(&path).unwrap();
		let err = Config::default().save_to(&path).unwrap_err();
		assert!(err.contains("config.json"), "{err}");
	}

	#[test]
	fn old_settings_are_carried_over_to_the_new_path() {
		let dir = TempDir::new("config");
		let primary = dir.join("meshvault").join("config.json");
		let legacy = dir.join("rbx_mesh_gui").join("config.json");
		std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
		let old = r#"{"place_id": 9, "concurrency": 2}"#;
		std::fs::write(&legacy, old).unwrap();

		let config = Config::load_from(Some(primary.clone()), Some(legacy.clone()));
		assert_eq!(config.place_id, Some(9));
		assert_eq!(config.concurrency, 2);
		// saved to the new path; the old file is left as it was
		let saved: serde_json::Value =
			serde_json::from_str(&std::fs::read_to_string(&primary).unwrap()).unwrap();
		assert_eq!(saved, json(&config));
		assert_eq!(std::fs::read_to_string(&legacy).unwrap(), old);

		// from then on the new file is the one read
		std::fs::write(&legacy, r#"{"place_id": 1}"#).unwrap();
		let config = Config::load_from(Some(primary), Some(legacy));
		assert_eq!(config.place_id, Some(9));
	}

	#[test]
	fn without_a_new_path_old_settings_load_but_are_not_saved() {
		let dir = TempDir::new("config");
		let legacy = dir.join("config.json");
		std::fs::write(&legacy, r#"{"place_id": 9}"#).unwrap();
		let config = Config::load_from(None, Some(legacy));
		assert_eq!(config.place_id, Some(9));
		let files: Vec<_> = std::fs::read_dir(&*dir).unwrap().collect();
		assert_eq!(files.len(), 1);
	}

	#[test]
	fn no_settings_anywhere_gives_the_defaults() {
		let dir = TempDir::new("config");
		let primary = dir.join("config.json");
		let config = Config::load_from(Some(primary.clone()), Some(dir.join("old.json")));
		assert_eq!(json(&config), json(&Config::default()));
		// nothing is written until something changes
		assert!(!primary.exists());
		let config = Config::load_from(None, None);
		assert_eq!(json(&config), json(&Config::default()));
	}

	#[test]
	fn unknown_keys_are_ignored_and_missing_fields_take_defaults() {
		let dir = TempDir::new("config");
		let path = dir.join("config.json");
		std::fs::write(
			&path,
			r#"{"place_id": 5, "theme": "Light", "from_the_future": [1, 2]}"#,
		)
		.unwrap();
		let config = Config::load_from(Some(path), None);
		let expected = Config {
			place_id: Some(5),
			theme: ThemeMode::Light,
			..Default::default()
		};
		assert_eq!(json(&config), json(&expected));
	}

	#[test]
	fn a_file_that_does_not_parse_is_dropped_whole() {
		let dir = TempDir::new("config");
		let path = dir.join("config.json");
		// a valid field next to an unknown variant, a wrong type, or bad JSON
		for text in [
			r#"{"place_id": 5, "theme": "Purple"}"#,
			r#"{"place_id": 5, "concurrency": "four"}"#,
			r#"{"place_id": 5,"#,
		] {
			std::fs::write(&path, text).unwrap();
			let config = Config::load_from(Some(path.clone()), None);
			assert_eq!(json(&config), json(&Config::default()), "{text}");
			// the file itself is left alone
			assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
		}
	}

	#[test]
	fn a_new_file_that_does_not_parse_is_replaced_by_the_old_one() {
		// today's behaviour (bug B08): the unreadable file is overwritten
		let dir = TempDir::new("config");
		let primary = dir.join("config.json");
		let legacy = dir.join("old.json");
		std::fs::write(&primary, "not json").unwrap();
		std::fs::write(&legacy, r#"{"place_id": 3}"#).unwrap();
		let config = Config::load_from(Some(primary.clone()), Some(legacy));
		assert_eq!(config.place_id, Some(3));
		let saved: serde_json::Value =
			serde_json::from_str(&std::fs::read_to_string(&primary).unwrap()).unwrap();
		assert_eq!(saved, json(&config));
	}
}
