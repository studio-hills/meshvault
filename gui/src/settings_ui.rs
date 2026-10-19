//! Settings dialog for Roblox download credentials.

use std::sync::mpsc::{Receiver, channel};
use std::sync::Arc;

use eframe::egui::{self, RichText};

use crate::config::{self, Config};
use crate::download::{AuthMethod, Client, Credentials, Endpoints, sniff_extension};
use crate::workspace::AssetKind;

pub struct SettingsDialog {
	method: AuthMethod,
	/// Newly typed secret; empty means "keep the saved one".
	secret: String,
	place_id: String,
	concurrency: usize,
	test_id: String,
	test: Option<Receiver<Result<String, String>>>,
	message: Option<(bool, String)>,
	/// Shown at the top, e.g. why the dialog was opened.
	pub notice: Option<String>,
	/// Whether a credential is saved for [API key, cookie]; cached to avoid
	/// querying the credential store every frame.
	saved: [bool; 2],
}

fn slot(method: AuthMethod) -> usize {
	match method {
		AuthMethod::ApiKey => 0,
		AuthMethod::Cookie => 1,
	}
}

/// Builds the download client from saved settings, if a credential is saved.
pub fn client_from_config(config: &Config) -> Option<Arc<Client>> {
	let secret = config::load_secret(config.auth_method)?;
	Some(Arc::new(Client::new(
		Credentials {
			method: config.auth_method,
			secret,
			place_id: config.place_id,
		},
		Endpoints::default(),
	)))
}

impl SettingsDialog {
	pub fn new(config: &Config, test_id: Option<u64>) -> Self {
		Self {
			method: config.auth_method,
			secret: String::new(),
			place_id: config.place_id.map(|p| p.to_string()).unwrap_or_default(),
			concurrency: config.concurrency,
			test_id: test_id.map(|i| i.to_string()).unwrap_or_default(),
			test: None,
			message: None,
			notice: None,
			saved: [
				config::load_secret(AuthMethod::ApiKey).is_some(),
				config::load_secret(AuthMethod::Cookie).is_some(),
			],
		}
	}

	fn place_id(&self) -> Result<Option<u64>, String> {
		let s = self.place_id.trim();
		if s.is_empty() {
			return Ok(None);
		}
		s.parse().map(Some).map_err(|_| "Place ID must be a number".into())
	}

	fn save(&mut self, config: &mut Config) -> Result<(), String> {
		let place_id = self.place_id()?;
		if !self.secret.trim().is_empty() {
			config::save_secret(self.method, &self.secret)?;
			self.saved[slot(self.method)] = true;
			self.secret.clear();
		}
		config.auth_method = self.method;
		config.place_id = place_id;
		config.concurrency = self.concurrency;
		config.save();
		Ok(())
	}

	fn start_test(&mut self, ctx: &egui::Context) {
		let place_id = match self.place_id() {
			Ok(p) => p,
			Err(e) => {
				self.message = Some((false, e));
				return;
			}
		};
		let secret = if self.secret.trim().is_empty() {
			config::load_secret(self.method)
		} else {
			Some(self.secret.clone())
		};
		let Some(secret) = secret else {
			self.message = Some((false, "Enter a credential first.".into()));
			return;
		};
		let Some(id) = crate::export::parse_asset_id(&self.test_id) else {
			self.message = Some((false, "Enter an asset id to test with.".into()));
			return;
		};
		let client = Client::new(
			Credentials {
				method: self.method,
				secret,
				place_id,
			},
			Endpoints::default(),
		);
		let (tx, rx) = channel();
		let ctx = ctx.clone();
		std::thread::spawn(move || {
			let result = client
				.fetch(id, false)
				.map(|bytes| {
					let kind = sniff_extension(&bytes, AssetKind::Mesh);
					let header = String::from_utf8_lossy(&bytes[..bytes.len().min(12)])
						.trim()
						.to_string();
					let what = if kind == "mesh" { header } else { format!(".{kind} file") };
					format!("Success: downloaded {} bytes ({what}).", bytes.len())
				})
				.map_err(|e| e.to_string());
			let _ = tx.send(result);
			ctx.request_repaint();
		});
		self.test = Some(rx);
		self.message = None;
	}

