//! Transient notifications in the bottom-right corner. Errors and warnings
//! stay until dismissed; everything else fades out on its own. A toast
//! rises into the stack from its bottom edge and leaves the way it came, so
//! the toasts above it glide rather than jump.

use std::time::{Duration, Instant};

use eframe::egui::{
	self, Align2, CornerRadius, Frame, Margin, Pos2, Rect, RichText, Sense, Stroke, UiBuilder, Vec2,
};
use eframe::emath::easing;

use super::theme::{BODY_LINE_HEIGHT, palette};
use super::{icons, widgets};

const LIFETIME: Duration = Duration::from_secs(5);
const MAX_SHOWN: usize = 5;
/// The widest a toast grows before its text wraps.
const MAX_WIDTH: f32 = 380.0;
/// Where the stack sits from the window's bottom-right corner: 40 clears
/// the status bar.
const ANCHOR_OFFSET: Vec2 = Vec2::new(-16.0, -40.0);
/// Space between toasts.
const GAP: f32 = 6.0;
/// How long a toast takes to rise in, to fade out at the end of its life,
/// to give back its room in the stack after that, and to leave when
/// dismissed.
const ENTER: Duration = Duration::from_millis(200);
const FADE_OUT: Duration = Duration::from_millis(500);
const COLLAPSE: Duration = Duration::from_millis(150);
const EXIT: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
	Info,
	Success,
	Warning,
	Error,
}

impl Level {
	fn stays(self) -> bool {
		matches!(self, Level::Warning | Level::Error)
	}
}

struct Toast {
	level: Level,
	text: String,
	created: Instant,
	/// When its close button was pressed: it is leaving.
	dismissed: Option<Instant>,
}

/// How present a toast is, from 0 to 1: its opacity, and how much of its
/// room in the stack it takes. `age` is how long it has been shown, and
/// `leaving` how long since it was dismissed.
fn presence(stays: bool, age: Duration, leaving: Option<Duration>) -> (f32, f32) {
	let progress = |d: Duration, of: Duration| (d.as_secs_f32() / of.as_secs_f32()).clamp(0.0, 1.0);
	let enter = easing::cubic_out(progress(age, ENTER));
	let (fade, collapse) = if stays {
		(1.0, 1.0)
	} else {
		let left = LIFETIME.saturating_sub(age);
		(
			progress(left, FADE_OUT),
			easing::cubic_out(progress(left, COLLAPSE)),
		)
	};
	let out = leaving.map_or(1.0, |d| 1.0 - easing::cubic_out(progress(d, EXIT)));
	(enter * fade * out, enter * collapse * out)
}

#[derive(Default)]
pub struct Toasts {
	list: Vec<Toast>,
}

impl Toasts {
	fn push(&mut self, level: Level, text: impl Into<String>) {
		let text = text.into();
		// repeating the same message just refreshes it
		self.list.retain(|t| t.text != text);
		self.list.push(Toast {
			level,
			text,
			created: Instant::now(),
			dismissed: None,
		});
	}
	pub fn info(&mut self, text: impl Into<String>) {
		self.push(Level::Info, text);
	}
	pub fn success(&mut self, text: impl Into<String>) {
		self.push(Level::Success, text);
	}
	pub fn warning(&mut self, text: impl Into<String>) {
		self.push(Level::Warning, text);
	}
	pub fn error(&mut self, text: impl Into<String>) {
		self.push(Level::Error, text);
	}

