//! Transient notifications in the bottom-right corner. Errors stay until
//! dismissed; everything else fades out on its own.

use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, CornerRadius, Frame, Margin, RichText, Stroke, Vec2};

use super::icons;
use super::theme::palette;

const LIFETIME: Duration = Duration::from_secs(5);
const MAX_SHOWN: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
	Info,
	Success,
	Error,
}

struct Toast {
	level: Level,
	text: String,
	created: Instant,
}

#[derive(Default)]
pub struct Toasts {
	list: Vec<Toast>,
}

impl Toasts {
	pub fn push(&mut self, level: Level, text: impl Into<String>) {
		let text = text.into();
		// repeating the same message just refreshes it
		self.list.retain(|t| t.text != text);
		self.list.push(Toast {
			level,
			text,
			created: Instant::now(),
		});
	}
	pub fn info(&mut self, text: impl Into<String>) {
		self.push(Level::Info, text);
	}
	pub fn success(&mut self, text: impl Into<String>) {
		self.push(Level::Success, text);
	}
	pub fn error(&mut self, text: impl Into<String>) {
		self.push(Level::Error, text);
	}

	pub fn show(&mut self, ctx: &egui::Context) {
		self.list
			.retain(|t| t.level == Level::Error || t.created.elapsed() < LIFETIME);
		if self.list.is_empty() {
			return;
		}
		let p = palette(ctx);
		let mut dismiss = None;
		let start = self.list.len().saturating_sub(MAX_SHOWN);
		egui::Area::new(egui::Id::unique("toasts"))
			.anchor(Align2::RIGHT_BOTTOM, Vec2::new(-16.0, -40.0))
			.order(egui::Order::Foreground)
			.interactable(true)
			.show(ctx, |ui| {
				ui.set_max_width(380.0);
				for (i, toast) in self.list.iter().enumerate().skip(start) {
					let (icon, colour) = match toast.level {
						Level::Info => (icons::INFO, p.accent),
						Level::Success => (icons::CHECK_CIRCLE, p.success),
						Level::Error => (icons::WARNING_CIRCLE, p.danger),
					};
					// fade out over the last half second
					let left = LIFETIME
						.saturating_sub(toast.created.elapsed())
						.as_secs_f32();
					let alpha = if toast.level == Level::Error {
						1.0
					} else {
						(left / 0.5).min(1.0)
					};
					ui.set_opacity(alpha);
					Frame::NONE
						.fill(p.raised)
						.stroke(Stroke::new(1.0, p.border))
						.corner_radius(CornerRadius::same(10))
						.inner_margin(Margin::symmetric(12, 10))
						.shadow(ctx.global_style().visuals.popup_shadow)
						.show(ui, |ui| {
							ui.horizontal(|ui| {
								ui.label(RichText::new(icon).size(18.0).color(colour));
								ui.add(egui::Label::new(&toast.text).wrap());
								if ui
									.add(
										egui::Button::new(
											RichText::new(icons::X).color(p.text_weak),
										)
										.frame(false),
									)
									.on_hover_text("Dismiss")
									.clicked()
								{
									dismiss = Some(i);
								}
							});
						});
					ui.add_space(6.0);
				}
			});
		if let Some(i) = dismiss {
			self.list.remove(i);
		}
		// keep animating the fade-out
		if self.list.iter().any(|t| t.level != Level::Error) {
			ctx.request_repaint_after(Duration::from_millis(100));
		}
	}
}
