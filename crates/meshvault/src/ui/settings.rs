//! Roblox access dialog: credentials, place id and download concurrency.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{self, RichText};

use crate::config::{self, Config};
use crate::ui::icons;
use crate::ui::theme::palette;
use crate::ui::widgets;
use crate::workspace::AssetKind;
use crate::workspace::download::{
	AuthMethod, Client, Credentials, Endpoints, MAX_CONCURRENCY_SETTING, sniff_extension,
};

pub struct SettingsDialog {
	method: AuthMethod,
	/// Newly typed secret; empty means "keep the saved one".
	secret: String,
	concurrency: usize,
	test_id: String,
	/// The workspace asset offered for testing, and the place it came from,
	/// sent when that asset is the one tested.
	test_asset: Option<(u64, Option<u64>)>,
	test: Option<Receiver<Result<String, String>>>,
	message: Option<Message>,
	/// Shown at the top, e.g. why the dialog was opened.
	pub notice: Option<String>,
	/// Cached to avoid querying the credential store every frame.
	saved: Saved,
	/// "Forget" was clicked; waiting for the inline confirmation.
	confirm_forget: bool,
}

/// The line under the fields: how the last action went.
enum Message {
	Ok(String),
	Err(String),
}

/// Whether a credential is saved for each method.
#[derive(Clone, Copy)]
struct Saved {
	api_key: bool,
	cookie: bool,
}

impl Saved {
	fn get(self, method: AuthMethod) -> bool {
		match method {
			AuthMethod::ApiKey => self.api_key,
			AuthMethod::Cookie => self.cookie,
		}
	}

	fn set(&mut self, method: AuthMethod, saved: bool) {
		match method {
			AuthMethod::ApiKey => self.api_key = saved,
			AuthMethod::Cookie => self.cookie = saved,
		}
	}
}

/// Every field shares one width, so inputs and buttons line up.
const FIELD: f32 = 320.0;

/// A field of the grid, laid out right to left in the shared width.
fn field(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
	ui.allocate_ui_with_layout(
		egui::vec2(FIELD, ui.spacing().interact_size.y),
		egui::Layout::right_to_left(egui::Align::Center),
		add,
	);
}

/// Builds the download client from saved settings, if a credential is saved.
pub fn client_from_config(config: &Config) -> Result<Option<Arc<Client>>, String> {
	let Some(secret) = config::load_secret(config.auth_method)? else {
		return Ok(None);
	};
	Ok(Some(Arc::new(Client::new(
		Credentials {
			method: config.auth_method,
			secret,
		},
		Endpoints::default(),
	))))
}

impl SettingsDialog {
	/// `test_asset` is an asset to offer for the connection test, with the
	/// place it was exported from.
	pub fn new(config: &Config, test_asset: Option<(u64, Option<u64>)>) -> Self {
		// a store that cannot be read shows why, rather than looking empty
		let mut message = None;
		let mut is_saved = |method| {
			config::load_secret(method)
				.inspect_err(|e| message = Some(Message::Err(e.clone())))
				.is_ok_and(|s| s.is_some())
		};
		let saved = Saved {
			api_key: is_saved(AuthMethod::ApiKey),
			cookie: is_saved(AuthMethod::Cookie),
		};
		Self {
			method: config.auth_method,
			secret: String::new(),
			concurrency: config.concurrency,
			test_id: test_asset.map(|(id, _)| id.to_string()).unwrap_or_default(),
			test_asset,
			test: None,
			message,
			notice: None,
			confirm_forget: false,
			saved,
		}
	}

	fn save(&mut self, config: &mut Config) -> Result<(), String> {
		if !self.secret.trim().is_empty() {
			config::save_secret(self.method, &self.secret)?;
			self.saved.set(self.method, true);
			self.secret.clear();
		}
		config.auth_method = self.method;
		config.concurrency = self.concurrency;
		config.save()
	}