	pub fn show(&mut self, ctx: &egui::Context) {
		self.list.retain(|t| {
			let timed_out = !t.level.stays() && t.created.elapsed() >= LIFETIME;
			let gone = t.dismissed.is_some_and(|d| d.elapsed() >= EXIT);
			!timed_out && !gone
		});
		if self.list.is_empty() {
			return;
		}
		let p = palette(ctx);
		let shadow = ctx.global_style().visuals.popup_shadow;
		let mut dismiss = None;
		let start = self.list.len().saturating_sub(MAX_SHOWN);
		egui::Area::new(egui::Id::unique("toasts"))
			.anchor(Align2::RIGHT_BOTTOM, ANCHOR_OFFSET)
			.order(egui::Order::Foreground)
			.interactable(true)
			.show(ctx, |ui| {
				ui.set_max_width(MAX_WIDTH);
				for (i, toast) in self.list.iter().enumerate().skip(start) {
					let (icon, colour) = match toast.level {
						Level::Info => (icons::INFO, p.accent),
						Level::Success => (icons::CHECK_CIRCLE, p.success),
						Level::Warning => (icons::WARNING, p.warning),
						Level::Error => (icons::WARNING_CIRCLE, p.danger),
					};
					let draw = |ui: &mut egui::Ui| {
						let mut closed = false;
						let frame = Frame::NONE
							.fill(p.raised)
							.stroke(Stroke::new(1.0, p.border))
							.corner_radius(CornerRadius::same(10))
							.inner_margin(Margin::symmetric(12, 10))
							.shadow(shadow)
							.show(ui, |ui| {
								ui.horizontal(|ui| {
									ui.spacing_mut().interact_size.y = 24.0;
									ui.label(RichText::new(icon).size(18.0).color(colour));
									ui.add(
										egui::Label::new(
											RichText::new(&toast.text)
												.line_height(Some(BODY_LINE_HEIGHT)),
										)
										.wrap(),
									);
									closed =
										widgets::icon_button(ui, icons::X, "Dismiss").clicked();
								});
							});
						(frame.response.rect.size(), closed)
					};
					let (opacity, room) = presence(
						toast.level.stays(),
						toast.created.elapsed(),
						toast.dismissed.map(|d| d.elapsed()),
					);
					// its full size, measured without taking any room
					let free = ui.available_rect_before_wrap();
					let mut probe =
						ui.new_child(UiBuilder::new().max_rect(free).sizing_pass().invisible());
					let (size, _) = draw(&mut probe);
					// the room it takes grows (and shrinks) with it, and it sits
					// on the room's bottom: it rises in from below, and the toasts
					// above move up with the room instead of jumping
					let full = size.y + GAP;
					let (slot, _) =
						ui.allocate_exact_size(Vec2::new(size.x, full * room), Sense::hover());
					let at = Rect::from_min_size(
						Pos2::new(slot.left(), slot.bottom() - full),
						Vec2::new(free.width(), full),
					);
					let mut child = ui.new_child(UiBuilder::new().max_rect(at));
					// hidden above the room's top edge until it has risen in
					let below = Rect::from_min_max(
						Pos2::new(f32::NEG_INFINITY, slot.top()),
						Pos2::new(f32::INFINITY, f32::INFINITY),
					);
					child.set_clip_rect(child.clip_rect().intersect(below));
					child.set_opacity(opacity);
					if draw(&mut child).1 && toast.dismissed.is_none() {
						dismiss = Some(i);
					}
				}
			});
		if let Some(i) = dismiss {
			self.list[i].dismissed = Some(Instant::now());
		}
		// every frame while one moves; otherwise not until one starts fading
		let moving = self.list.iter().any(|t| {
			let age = t.created.elapsed();
			t.dismissed.is_some() || age < ENTER || (!t.level.stays() && age + FADE_OUT >= LIFETIME)
		});
		if moving {
			ctx.request_repaint();
		} else if let Some(next) = self
			.list
			.iter()
			.filter(|t| !t.level.stays())
			.map(|t| (LIFETIME - FADE_OUT).saturating_sub(t.created.elapsed()))
			.min()
		{
			ctx.request_repaint_after(next);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn ms(n: u64) -> Duration {
		Duration::from_millis(n)
	}

	/// A toast rises in over its first 200 ms, opacity and room together.
	#[test]
	fn toasts_rise_in() {
		assert_eq!(presence(false, ms(0), None), (0.0, 0.0));
		let (opacity, room) = presence(false, ms(100), None);
		assert!(opacity > 0.5 && opacity < 1.0, "{opacity}");
		assert_eq!(opacity, room);
		assert_eq!(presence(false, ms(200), None), (1.0, 1.0));
		assert_eq!(presence(true, ms(4_000), None), (1.0, 1.0));
	}

	/// At the end of its life a toast fades over half a second, then gives
	/// back its room over the last 150 ms; one that stays never does.
	#[test]
	fn timed_toasts_fade_then_give_back_their_room() {
		let (opacity, room) = presence(false, LIFETIME - ms(250), None);
		assert!(
			(opacity - 0.5).abs() < 1e-3 && room == 1.0,
			"{opacity} {room}"
		);
		let (_, room) = presence(false, LIFETIME - ms(75), None);
		assert!(room > 0.0 && room < 1.0);
		assert_eq!(presence(false, LIFETIME, None), (0.0, 0.0));
		assert_eq!(presence(true, LIFETIME * 10, None), (1.0, 1.0));
	}

	/// Dismissed, a toast leaves the way it came in 150 ms.
	#[test]
	fn dismissed_toasts_leave_the_way_they_came() {
		let (opacity, room) = presence(true, ms(1_000), Some(ms(75)));
		assert!(opacity > 0.0 && opacity < 1.0 && opacity == room);
		assert_eq!(presence(true, ms(1_000), Some(EXIT)), (0.0, 0.0));
	}
}
