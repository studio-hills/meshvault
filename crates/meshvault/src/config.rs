//! App settings. Non-secret settings live in a JSON file in the user's
//! config folder; credentials live in the OS credential store
//! (Windows Credential Manager, macOS Keychain, Secret Service on Linux).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ui::theme::ThemeMode;
use crate::workspace::download::AuthMethod;

/// Name used for the config folder and the credential store.
const APP_ID: &str = "meshvault";
/// The name used before the app was called Meshvault; settings and
/// credentials saved under it are migrated on first use.
const LEGACY_APP_ID: &str = "rbx_mesh_gui";
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
	pub lighting: crate::render::viewport::Lighting,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			recent_workspaces: Vec::new(),
			auth_method: AuthMethod::ApiKey,
			place_id: None,
			concurrency: 4,
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
		if let Some(config) = read_config(config_path()) {
			return config;
		}
		// first run after the rename: carry the old settings over
		match read_config(config_path_for(LEGACY_APP_ID)) {
			Some(config) => {
				config.save();
				config
			}
			None => Self::default(),
		}
	}

	pub fn save(&self) {
		let Some(path) = config_path() else { return };
		if let Some(dir) = path.parent() {
			let _ = std::fs::create_dir_all(dir);
		}
		if let Ok(json) = serde_json::to_string_pretty(self) {
			let _ = std::fs::write(path, json);
		}
	}

	/// Takes a workspace off the recent list. Its folder is not touched.
	pub fn remove_recent(&mut self, root: &std::path::Path) {
		self.recent_workspaces.retain(|p| p != root);
		self.save();
	}

	pub fn add_recent(&mut self, root: PathBuf) {
		self.recent_workspaces.retain(|p| p != &root);
		self.recent_workspaces.insert(0, root);
		self.recent_workspaces.truncate(MAX_RECENT);
		self.save();
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

fn read_secret(service: &str, method: AuthMethod) -> Option<String> {
	keyring::Entry::new(service, keyring_user(method))
		.ok()?
		.get_password()
		.ok()
		.filter(|s| !s.is_empty())
}

pub fn load_secret(method: AuthMethod) -> Option<String> {
	if let Some(secret) = read_secret(APP_ID, method) {
		return Some(secret);
	}
	// copy a credential saved under the old name. The old entry is left in
	// place so builds from before the rename keep working; Forget removes both.
	let secret = read_secret(LEGACY_APP_ID, method)?;
	let _ = save_secret(method, &secret);
	Some(secret)
}

pub fn save_secret(method: AuthMethod, secret: &str) -> Result<(), String> {
	keyring::Entry::new(APP_ID, keyring_user(method))
		.and_then(|e| e.set_password(secret.trim()))
		.map_err(|e| format!("Could not save to the system credential store: {e}"))
}

pub fn delete_secret(method: AuthMethod) {
	for service in [APP_ID, LEGACY_APP_ID] {
		if let Ok(entry) = keyring::Entry::new(service, keyring_user(method)) {
			let _ = entry.delete_credential();
		}
	}
}
