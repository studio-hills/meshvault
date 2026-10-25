//! App settings. Non-secret settings live in a JSON file in the user's
//! config folder; credentials live in the OS credential store
//! (Windows Credential Manager; macOS Keychain; on Linux the kernel keyring,
//! which keeps them only until reboot).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
	pub concurrency: usize,
	pub theme: ThemeMode,
	/// How the 3D view is lit.
	pub lighting: crate::render::lighting::Lighting,
	/// Whether to ask GitHub for a newer release at startup.
	pub check_for_updates: bool,
	/// What exports from Roblox Studio leave out, as chosen in the export
	/// dialog.
	pub leave_out: crate::studio::link::LeaveOut,
	/// A settings file that could not be read nor moved aside when it was
	/// loaded. No save replaces it until it has been moved aside.
	#[serde(skip)]
	unreadable: Option<PathBuf>,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			recent_workspaces: Vec::new(),
			auth_method: AuthMethod::ApiKey,
			concurrency: DEFAULT_CONCURRENCY,
			theme: ThemeMode::Dark,
			lighting: Default::default(),
			check_for_updates: true,
			leave_out: Default::default(),
			unreadable: None,
		}
	}
}

fn config_path_for(app_id: &str) -> Option<PathBuf> {
	Some(dirs::config_dir()?.join(app_id).join("config.json"))
}

fn config_path() -> Option<PathBuf> {
	config_path_for(APP_ID)
}

/// The settings in the file at `path`, or `None` if there is no file.
fn read_config(path: &Path) -> Result<Option<Config>, String> {
	let text = match std::fs::read_to_string(path) {
		Ok(text) => text,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
		Err(e) => return Err(e.to_string()),
	};
	// editors that save UTF-8 with a byte-order mark are common on Windows
	serde_json::from_str(text.trim_start_matches('\u{feff}'))
		.map(Some)
		.map_err(|e| e.to_string())
}

/// Moves a settings file that could not be read out of the way, so the
/// next save does not overwrite it. Returns where it was kept.
fn set_aside(path: &Path) -> std::io::Result<PathBuf> {
	set_aside_with(path, || {})
}

/// [`set_aside`], calling `before_keep` once a name for the kept file is
/// chosen, just before the file is moved there. The name is this app's own
/// (by process id and a count), so no other app moves a file over it, and
/// the move takes whatever is at `path` by then: if another app has saved
/// its settings there meanwhile, those are what is kept, never deleted.
/// A file already gone was moved aside by another app (`NotFound`).
fn set_aside_with(path: &Path, before_keep: impl FnOnce()) -> std::io::Result<PathBuf> {
	static NEXT: AtomicU64 = AtomicU64::new(0);
	let name = path.file_name().unwrap_or_default().to_string_lossy();
	let kept = loop {
		let n = NEXT.fetch_add(1, Ordering::Relaxed);
		let kept = path.with_file_name(format!("{name}.bad-{}-{n}", std::process::id()));
		// left by an earlier app that had the same process id
		if !kept.exists() {
			break kept;
		}
	};
	before_keep();
	std::fs::rename(path, &kept)?;
	Ok(kept)
}

/// Writes `bytes` to a new file beside `path` and closes it, returning its
/// name. The name is this write's own (by process id and a count, and made
/// only if it does not exist), so two apps saving at once never write to
/// the same file, and neither can remove the other's.
fn write_beside(path: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
	static NEXT: AtomicU64 = AtomicU64::new(0);
	let name = path.file_name().unwrap_or_default().to_string_lossy();
	loop {
		let n = NEXT.fetch_add(1, Ordering::Relaxed);
		let tmp = path.with_file_name(format!("{name}.{}-{n}.tmp", std::process::id()));
		let mut file = match std::fs::OpenOptions::new()
			.write(true)
			.create_new(true)
			.open(&tmp)
		{
			Ok(file) => file,
			// left by an earlier app that had the same process id
			Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
			Err(e) => return Err(e),
		};
		let written = file.write_all(bytes);
		drop(file);
		return match written {
			Ok(()) => Ok(tmp),
			Err(e) => {
				let _ = std::fs::remove_file(&tmp);
				Err(e)
			}
		};
	}
}

