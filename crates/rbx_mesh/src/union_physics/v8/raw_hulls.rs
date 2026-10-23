use super::Hull;

/// Hulls as range tables over shared face and position lists: hull `i`
/// uses `faces[face_ranges[i]..face_ranges[i + 1]]` and
/// `positions[pos_ranges[i]..pos_ranges[i + 1]]`.
///
/// Reading checks the tables (see [`Hulls::check`]), so the hulls of a
/// parsed mesh always slice cleanly.
#[derive(Clone, Debug, Default)]
pub struct Hulls {
	pub face_ranges: Vec<u32>,
	pub faces: Vec<u32>,
	pub pos_ranges: Vec<u32>,
	pub positions: Vec<f32>,
}

#[binrw::binread]
#[br(little)]
struct RawHulls {
	#[br(temp)]
	face_range_count: u32,
	#[br(count = face_range_count)]
	face_ranges: Vec<u32>,
	// the last hull_range value gives the total index count
	#[br(count = face_ranges.last().copied().unwrap_or(0))]
	faces: Vec<u32>,
	#[br(temp)]
	pos_range_count: u32,
	#[br(count = pos_range_count)]
	pos_ranges: Vec<u32>,
	#[br(count = pos_ranges.last().copied().unwrap_or(0))]
	positions: Vec<f32>,
}

impl binrw::BinRead for Hulls {
	type Args<'a> = ();
	fn read_options<R: binrw::io::Read + binrw::io::Seek>(
		reader: &mut R,
		endian: binrw::Endian,
		args: Self::Args<'_>,
	) -> binrw::BinResult<Self> {
		let pos = reader.stream_position()?;
		let RawHulls {
			face_ranges,
			faces,
			pos_ranges,
			positions,
		} = RawHulls::read_options(reader, endian, args)?;
		let hulls = Hulls {
			face_ranges,
			faces,
			pos_ranges,
			positions,
		};
		hulls
			.check()
			.map_err(|message| binrw::Error::AssertFail { pos, message })?;
		Ok(hulls)
	}
}

/// Checks one range table against the length of the list it indexes.
fn check_ranges(what: &str, ranges: &[u32], len: usize) -> Result<(), String> {
	let Some((&first, rest)) = ranges.split_first() else {
		return Ok(());
	};
	if first != 0 {
		return Err(format!("{what} ranges start at {first}, not 0"));
	}
	let mut last = first;
	for (i, &end) in rest.iter().enumerate() {
		if end < last {
			return Err(format!("{what} range {i} runs backwards ({last}..{end})"));
		}
		if end % 3 != 0 {
			return Err(format!(
				"{what} range {i} ends at {end}, not a multiple of 3"
			));
		}
		last = end;
	}
	if len < last as usize {
		return Err(format!(
			"{what} ranges end at {last}, past the {len} stored"
		));
	}
	Ok(())
}

impl Hulls {
	/// Checks that both range tables describe the same number of hulls,
	/// start at 0, never run backwards, cover whole triangles and positions,
	/// and stay within the stored faces and positions.
	pub fn check(&self) -> Result<(), String> {
		if self.face_ranges.len() != self.pos_ranges.len() {
			return Err(format!(
				"{} face ranges but {} position ranges",
				self.face_ranges.len(),
				self.pos_ranges.len()
			));
		}
		check_ranges("face", &self.face_ranges, self.faces.len())?;
		check_ranges("position", &self.pos_ranges, self.positions.len())
	}
	/// Iterates the hulls. A range that does not fit the stored data (only
	/// possible for `Hulls` built by hand, see [`Hulls::check`]) yields an
	/// empty slice instead of panicking.
	pub fn iter_hulls(&self) -> impl ExactSizeIterator<Item = Hull<'_>> {
		self.face_ranges
			.array_windows()
			.zip(self.pos_ranges.array_windows())
			.map(move |(&[face_start, face_end], &[pos_start, pos_end])| {
				let positions = self
					.positions
					.get(pos_start as usize..pos_end as usize)
					.unwrap_or_default();
				let faces = self
					.faces
					.get(face_start as usize..face_end as usize)
					.unwrap_or_default();

				let (positions, _) = positions.as_chunks();
				let (faces, _) = faces.as_chunks();

				Hull { positions, faces }
			})
	}
}
