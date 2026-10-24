//! Reading lists of an export within bounds. Every list in a document is
//! read through [`seq`], which stops at that list's cap before taking one
//! more record, and charges the memory it takes to a budget for the whole
//! document. A record can be as short as `{}` and still expand into
//! hundreds of bytes, so counting after the document is built would be too
//! late.
//!
//! The budget is set by [`with_budget`] for one document; outside it (a
//! model built in code, a test fixture) only the caps apply.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::marker::PhantomData;

use serde::de::{Deserialize, Deserializer, Error, SeqAccess, Visitor};

/// The most of each thing an export may hold, and how far its lists may
/// expand. Real exports have thousands of meshes, some hundred models and
/// at most a few thousand parts or bones each; these only stop a document
/// that would take far too long, or far too much memory, to read.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
	pub meshes: usize,
	pub models: usize,
	/// Parts in one model.
	pub parts: usize,
	/// Joints in one model.
	pub joints: usize,
	/// Bones in one model.
	pub bones: usize,
	/// Animations of one model.
	pub clips: usize,
	/// Keyframes of one animation.
	pub keyframes: usize,
	/// Poses in one keyframe, and child poses of one pose.
	pub poses: usize,
	/// Face controls of one pose.
	pub face_controls: usize,
	/// Markers of one keyframe.
	pub markers: usize,
	/// Curves of one animation.
	pub curves: usize,
	/// Keys of one curve, or of one channel of it.
	pub keys: usize,
	/// Decals of one part.
	pub decals: usize,
	/// Lights of one part.
	pub lights: usize,
	/// Attachments in one model.
	pub attachments: usize,
	/// Effects (particle emitters, beams, trails...) in one model.
	pub effects: usize,
	/// Keypoints of one effect's sequence.
	pub keypoints: usize,
	/// Folders in one stored path (workspace manifests).
	pub folders: usize,
	/// Keyframes, poses and curve keys of one animation, together.
	pub clip_keys: usize,
	/// The lists of one document may take this many bytes of memory...
	pub decoded_base: usize,
	/// ...plus this many for each byte of the document.
	pub decoded_per_byte: usize,
}

pub const LIMITS: Limits = Limits {
	meshes: 1_000_000,
	models: 100_000,
	parts: 100_000,
	joints: 100_000,
	bones: 10_000,
	clips: 10_000,
	keyframes: 1_000_000,
	poses: 100_000,
	face_controls: 1_000,
	markers: 1_000,
	curves: 1_000_000,
	keys: 5_000_000,
	decals: 1_000,
	lights: 1_000,
	attachments: 100_000,
	effects: 100_000,
	// Roblox allows 20
	keypoints: 1_000,
	// workspace paths are checked to 64 folders when opened; a little more
	// is read so that check, not this one, says what is wrong
	folders: 128,
	clip_keys: 5_000_000,
	decoded_base: 64 * 1024 * 1024,
	decoded_per_byte: 8,
};

/// Which list is being read.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
	Meshes,
	Models,
	Parts,
	Joints,
	Bones,
	Clips,
	Keyframes,
	Poses,
	FaceControls,
	Markers,
	Curves,
	Keys,
	Decals,
	Lights,
	Attachments,
	Effects,
	Keypoints,
	Folders,
}

impl Kind {
	fn cap(self, limits: &Limits) -> usize {
		match self {
			Kind::Meshes => limits.meshes,
			Kind::Models => limits.models,
			Kind::Parts => limits.parts,
			Kind::Joints => limits.joints,
			Kind::Bones => limits.bones,
			Kind::Clips => limits.clips,
			Kind::Keyframes => limits.keyframes,
			Kind::Poses => limits.poses,
			Kind::FaceControls => limits.face_controls,
			Kind::Markers => limits.markers,
			Kind::Curves => limits.curves,
			Kind::Keys => limits.keys,
			Kind::Decals => limits.decals,
			Kind::Lights => limits.lights,
			Kind::Attachments => limits.attachments,
			Kind::Effects => limits.effects,
			Kind::Keypoints => limits.keypoints,
			Kind::Folders => limits.folders,
		}
	}

	/// What is over the cap, as in "more than 10 {what}".
	fn what(self) -> &'static str {
		match self {
			Kind::Meshes => "meshes",
			Kind::Models => "models",
			Kind::Parts => "parts in one model",
			Kind::Joints => "joints in one model",
			Kind::Bones => "bones in one model",
			Kind::Clips => "animations in one model",
			Kind::Keyframes => "keyframes in one animation",
			Kind::Poses => "poses in one keyframe or pose",
			Kind::FaceControls => "face controls in one pose",
			Kind::Markers => "markers in one keyframe",
			Kind::Curves => "curves in one animation",
			Kind::Keys => "keys in one curve",
			Kind::Decals => "decals on one part",
			Kind::Lights => "lights on one part",
			Kind::Attachments => "attachments in one model",
			Kind::Effects => "effects in one model",
			Kind::Keypoints => "keypoints in one sequence",
			Kind::Folders => "folders in one path",
		}
	}
}

struct State {
	limits: Limits,
	/// Bytes the document's lists may still take; `None` outside a document.
	remaining: Option<usize>,
	/// Why the document was refused, once it is.
	over: Option<String>,
}