impl Config {
	/// Reads the settings, with anything that went wrong to tell the user.
	pub fn load() -> (Self, Vec<String>) {
		Self::load_from(config_path(), config_path_for(LEGACY_APP_ID))
	}

	/// Reads the settings at `primary`, or on the first run after the
	/// rename carries those at `legacy` over, saving them to `primary`.
	/// A `primary` that cannot be read is moved aside, not overwritten.
	fn load_from(primary: Option<PathBuf>, legacy: Option<PathBuf>) -> (Self, Vec<String>) {
		let mut problems = Vec::new();
		let mut unreadable = None;
		if let Some(path) = &primary {
			match read_config(path) {
				Ok(Some(config)) => return (config, problems),
				Ok(None) => {}
				Err(why) => match set_aside(path) {
					Ok(kept) => problems.push(format!(
						"Could not read the settings in {} ({why}). The file was kept as {}.",
						path.display(),
						kept.display()
					)),
					Err(e) => {
						problems.push(format!(
							"Could not read the settings in {} ({why}), nor move the file aside: {e}",
							path.display()
						));
						unreadable = Some(path.clone());
					}
				},
			}
		}
		// the old file is never written, so one that does not read is left be
		let old = legacy.and_then(|path| read_config(&path).ok().flatten());
		let config = match old {
			Some(mut config) => {
				config.unreadable = unreadable;
				if let Some(path) = &primary
					&& let Err(e) = config.save_to(path)
				{
					problems.push(e);
				}
				config
			}
			None => Self {
				unreadable,
				..Self::default()
			},
		};
		(config, problems)
	}

	pub fn save(&mut self) -> Result<(), String> {
		let path = config_path().ok_or("Could not find the user's config folder")?;
		self.save_to(&path)
	}

	/// Writes the settings to `path`, making its folder first. The write is
	/// tried even when the folder cannot be made, and its error is the one
	/// reported. The file is written beside `path` and then renamed over
	/// it, so a failed write never leaves half a file. A settings file that
	/// could not be read is first moved aside; until it can be, nothing is
	/// saved.
	fn save_to(&mut self, path: &Path) -> Result<(), String> {
		self.save_with(path, || {}, || {})
	}

	/// [`Self::save_to`], calling `before_aside` as an unreadable file is
	/// about to be kept (see [`set_aside_with`]), and `before_replace` once
	/// the new file is written and closed, just before it replaces the old
	/// one.
	fn save_with(
		&mut self,
		path: &Path,
		before_aside: impl FnOnce(),
		before_replace: impl FnOnce(),
	) -> Result<(), String> {
		if let Some(bad) = &self.unreadable {
			match set_aside_with(bad, before_aside) {
				Ok(_) => {}
				Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
				Err(e) => {
					return Err(format!(
						"Could not save the settings: {} could not be read nor moved aside, so it was kept: {e}",
						bad.display()
					));
				}
			}
			self.unreadable = None;
		}
		if let Some(dir) = path.parent() {
			let _ = std::fs::create_dir_all(dir);
		}
		let json = serde_json::to_string_pretty(self)
			.map_err(|e| format!("Could not save the settings: {e}"))?;
		let failed =
			|e: std::io::Error| format!("Could not save the settings to {}: {e}", path.display());
		let tmp = write_beside(path, json.as_bytes()).map_err(failed)?;
		before_replace();
		std::fs::rename(&tmp, path).map_err(|e| {
			let _ = std::fs::remove_file(&tmp);
			failed(e)
		})
	}

	/// Takes a workspace off the recent list. Its folder is not touched.
	pub fn remove_recent(&mut self, root: &Path) -> Result<(), String> {
		self.recent_workspaces.retain(|p| p != root);
		self.save()
	}

