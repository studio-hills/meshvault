//! Everything drawn on screen, built from a small design system:
//! [`theme`] (fonts, palette, spacing), [`widgets`] and [`icons`], with
//! [`window`] (the window frame) and [`tree`] (folder open/closed state)
//! shared by the screens too.

mod code;
mod dialogs;
pub(crate) mod format;
pub mod icons;
pub mod import;
mod inspector;
mod layout;
pub mod library;
mod model;
pub mod settings;
mod studio;
mod switch;
pub mod theme;
mod toasts;
pub mod tree;
mod welcome;
pub mod widgets;
pub mod window;

pub use dialogs::{Confirm, ConfirmAction};
pub use studio::{StudioDialog, unavailable_text};
pub use toasts::Toasts;
pub use welcome::paint_gradient;