	fn start_test(&mut self, ctx: &egui::Context) {
		let secret = if self.secret.trim().is_empty() {
			config::load_secret(self.method)
		} else {
			Ok(Some(self.secret.clone()))
		};
		let secret = match secret {
			Ok(Some(secret)) => secret,
			Ok(None) => {
				self.message = Some(Message::Err("Enter a credential first.".into()));
				return;
			}
			Err(e) => {
				self.message = Some(Message::Err(e));
				return;
			}
		};
		let Some(id) = crate::content_id::parse_asset_id(&self.test_id) else {
			self.message = Some(Message::Err("Enter an asset id to test with.".into()));
			return;
		};
		let client = Client::new(
			Credentials {
				method: self.method,
				secret,
			},
			Endpoints::default(),
		);
		// the offered asset is fetched as its downloads are, from its place
		let place_id = self
			.test_asset
			.and_then(|(asset, place)| (asset == id).then_some(place).flatten());
		let (tx, rx) = channel();
		let ctx = ctx.clone();
		std::thread::spawn(move || {
			let result = client
				.fetch(id, false, place_id)
				.map(|bytes| {
					let kind = sniff_extension(&bytes, AssetKind::Mesh);
					let header = String::from_utf8_lossy(&bytes[..bytes.len().min(12)])
						.trim()
						.to_string();
					let what = if kind == "mesh" {
						header
					} else {
						format!(".{kind} file")
					};
					format!("Success: downloaded {} bytes ({what}).", bytes.len())
				})
				.map_err(|e| e.to_string());
			let _ = tx.send(result);
			ctx.request_repaint();
		});
		self.test = Some(rx);
		self.message = None;
	}

	/// Takes the test's result once it has arrived, before the frame draws.
	fn poll_test(&mut self) {
		if let Some(rx) = &self.test
			&& let Ok(result) = rx.try_recv()
		{
			self.message = Some(match result {
				Ok(m) => Message::Ok(m),
				Err(e) => Message::Err(e),
			});
			self.test = None;
		}
	}

	/// Returns false when the dialog should close.
	pub fn show(&mut self, ctx: &egui::Context, config: &mut Config) -> bool {
		self.poll_test();
		let mut close = false;
		let modal = widgets::modal(ctx, "settings_dialog").show(ctx, |ui| {
			ui.set_width(500.0);
			let accent = palette(ui.ctx()).accent;
			widgets::dialog_title(ui, Some((icons::KEY, 20.0, accent)), "Roblox access");
			ui.add_space(4.0);
			if let Some(notice) = &self.notice {
				ui.label(RichText::new(notice).color(palette(ui.ctx()).warning));
				ui.add_space(4.0);
			}
			self.method_choice(ui);
			ui.add_space(6.0);
			self.fields_grid(ui);
			if let Some(message) = &self.message {
				ui.add_space(4.0);
				let p = palette(ui.ctx());
				let (colour, text) = match message {
					Message::Ok(text) => (p.success, text),
					Message::Err(text) => (p.danger, text),
				};
				ui.label(RichText::new(text).color(colour));
			}
			ui.add_space(6.0);
			close = self.footer(ui, config);
		});
		!modal.should_close() && !close
	}

	/// The credential kind to use, with how to get one.
	fn method_choice(&mut self, ui: &mut egui::Ui) {
		ui.label("Roblox only delivers assets to signed-in users, so downloads need a credential.");
		ui.add_space(6.0);
		let api = widgets::pointer(ui.radio_value(
			&mut self.method,
			AuthMethod::ApiKey,
			"Open Cloud API key (recommended)",
		));
		let cookie = widgets::pointer(ui.radio_value(
			&mut self.method,
			AuthMethod::Cookie,
			".ROBLOSECURITY cookie",
		));
		// a pending "forget" belongs to the credential it was asked about
		if api.changed() || cookie.changed() {
			self.confirm_forget = false;
		}
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
					ui.label(
						"permission. Keys made for a group cannot use this permission; use one for your own account.",
					);
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
	}

	/// The credential, concurrency and test fields.
	fn fields_grid(&mut self, ui: &mut egui::Ui) {
		// read once, so the whole frame shows the state it started with
		let saved = self.saved.get(self.method);
		egui::Grid::new("settings")
			.num_columns(2)
			.spacing(egui::vec2(16.0, 10.0))
			.show(ui, |ui| {
				ui.label(match self.method {
					AuthMethod::ApiKey => "API key",
					AuthMethod::Cookie => "Cookie",
				});
				field(ui, |ui| self.credential_row(ui, saved));
				ui.end_row();

				ui.label("Parallel downloads");
				field(ui, |ui| {
					// right to left like the other fields: the number box ends
					// on the shared right edge and the slider fills the rest
					let height = ui.spacing().interact_size.y;
					widgets::pointer(
						ui.add_sized(
							[48.0, height],
							egui::DragValue::new(&mut self.concurrency)
								.range(1..=MAX_CONCURRENCY_SETTING),
						),
					);
					ui.spacing_mut().slider_width =
						ui.available_width() - ui.spacing().item_spacing.x;
					widgets::pointer(
						ui.add(
							egui::Slider::new(&mut self.concurrency, 1..=MAX_CONCURRENCY_SETTING)
								.show_value(false),
						),
					);
				});
				ui.end_row();

				ui.label("Test with asset id");
				field(ui, |ui| {
					if self.test.is_some() {
						ui.spinner();
					} else if widgets::text_button(ui, "Test").clicked() {
						self.start_test(ui.ctx());
					}
					ui.add(widgets::input(&mut self.test_id).desired_width(ui.available_width()));
				});
				ui.end_row();
			});
	}