	pub fn add_recent(&mut self, root: PathBuf) -> Result<(), String> {
		self.push_recent(root);
		self.save()
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

/// The saved credential, or `None` when there is none. A credential store
/// that cannot be read is an error, not "none", so the user is not sent to
/// enter a credential that is in fact saved.
pub fn load_secret(method: AuthMethod) -> Result<Option<String>, String> {
	pick_secret(
		|service| read_secret(service, method),
		|secret| save_secret(method, secret),
	)
}

/// Reads the current credential, falling back to one saved under the old
/// name, which is copied to the current name. An unreadable current entry
/// stops there rather than quietly using an older credential.
fn pick_secret(
	mut read: impl FnMut(&str) -> Result<Option<String>, String>,
	copy: impl FnOnce(&str) -> Result<(), String>,
) -> Result<Option<String>, String> {
	if let Some(secret) = read(APP_ID)? {
		return Ok(Some(secret));
	}
	// The old entry is left in place so builds from before the rename keep
	// working; Forget removes both.
	let Some(secret) = read(LEGACY_APP_ID)? else {
		return Ok(None);
	};
	// a failed copy still leaves the old entry to read next time
	let _ = copy(&secret);
	Ok(Some(secret))
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

	/// Loads settings that are expected to read without a problem.
	fn load(primary: Option<PathBuf>, legacy: Option<PathBuf>) -> Config {
		let (config, problems) = Config::load_from(primary, legacy);
		assert!(problems.is_empty(), "{problems:?}");
		config
	}

	fn json(config: &Config) -> serde_json::Value {
		serde_json::to_value(config).unwrap()
	}

	/// A fake store: what reading each service gives, and what was copied.
	fn pick(
		current: Result<Option<&str>, &str>,
		legacy: Result<Option<&str>, &str>,
	) -> (Result<Option<String>, String>, Option<String>) {
		let mut copied = None;
		let picked = pick_secret(
			|service| {
				let read = if service == APP_ID { current } else { legacy };
				read.map(|s| s.map(str::to_string)).map_err(str::to_string)
			},
			|secret| {
				copied = Some(secret.to_string());
				Ok(())
			},
		);
		(picked, copied)
	}

	#[test]
	fn the_current_credential_wins_and_an_old_one_is_copied_over() {
		assert_eq!(
			pick(Ok(Some("new")), Ok(Some("old"))),
			(Ok(Some("new".into())), None)
		);
		assert_eq!(
			pick(Ok(None), Ok(Some("old"))),
			(Ok(Some("old".into())), Some("old".into()))
		);
		assert_eq!(pick(Ok(None), Ok(None)), (Ok(None), None));
	}

	#[test]
	fn a_store_that_cannot_be_read_is_not_taken_for_no_credential() {
		// not "none", and not quietly the older credential either
		assert_eq!(
			pick(Err("locked"), Ok(Some("old"))),
			(Err("locked".into()), None)
		);
		assert_eq!(pick(Err("locked"), Ok(None)), (Err("locked".into()), None));
		assert_eq!(pick(Ok(None), Err("locked")), (Err("locked".into()), None));
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
			concurrency: 7,
			theme: ThemeMode::Light,
			..Default::default()
		};
		config.push_recent("ws".into());
		// the folder is made
		config.save_to(&path).unwrap();
		let loaded = load(Some(path), None);
		assert_eq!(json(&loaded), json(&config));
	}

	#[test]
	fn old_settings_are_carried_over_to_the_new_path() {
		let dir = TempDir::new("config");
		let primary = dir.join("meshvault").join("config.json");
		let legacy = dir.join("rbx_mesh_gui").join("config.json");
		std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
		let old = r#"{"concurrency": 9, "theme": "Light"}"#;
		std::fs::write(&legacy, old).unwrap();

		let config = load(Some(primary.clone()), Some(legacy.clone()));
		assert_eq!(config.concurrency, 9);
		assert_eq!(config.theme, ThemeMode::Light);
		// saved to the new path; the old file is left as it was
		let saved: serde_json::Value =
			serde_json::from_str(&std::fs::read_to_string(&primary).unwrap()).unwrap();
		assert_eq!(saved, json(&config));
		assert_eq!(std::fs::read_to_string(&legacy).unwrap(), old);

		// from then on the new file is the one read
		std::fs::write(&legacy, r#"{"concurrency": 1}"#).unwrap();
		let config = load(Some(primary), Some(legacy));
		assert_eq!(config.concurrency, 9);
	}

	#[test]
	fn without_a_new_path_old_settings_load_but_are_not_saved() {
		let dir = TempDir::new("config");
		let legacy = dir.join("config.json");
		std::fs::write(&legacy, r#"{"concurrency": 9}"#).unwrap();
		let config = load(None, Some(legacy));
		assert_eq!(config.concurrency, 9);
		let files: Vec<_> = std::fs::read_dir(&*dir).unwrap().collect();
		assert_eq!(files.len(), 1);
	}

	#[test]
	fn no_settings_anywhere_gives_the_defaults() {
		let dir = TempDir::new("config");
		let primary = dir.join("config.json");
		let config = load(Some(primary.clone()), Some(dir.join("old.json")));
		assert_eq!(json(&config), json(&Config::default()));
		// nothing is written until something changes
		assert!(!primary.exists());
		let config = load(None, None);
		assert_eq!(json(&config), json(&Config::default()));
	}

	#[test]
	fn unknown_keys_are_ignored_and_missing_fields_take_defaults() {
		let dir = TempDir::new("config");
		let path = dir.join("config.json");
		std::fs::write(
			&path,
			r#"{"concurrency": 5, "theme": "Light", "from_the_future": [1, 2]}"#,
		)
		.unwrap();
		let config = load(Some(path), None);
		let expected = Config {
			concurrency: 5,
			theme: ThemeMode::Light,
			..Default::default()
		};
		assert_eq!(json(&config), json(&expected));
	}

	/// Windows PowerShell 5.1 and older Notepad save UTF-8 with a byte-order
	/// mark; a settings file edited with them is still read, not set aside.
	#[test]
	fn a_byte_order_mark_is_ignored() {
		let dir = TempDir::new("config");
		let path = dir.join("config.json");
		std::fs::write(&path, "\u{feff}{\"theme\": \"Light\"}").unwrap();
		let config = load(Some(path), None);
		assert_eq!(config.theme, ThemeMode::Light);
		let files: Vec<_> = std::fs::read_dir(&*dir).unwrap().collect();
		assert_eq!(files.len(), 1, "nothing set aside");
	}

	#[test]
	fn a_file_that_does_not_parse_is_set_aside_whole() {
		let dir = TempDir::new("config");
		let path = dir.join("config.json");
		// a valid field next to an unknown variant, a wrong type, or bad JSON
		let texts = [
			r#"{"concurrency": 5, "theme": "Purple"}"#,
			r#"{"theme": "Light", "concurrency": "four"}"#,
			r#"{"concurrency": 5,"#,
		];
		for text in texts {
			std::fs::write(&path, text).unwrap();
			let (config, problems) = Config::load_from(Some(path.clone()), None);
			assert_eq!(json(&config), json(&Config::default()), "{text}");
			assert_eq!(problems.len(), 1, "{text}");
			assert!(
				problems[0].starts_with("Could not read the settings"),
				"{problems:?}"
			);
			// moved out of the way, so the next save keeps it
			assert!(!path.exists());
		}
		// each kept under its own name
		let mut texts = texts.map(str::to_string).to_vec();
		texts.sort();
		assert_eq!(kept(&dir), texts);
	}

	#[test]
	fn a_new_file_that_does_not_parse_is_kept_when_the_old_one_is_carried_over() {
		// once the unreadable file was overwritten here (bug B08)
		let dir = TempDir::new("config");
		let primary = dir.join("config.json");
		let legacy = dir.join("old.json");
		std::fs::write(&primary, "not json").unwrap();
		std::fs::write(&legacy, r#"{"concurrency": 3}"#).unwrap();
		let (config, problems) = Config::load_from(Some(primary.clone()), Some(legacy));
		assert_eq!(config.concurrency, 3);
		assert_eq!(problems.len(), 1, "{problems:?}");
		assert!(problems[0].contains("config.json.bad-"), "{problems:?}");
		assert_eq!(kept(&dir), ["not json"]);
		let saved: serde_json::Value =
			serde_json::from_str(&std::fs::read_to_string(&primary).unwrap()).unwrap();
		assert_eq!(saved, json(&config));
	}

	#[test]
	fn saving_replaces_the_file_and_leaves_nothing_beside_it() {
		let dir = TempDir::new("config");
		let path = dir.join("config.json");
		std::fs::write(&path, "old").unwrap();
		let mut config = Config {
			concurrency: 1,
			..Default::default()
		};
		config.save_to(&path).unwrap();
		assert_eq!(load(Some(path.clone()), None).concurrency, 1);
		let files: Vec<_> = std::fs::read_dir(&*dir).unwrap().collect();
		assert_eq!(files.len(), 1);

		// a failed save does not leave its half-made file either
		let blocked = dir.join("blocked.json");
		std::fs::create_dir_all(&blocked).unwrap();
		let err = config.save_to(&blocked).unwrap_err();
		// and says which file it could not save
		assert!(err.contains("blocked.json"), "{err}");
		assert_eq!(names(&dir), ["blocked.json", "config.json"]);
	}

	#[test]
	fn a_failed_carry_over_is_reported() {
		let dir = TempDir::new("config");
		let legacy = dir.join("old.json");
		std::fs::write(&legacy, r#"{"concurrency": 3}"#).unwrap();
		// the new file cannot be made inside a file
		std::fs::write(dir.join("file"), "").unwrap();
		let primary = dir.join("file").join("config.json");
		let (config, problems) = Config::load_from(Some(primary), Some(legacy));
		// the old settings still load
		assert_eq!(config.concurrency, 3);
		assert!(
			problems
				.iter()
				.any(|p| p.starts_with("Could not save the settings")),
			"{problems:?}"
		);
	}

	/// A settings file that cannot be read nor moved aside (here another
	/// program has it open without allowing it to be moved) is not saved
	/// over, by the carry-over or by any later save, until it has been
	/// moved aside (once, a failed move aside left the next save free to
	/// replace it: bug B08).
	#[cfg(windows)]
	#[test]
	fn a_file_that_cannot_be_moved_aside_is_not_saved_over() {
		use std::os::windows::fs::OpenOptionsExt;
		const FILE_SHARE_READ: u32 = 1;
		let dir = TempDir::new("config");
		let primary = dir.join("config.json");
		let legacy = dir.join("old.json");
		std::fs::write(&primary, "not json").unwrap();
		std::fs::write(&legacy, r#"{"concurrency": 3}"#).unwrap();
		let held = std::fs::OpenOptions::new()
			.read(true)
			.share_mode(FILE_SHARE_READ)
			.open(&primary)
			.unwrap();

		let (mut config, problems) = Config::load_from(Some(primary.clone()), Some(legacy));
		assert_eq!(config.concurrency, 3);
		assert!(
			problems
				.iter()
				.any(|p| p.contains("nor move the file aside")),
			"{problems:?}"
		);
		assert_eq!(std::fs::read_to_string(&primary).unwrap(), "not json");
		config.theme = ThemeMode::Light;
		let err = config.save_to(&primary).unwrap_err();
		assert!(err.starts_with("Could not save the settings"), "{err}");

		// once nothing holds it, the next save moves it aside first
		drop(held);
		assert_eq!(std::fs::read_to_string(&primary).unwrap(), "not json");
		config.save_to(&primary).unwrap();
		assert_eq!(kept(&dir), ["not json"]);
		assert_eq!(load(Some(primary.clone()), None).theme, ThemeMode::Light);
		// and from then on saves replace the file as usual
		config.concurrency = 4;
		config.save_to(&primary).unwrap();
		assert_eq!(load(Some(primary), None).concurrency, 4);
		assert_eq!(kept(&dir), ["not json"]);
	}

	/// The names of the files in `dir`, sorted.
	fn names(dir: &Path) -> Vec<String> {
		let mut names: Vec<String> = std::fs::read_dir(dir)
			.unwrap()
			.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
			.collect();
		names.sort();
		names
	}

	/// Two apps saving at once each write a file of their own, so each
	/// replaces the settings with a whole file of its own (once both wrote
	/// config.json.tmp, and the second's rename took the first's file).
	#[test]
	fn saves_at_the_same_time_do_not_share_a_file() {
		let dir = TempDir::new("config");
		let path = dir.join("config.json");
		let mut first = Config {
			concurrency: 1,
			..Default::default()
		};
		let mut second = Config {
			concurrency: 2,
			..Default::default()
		};
		first
			.save_with(
				&path,
				|| {},
				|| {
					// the other app saves between this one writing its file and
					// moving it into place
					second.save_with(&path, || {}, || {}).unwrap();
					assert_eq!(load(Some(path.clone()), None).concurrency, 2);
					// and this one's file is still there, whole
					let waiting: Vec<_> = names(&dir)
						.into_iter()
						.filter(|n| n.ends_with(".tmp"))
						.collect();
					assert_eq!(waiting.len(), 1, "{waiting:?}");
					let text = std::fs::read_to_string(dir.join(&waiting[0])).unwrap();
					assert_eq!(
						serde_json::from_str::<Config>(&text).unwrap().concurrency,
						1
					);
				},
			)
			.unwrap();
		assert_eq!(load(Some(path.clone()), None).concurrency, 1);
		assert_eq!(names(&dir), ["config.json"]);

		// a save that fails removes its own file and no other
		let other = dir.join("config.json.1-0.tmp");
		std::fs::write(&other, "another app's").unwrap();
		std::fs::remove_file(&path).unwrap();
		std::fs::create_dir(&path).unwrap();
		first.save_to(&path).unwrap_err();
		assert_eq!(names(&dir), ["config.json", "config.json.1-0.tmp"]);
	}

	/// What each kept copy beside `dir`'s config.json holds, sorted.
	fn kept(dir: &Path) -> Vec<String> {
		let mut kept: Vec<String> = names(dir)
			.into_iter()
			.filter(|n| n.starts_with("config.json.bad"))
			.map(|n| std::fs::read_to_string(dir.join(n)).unwrap())
			.collect();
		kept.sort();
		kept
	}

	/// The concurrency in `text`, if it is a whole settings file.
	fn concurrency(text: &str) -> Option<usize> {
		Some(serde_json::from_str::<Config>(text).ok()?.concurrency)
	}

	/// Two apps that both found the settings unreadable each keep what they
	/// find before saving, and neither keeps it over the other's: here the
	/// other app recovers and saves while this one is about to keep the file
	/// (once this one moved the other's new settings over the kept file, and
	/// the original was lost; later it removed them without keeping them).
	#[test]
	fn two_recovering_saves_both_keep_the_unreadable_file() {
		let dir = TempDir::new("config");
		let path = dir.join("config.json");
		std::fs::write(&path, "not json").unwrap();
		let recovering = |concurrency| Config {
			concurrency,
			unreadable: Some(path.clone()),
			..Default::default()
		};
		let (mut first, mut second) = (recovering(1), recovering(2));
		first
			.save_with(
				&path,
				|| {
					second.save_to(&path).unwrap();
					assert_eq!(load(Some(path.clone()), None).concurrency, 2);
				},
				|| {},
			)
			.unwrap();
		assert_eq!(load(Some(path.clone()), None).concurrency, 1);
		// the original, and the other app's settings this one moved aside
		let kept = kept(&dir);
		assert_eq!(kept.len(), 2, "{kept:?}");
		assert_eq!(kept[0], "not json");
		assert_eq!(concurrency(&kept[1]), Some(2), "{kept:?}");
		assert_eq!(names(&dir).len(), 3, "{:?}", names(&dir));
	}

	/// The other app recovers and saves while this one is about to keep the
	/// file, and then this one's own save fails: the other app's settings are
	/// still there, kept whole beside the settings path (once this one
	/// removed them, having kept only the original, so they were lost).
	#[test]
	fn a_recovery_that_fails_to_save_keeps_the_other_apps_settings() {
		let dir = TempDir::new("config");
		let path = dir.join("config.json");
		std::fs::write(&path, "not json").unwrap();
		let recovering = |concurrency| Config {
			concurrency,
			unreadable: Some(path.clone()),
			..Default::default()
		};
		let (mut first, mut second) = (recovering(1), recovering(2));
		first
			.save_with(
				&path,
				|| second.save_to(&path).unwrap(),
				// a folder in the way stops this one's file being moved in
				|| std::fs::create_dir(&path).unwrap(),
			)
			.unwrap_err();
		let kept = kept(&dir);
		assert_eq!(kept.len(), 2, "{kept:?}");
		assert_eq!(kept[0], "not json");
		assert_eq!(
			concurrency(&kept[1]),
			Some(2),
			"the other app's settings were lost: {kept:?}"
		);
	}
}
