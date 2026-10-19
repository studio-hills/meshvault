//! App settings. Non-secret settings live in a JSON file in the user's
//! config folder; credentials live in the OS credential store
//! (Windows Credential Manager, macOS Keychain, Secret Service on Linux).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::download::AuthMethod;

const KEYRING_SERVICE: &str = "rbx_mesh_gui";
const MAX_RECENT: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
	pub recent_workspaces: Vec<PathBuf>,
	pub auth_method: AuthMethod,
	pub place_id: Option<u64>,
	pub concurrency: usize,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			recent_workspaces: Vec::new(),
			auth_method: AuthMethod::ApiKey,
			place_id: None,
			concurrency: 4,
		}
	}
}

fn config_path() -> Option<PathBuf> {
	Some(dirs::config_dir()?.join("rbx_mesh_gui").join("config.json"))
}

impl Config {
	pub fn load() -> Self {
		config_path()
			.and_then(|p| std::fs::read_to_string(p).ok())
			.and_then(|s| serde_json::from_str(&s).ok())
			.unwrap_or_default()
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
			.join("rbx_mesh workspaces")
	}
}

fn keyring_user(method: AuthMethod) -> &'static str {
	match method {
		AuthMethod::ApiKey => "open_cloud_api_key",
		AuthMethod::Cookie => "roblosecurity_cookie",
	}
}

pub fn load_secret(method: AuthMethod) -> Option<String> {
	keyring::Entry::new(KEYRING_SERVICE, keyring_user(method))
		.ok()?
		.get_password()
		.ok()
		.filter(|s| !s.is_empty())
}

pub fn save_secret(method: AuthMethod, secret: &str) -> Result<(), String> {
	keyring::Entry::new(KEYRING_SERVICE, keyring_user(method))
		.and_then(|e| e.set_password(secret.trim()))
		.map_err(|e| format!("Could not save to the system credential store: {e}"))
}

pub fn delete_secret(method: AuthMethod) {
	if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, keyring_user(method)) {
		let _ = entry.delete_credential();
	}
}