	/// The credential field: the input with a Forget button while one is
	/// saved, or the inline confirmation in place of both. `saved` is as it
	/// was when the frame began.
	fn credential_row(&mut self, ui: &mut egui::Ui, saved: bool) {
		// laid out right to left: the buttons first, then the input
		// or the confirmation fills the rest
		if saved && self.confirm_forget {
			let what = match self.method {
				AuthMethod::ApiKey => "key",
				AuthMethod::Cookie => "cookie",
			};
			if widgets::danger_button(ui, "Forget").clicked() {
				self.forgot(config::delete_secret(self.method));
			}
			if widgets::text_button(ui, "Keep").clicked() {
				self.confirm_forget = false;
			}
			ui.label(
				RichText::new(format!("Delete the saved {what}?")).color(palette(ui.ctx()).danger),
			);
			// the confirmation replaces the input in this field only
			return;
		}
		if saved {
			let p = palette(ui.ctx());
			let forget = ui.add(
				egui::Button::new(RichText::new("Forget").color(p.danger))
					.stroke(egui::Stroke::new(1.0, p.danger.gamma_multiply(0.5))),
			);
			if forget
				.on_hover_text("Delete the saved credential")
				.clicked()
			{
				self.confirm_forget = true;
			}
		}
		ui.add(
			widgets::input(&mut self.secret)
				.password(true)
				.desired_width(ui.available_width())
				.hint_text(if saved {
					"saved (type to replace)"
				} else {
					"paste here"
				}),
		);
	}

	/// Shows how deleting the saved credential went. One that may still be
	/// saved keeps its Forget button.
	fn forgot(&mut self, deleted: Result<(), String>) {
		self.confirm_forget = false;
		self.message = Some(match deleted {
			Ok(()) => {
				self.saved.set(self.method, false);
				Message::Ok("Saved credential removed.".into())
			}
			Err(e) => Message::Err(e),
		});
	}

	/// Save and Cancel, laid out right to left. Returns true to close.
	fn footer(&mut self, ui: &mut egui::Ui, config: &mut Config) -> bool {
		let mut close = false;
		ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
			if widgets::primary_button(ui, "Save").clicked() {
				match self.save(config) {
					Ok(()) => close = true,
					Err(e) => self.message = Some(Message::Err(e)),
				}
			}
			if widgets::text_button(ui, "Cancel").clicked() {
				close = true;
			}
		});
		close
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn dialog() -> SettingsDialog {
		SettingsDialog {
			method: AuthMethod::ApiKey,
			secret: String::new(),
			concurrency: 1,
			test_id: String::new(),
			test_asset: None,
			test: None,
			message: None,
			notice: None,
			saved: Saved {
				api_key: true,
				cookie: true,
			},
			confirm_forget: true,
		}
	}

	#[test]
	fn forget_says_removed_only_when_the_delete_worked() {
		let mut ok = dialog();
		ok.forgot(Ok(()));
		assert!(!ok.saved.get(AuthMethod::ApiKey) && ok.saved.get(AuthMethod::Cookie));
		assert!(matches!(&ok.message, Some(Message::Ok(m)) if m == "Saved credential removed."));
		assert!(!ok.confirm_forget);

		let mut failed = dialog();
		failed.forgot(Err(
			"Could not remove from the system credential store: denied".into(),
		));
		assert!(failed.saved.get(AuthMethod::ApiKey));
		assert!(matches!(&failed.message, Some(Message::Err(m)) if m.ends_with("denied")));
		assert!(!failed.confirm_forget);
	}
}
