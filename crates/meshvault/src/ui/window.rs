//! The window frame Meshvault draws itself, since the system one is off:
//! caption buttons, dragging and maximizing from the top bar, and resizing
//! from the window's edges. Windows 11 rounds the corners.

use eframe::egui::{
	self, Align2, Color32, CursorIcon, Id, Order, Rect, Response, Sense, Stroke, Ui, Vec2,
	ViewportCommand, pos2, vec2,
};

use super::theme::{CONTROL_RADIUS, palette};
use super::widgets::pointer;

fn maximized(ctx: &egui::Context) -> bool {
	ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}

/// Lets the empty parts of the top bar move the window, and a double click
/// maximize or restore it. Call before adding the bar's widgets, so they
/// take the pointer where they are.
pub fn title_bar_drag(ui: &Ui, rect: Rect) {
	let response = ui.interact(rect, Id::unique("title_bar_drag"), Sense::click_and_drag());
	let ctx = ui.ctx();
	if response.double_clicked() {
		ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized(ctx)));
	} else if response.drag_started_by(egui::PointerButton::Primary) {
		ctx.send_viewport_cmd(ViewportCommand::StartDrag);
	}
}

#[derive(Clone, Copy)]
enum Caption {
	Minimize,
	Maximize,
	Restore,
	Close,
}

/// Minimize, maximize (or restore) and close, right to left as Windows has
/// them: add them first in a right-to-left layout.
pub fn caption_buttons(ui: &mut Ui) {
	let ctx = ui.ctx().clone();
	if caption_button(ui, Caption::Close, "Close").clicked() {
		ctx.send_viewport_cmd(ViewportCommand::Close);
	}
	let max = maximized(&ctx);
	let (kind, tip) = if max {
		(Caption::Restore, "Restore down")
	} else {
		(Caption::Maximize, "Maximize")
	};
	if caption_button(ui, kind, tip).clicked() {
		ctx.send_viewport_cmd(ViewportCommand::Maximized(!max));
	}
	if caption_button(ui, Caption::Minimize, "Minimize").clicked() {
		ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
	}
}

/// Width of each caption button.
const CAPTION_WIDTH: f32 = 40.0;

fn caption_button(ui: &mut Ui, kind: Caption, tooltip: &str) -> Response {
	let p = palette(ui.ctx());
	let size = vec2(CAPTION_WIDTH, ui.spacing().interact_size.y);
	let (rect, response) = ui.allocate_exact_size(size, Sense::click());
	let response = pointer(response);
	let hovered = response.hovered();
	let close = matches!(kind, Caption::Close);
	let (fill, colour) = match (hovered, close) {
		(true, true) => (p.danger, Color32::WHITE),
		(true, false) => (p.raised_hover, p.text),
		_ => (Color32::TRANSPARENT, p.text_weak),
	};
	let painter = ui.painter();
	painter.rect_filled(rect, CONTROL_RADIUS, fill);
	// thin line glyphs, 10 points across, like the system's
	let stroke = Stroke::new(1.0, colour);
	let c = rect.center();
	let r = 5.0;
	match kind {
		Caption::Minimize => {
			painter.line_segment([pos2(c.x - r, c.y), pos2(c.x + r, c.y)], stroke);
		}
		Caption::Maximize => {
			painter.rect_stroke(
				Rect::from_center_size(c, Vec2::splat(2.0 * r)),
				1.0,
				stroke,
				egui::StrokeKind::Middle,
			);
		}
		Caption::Restore => {
			// a window in front, and the edges of one behind it
			let front =
				Rect::from_min_size(pos2(c.x - r, c.y - r + 2.0), Vec2::splat(2.0 * r - 2.0));
			painter.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Middle);
			let (l, t, rr, b) = (c.x - r + 2.0, c.y - r, c.x + r, c.y + r - 2.0);
			painter.line_segment([pos2(l, t), pos2(rr, t)], stroke);
			painter.line_segment([pos2(rr, t), pos2(rr, b)], stroke);
		}
		Caption::Close => {
			painter.line_segment([pos2(c.x - r, c.y - r), pos2(c.x + r, c.y + r)], stroke);
			painter.line_segment([pos2(c.x - r, c.y + r), pos2(c.x + r, c.y - r)], stroke);
		}
	}
	response.on_hover_text(tooltip)
}