thread_local! {
	static STATE: RefCell<State> = const {
		RefCell::new(State {
			limits: LIMITS,
			remaining: None,
			over: None,
		})
	};
	/// Records taken into lists since the last [`with_budget`], so tests can
	/// see how far a refused document was read.
	static READ: Cell<usize> = const { Cell::new(0) };
}

/// Runs `read` on a document of `len` bytes with `limits` in force,
/// returning its result and, when a limit stopped it, why. Whatever was in
/// force before is restored after, even if `read` panics.
pub fn with_budget<R>(
	limits: &Limits,
	len: usize,
	read: impl FnOnce() -> R,
) -> (R, Option<String>) {
	struct Restore(Option<State>);
	impl Drop for Restore {
		fn drop(&mut self) {
			if let Some(state) = self.0.take() {
				STATE.with(|s| *s.borrow_mut() = state);
			}
		}
	}
	let budget = len
		.saturating_mul(limits.decoded_per_byte)
		.saturating_add(limits.decoded_base);
	let before = STATE.with(|s| {
		s.replace(State {
			limits: *limits,
			remaining: Some(budget),
			over: None,
		})
	});
	READ.with(|r| r.set(0));
	let restore = Restore(Some(before));
	let result = read();
	let over = STATE.with(|s| s.borrow_mut().over.take());
	drop(restore);
	(result, over)
}

/// Records taken into lists since the last [`with_budget`] began.
#[cfg(test)]
pub fn records_read() -> usize {
	READ.with(Cell::get)
}

/// Makes room for one more record in `list`, a list of `kind`, charging
/// any new memory to the document. Errors once the list is full or the
/// document's budget is spent.
fn make_room<T>(list: &mut Vec<T>, kind: Kind) -> Result<(), String> {
	STATE.with(|s| {
		let mut s = s.borrow_mut();
		let cap = kind.cap(&s.limits);
		let mut refused = None;
		if list.len() >= cap {
			refused = Some(format!(
				"This export has more than {cap} {}, the most Meshvault reads.",
				kind.what()
			));
		} else if list.len() == list.capacity() {
			// grows as a Vec would, but never past the cap
			let more = list.len().max(4).min(cap - list.len());
			let bytes = more.saturating_mul(size_of::<T>().max(1));
			match &mut s.remaining {
				Some(remaining) if bytes > *remaining => {
					refused = Some(
						"This export would take too much memory to read: it holds far more records than its size suggests."
							.into(),
					);
				}
				Some(remaining) => *remaining -= bytes,
				None => {}
			}
			if refused.is_none() {
				list.reserve_exact(more);
			}
		}
		if let Some(why) = refused {
			s.over = Some(why.clone());
			return Err(why);
		}
		READ.with(|r| r.set(r.get() + 1));
		Ok(())
	})
}

/// Adds `item` to `list`, a list of `kind` read by a deserializer of its
/// own, within the list's cap and the document's budget.
pub fn push<T>(list: &mut Vec<T>, item: T, kind: Kind) -> Result<(), String> {
	make_room(list, kind)?;
	list.push(item);
	Ok(())
}

/// Reads a list of `kind` within its cap and the document's budget.
pub fn seq<'de, D, T>(d: D, kind: Kind) -> Result<Vec<T>, D::Error>
where
	D: Deserializer<'de>,
	T: Deserialize<'de>,
{
	d.deserialize_seq(Bounded(kind, PhantomData))
}

/// [`seq`] for a list that may be `null`.
pub fn opt_seq<'de, D, T>(d: D, kind: Kind) -> Result<Option<Vec<T>>, D::Error>
where
	D: Deserializer<'de>,
	T: Deserialize<'de>,
{
	d.deserialize_option(Maybe(kind, PhantomData))
}

struct Bounded<T>(Kind, PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for Bounded<T> {
	type Value = Vec<T>;

	fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.write_str("a list")
	}

	fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
		let mut list = Vec::new();
		while let Some(item) = seq.next_element()? {
			make_room(&mut list, self.0).map_err(A::Error::custom)?;
			list.push(item);
		}
		Ok(list)
	}
}

struct Maybe<T>(Kind, PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for Maybe<T> {
	type Value = Option<Vec<T>>;

	fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.write_str("a list or null")
	}

	fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
		Ok(None)
	}

	fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
		Ok(None)
	}

	fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
		seq(d, self.0).map(Some)
	}
}

/// `deserialize_with` functions, one per kind of list.
macro_rules! lists {
	($($name:ident => $kind:ident),* $(,)?) => {$(
		#[doc = concat!("A bounded list of ", stringify!($name), "; see [`seq`].")]
		pub fn $name<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
		where
			D: Deserializer<'de>,
			T: Deserialize<'de>,
		{
			seq(d, Kind::$kind)
		}
	)*};
}

lists! {
	meshes => Meshes,
	models => Models,
	parts => Parts,
	joints => Joints,
	bones => Bones,
	clips => Clips,
	keyframes => Keyframes,
	poses => Poses,
	face_controls => FaceControls,
	markers => Markers,
	curves => Curves,
	keys => Keys,
	decals => Decals,
	lights => Lights,
	attachments => Attachments,
	effects => Effects,
	folders => Folders,
}

/// A curve channel's keys, which may be absent or `null`.
pub fn channel_keys<'de, D, T>(d: D) -> Result<Option<Vec<T>>, D::Error>
where
	D: Deserializer<'de>,
	T: Deserialize<'de>,
{
	opt_seq(d, Kind::Keys)
}
