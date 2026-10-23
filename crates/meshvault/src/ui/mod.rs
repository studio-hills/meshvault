//! Everything drawn on screen, built from a small design system:
//! [`theme`] (fonts, palette, spacing), [`widgets`] and [`icons`], with
//! [`window`] (the window frame) and [`tree`] (folder open/closed state)
//! shared by the screens too.

mod code;
mod dialogs;
mod drop;
pub(crate) mod format;
mod icons;
pub mod import;
mod inspector;
pub(crate) mod layout;
pub mod library;
mod model;
pub mod settings;
mod shortcuts;
mod status_bar;
mod studio;
mod switch;
pub mod theme;
mod toasts;
mod top_bar;
pub(crate) mod tree;
mod viewport_toolbar;
mod welcome;
mod widgets;
pub mod window;

pub use dialogs::{Confirm, ConfirmAction, NewWorkspaceDialog};
pub use studio::{StudioDialog, unavailable_text};
pub use toasts::Toasts;
pub use widgets::vertical_gradient;