/// Invisible grips along the window's edges and corners that hand resizing
/// to the system, as the frame it no longer draws would.
pub fn resize_grips(ctx: &egui::Context) {
	if maximized(ctx) || ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)) {
		return;
	}
	use egui::ResizeDirection as D;
	const EDGE: f32 = 5.0;
	const CORNER: f32 = 12.0;
	let screen = ctx.content_rect();
	let (l, t, r, b) = (screen.left(), screen.top(), screen.right(), screen.bottom());
	let grips = [
		(
			D::North,
			Rect::from_min_max(pos2(l + CORNER, t), pos2(r - CORNER, t + EDGE)),
		),
		(
			D::South,
			Rect::from_min_max(pos2(l + CORNER, b - EDGE), pos2(r - CORNER, b)),
		),
		(
			D::West,
			Rect::from_min_max(pos2(l, t + CORNER), pos2(l + EDGE, b - CORNER)),
		),
		(
			D::East,
			Rect::from_min_max(pos2(r - EDGE, t + CORNER), pos2(r, b - CORNER)),
		),
		(
			D::NorthWest,
			Rect::from_min_size(pos2(l, t), Vec2::splat(CORNER)),
		),
		(
			D::NorthEast,
			Rect::from_min_size(pos2(r - CORNER, t), Vec2::splat(CORNER)),
		),
		(
			D::SouthWest,
			Rect::from_min_size(pos2(l, b - CORNER), Vec2::splat(CORNER)),
		),
		(
			D::SouthEast,
			Rect::from_min_size(pos2(r - CORNER, b - CORNER), Vec2::splat(CORNER)),
		),
	];
	for (i, (direction, rect)) in grips.into_iter().enumerate() {
		// above every panel and dialog, so the edge always resizes
		egui::Area::new(Id::unique(("resize_grip", i)))
			.order(Order::Foreground)
			.fixed_pos(rect.min)
			.pivot(Align2::LEFT_TOP)
			.interactable(true)
			.show(ctx, |ui| {
				let response = ui.allocate_response(rect.size(), Sense::drag());
				if response.hovered() || response.dragged() {
					ui.ctx().set_cursor_icon(match direction {
						D::North | D::South => CursorIcon::ResizeVertical,
						D::East | D::West => CursorIcon::ResizeHorizontal,
						D::NorthWest | D::SouthEast => CursorIcon::ResizeNwSe,
						D::NorthEast | D::SouthWest => CursorIcon::ResizeNeSw,
					});
				}
				if response.drag_started() {
					ui.ctx()
						.send_viewport_cmd(ViewportCommand::BeginResize(direction));
				}
			});
	}
}

/// Asks Windows 11 to round the corners of the undecorated window, as it does
/// for framed ones (older Windows ignores it).
#[cfg(windows)]
pub fn round_corners(window: &impl raw_window_handle::HasWindowHandle) {
	use raw_window_handle::RawWindowHandle;
	use windows_sys::Win32::Foundation::HWND;
	use windows_sys::Win32::Graphics::Dwm::{
		DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
	};
	let Ok(handle) = window.window_handle() else {
		return;
	};
	if let RawWindowHandle::Win32(h) = handle.as_raw() {
		let preference = DWMWCP_ROUND;
		// SAFETY: `handle` is a borrowed window handle, which promises the
		// window stays alive while it is held, and it is held through this
		// call, so the HWND is valid. `preference` is a local that outlives
		// the synchronous call, and the size passed is its own size.
		unsafe {
			// best effort: Windows 10 and earlier reject this attribute,
			// and square corners are fine there, so the HRESULT is ignored
			DwmSetWindowAttribute(
				h.hwnd.get() as HWND,
				DWMWA_WINDOW_CORNER_PREFERENCE as u32,
				std::ptr::from_ref(&preference).cast(),
				size_of_val(&preference) as u32,
			);
		}
	}
}

#[cfg(not(windows))]
pub fn round_corners(_window: &impl raw_window_handle::HasWindowHandle) {}