	/// Returns false when the dialog should close.
	pub fn show(&mut self, ctx: &egui::Context, config: &mut Config) -> bool {
		if let Some(rx) = &self.test
			&& let Ok(result) = rx.try_recv()
		{
			self.message = Some(match result {
				Ok(m) => (true, m),
				Err(e) => (false, e),
			});
			self.test = None;
		}

		let mut open = true;
		let mut close = false;
		egui::Window::new("Roblox access")
			.open(&mut open)
			.collapsible(false)
			.resizable(false)
			.default_width(480.0)
			.show(ctx, |ui| {
				if let Some(notice) = &self.notice {
					ui.label(RichText::new(notice).strong());
					ui.add_space(4.0);
				}
				ui.label("Roblox only delivers assets to signed-in users, so downloads need a credential.");
				ui.add_space(6.0);
				ui.radio_value(&mut self.method, AuthMethod::ApiKey, "Open Cloud API key (recommended)");
				ui.radio_value(&mut self.method, AuthMethod::Cookie, ".ROBLOSECURITY cookie");
				ui.add_space(4.0);
				match self.method {
					AuthMethod::ApiKey => {
						ui.horizontal_wrapped(|ui| {
							ui.label("Create a key at");
							ui.hyperlink_to(
								"create.roblox.com → Credentials",
								"https://create.roblox.com/dashboard/credentials",
							);
							ui.label("and give it the");
							ui.code("legacy-asset:manage");
							ui.label("permission. Keys made for a group cannot use this permission; use one for your own account.");
						});
					}
					AuthMethod::Cookie => {
						ui.label(
							RichText::new(
								"Your .ROBLOSECURITY cookie gives full access to your account. Only use \
								it on a computer you trust. It is stored in the system credential \
								store, never in a file, and only sent to roblox.com.",
							)
							.color(ui.visuals().warn_fg_color),
						);
					}
				}
				ui.add_space(6.0);
				let saved = self.saved[slot(self.method)];
				egui::Grid::new("settings").num_columns(2).show(ui, |ui| {
					ui.label(match self.method {
						AuthMethod::ApiKey => "API key",
						AuthMethod::Cookie => "Cookie",
					});
					ui.horizontal(|ui| {
						ui.add(
							egui::TextEdit::singleline(&mut self.secret)
								.password(true)
								.desired_width(260.0)
								.hint_text(if saved { "saved (type to replace)" } else { "paste here" }),
						);
						if saved && ui.small_button("Forget").clicked() {
							config::delete_secret(self.method);
							self.saved[slot(self.method)] = false;
							self.message = Some((true, "Saved credential removed.".into()));
						}
					});
					ui.end_row();
					ui.label("Place ID");
					ui.add(
						egui::TextEdit::singleline(&mut self.place_id)
							.desired_width(260.0)
							.hint_text("optional"),
					)
					.on_hover_text(
						"A place you can edit. Sent as Roblox-Place-Id, which can grant access \
						to private assets used in that place.",
					);
					ui.end_row();
					ui.label("Parallel downloads");
					ui.add(egui::Slider::new(&mut self.concurrency, 1..=8));
					ui.end_row();
					ui.label("Test with asset id");
					ui.horizontal(|ui| {
						ui.add(egui::TextEdit::singleline(&mut self.test_id).desired_width(180.0));
						if self.test.is_some() {
							ui.spinner();
						} else if ui.button("Test").clicked() {
							self.start_test(ui.ctx());
						}
					});
					ui.end_row();
				});
				if let Some((ok, message)) = &self.message {
					ui.add_space(4.0);
					let colour = if *ok {
						egui::Color32::from_rgb(110, 200, 120)
					} else {
						ui.visuals().error_fg_color
					};
					ui.label(RichText::new(message).color(colour));
				}
				ui.add_space(6.0);
				ui.horizontal(|ui| {
					if ui.button("Save").clicked() {
						match self.save(config) {
							Ok(()) => close = true,
							Err(e) => self.message = Some((false, e)),
						}
					}
					if ui.button("Cancel").clicked() {
						close = true;
					}
				});
			});
		open && !close
	}
}
