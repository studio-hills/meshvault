use super::roblox_bit_reader::{BitCounterError, RobloxBitReader};

#[derive(Debug, Eq, PartialEq)]
pub enum Symbol {
	// 1 bit
	Continue, // 0b0
	// 3 bits
	Split, // 0b1_00
	Left,  // 0b1_01
	Right, // 0b1_10
	End,   // 0b1_11
}

pub struct SymbolReader<'a> {
	bit_reader: RobloxBitReader<'a>,
}
impl<'a> SymbolReader<'a> {
	pub fn new(bytes: &'a [u8], bits: u32) -> Result<Self, BitCounterError> {
		let bit_reader = RobloxBitReader::new(bytes, bits)?;
		Ok(Self { bit_reader })
	}
	pub const fn remaining_bits(&self) -> u32 {
		self.bit_reader.remaining_bits()
	}
	pub fn read(&mut self) -> Result<Symbol, BitCounterError> {
		if self.bit_reader.read(1)? == 0 {
			return Ok(Symbol::Continue);
		}
		let bits = self.bit_reader.read(2)?;
		Ok(match (bits & 0b10 != 0, bits & 0b01 != 0) {
			(false, false) => Symbol::Split,
			(false, true) => Symbol::Left,
			(true, false) => Symbol::Right,
			(true, true) => Symbol::End,
		})
	}
}

#[test]
fn read_symbols() {
	// 2 C_C_R_C_S_L_E_C
	// 1 C_C_R_C_S_L_E_C
	// 4 C_C_R_C_S_L_E_C
	// 3     R_C_S_L_E_C
	// BYTES are read in the order 4,3,2,1, 8,7,6,5 msb-first
	#[rustfmt::skip]
	#[expect(clippy::unusual_byte_groupings, reason = "bits are grouped by symbol")]
	const BYTES: [u8; 8] = [
		0b0_101_111_0, 0b0_0_110_0_10, // [4, 3]
		0b0_101_111_0, 0b0_0_110_0_10, // [2, 1]
		0b0_101_111_0, 0b0_0_110_0_10, // [8, 7]
		0b0_101_111_0,     0b110_0_10, // [6, 5]
	];
	// truncate last word by 2 bits
	let mut r = SymbolReader::new(&BYTES, 62).unwrap();
	assert_eq!(r.read(), Ok(Symbol::Continue));
	assert_eq!(r.read(), Ok(Symbol::Continue));
	assert_eq!(r.read(), Ok(Symbol::Right));
	assert_eq!(r.read(), Ok(Symbol::Continue));
	assert_eq!(r.read(), Ok(Symbol::Split));
	assert_eq!(r.read(), Ok(Symbol::Left));
	assert_eq!(r.read(), Ok(Symbol::End));
	assert_eq!(r.read(), Ok(Symbol::Continue));

	assert_eq!(r.read(), Ok(Symbol::Continue));
	assert_eq!(r.read(), Ok(Symbol::Continue));
	assert_eq!(r.read(), Ok(Symbol::Right));
	assert_eq!(r.read(), Ok(Symbol::Continue));
	assert_eq!(r.read(), Ok(Symbol::Split));
	assert_eq!(r.read(), Ok(Symbol::Left));
	assert_eq!(r.read(), Ok(Symbol::End));
	assert_eq!(r.read(), Ok(Symbol::Continue));

	// Truncation affects this section
	assert_eq!(r.read(), Ok(Symbol::Right));
	assert_eq!(r.read(), Ok(Symbol::Continue));
	assert_eq!(r.read(), Ok(Symbol::Split));
	assert_eq!(r.read(), Ok(Symbol::Left));
	assert_eq!(r.read(), Ok(Symbol::End));
	assert_eq!(r.read(), Ok(Symbol::Continue));

	assert_eq!(r.read(), Ok(Symbol::Continue));
	assert_eq!(r.read(), Ok(Symbol::Continue));
	assert_eq!(r.read(), Ok(Symbol::Right));
	assert_eq!(r.read(), Ok(Symbol::Continue));
	assert_eq!(r.read(), Ok(Symbol::Split));
	assert_eq!(r.read(), Ok(Symbol::Left));
	assert_eq!(r.read(), Ok(Symbol::End));
	assert_eq!(r.read(), Ok(Symbol::Continue));
	assert_eq!(r.read(), Err(BitCounterError::NotEnoughBits));
}
#[test]
fn empirical() {
	const BYTES: [u8; 4] = [183, 219, 55, 0];
	let mut r = SymbolReader::new(&BYTES, 22).unwrap();
	// dump partial symbol
	assert_eq!(r.bit_reader.read(1), Ok(1));

	assert_eq!(r.read(), Ok(Symbol::Left));
	assert_eq!(r.read(), Ok(Symbol::End));
	assert_eq!(r.read(), Ok(Symbol::Left));
	assert_eq!(r.read(), Ok(Symbol::Left));
	assert_eq!(r.read(), Ok(Symbol::Right));
	assert_eq!(r.read(), Ok(Symbol::Right));
	assert_eq!(r.read(), Ok(Symbol::End));
	assert_eq!(r.read(), Err(BitCounterError::NotEnoughBits));
}

/// Packs symbols written as `C`, `S`, `L`, `R`, `E` the way
/// [`SymbolReader`] reads them, returning the bytes and the bit count.
#[cfg(test)]
pub fn pack(symbols: &str) -> (Vec<u8>, u32) {
	let mut bits = Vec::new();
	for symbol in symbols.chars() {
		match symbol {
			'C' => bits.push(false),
			'S' => bits.extend([true, false, false]),
			'L' => bits.extend([true, false, true]),
			'R' => bits.extend([true, true, false]),
			'E' => bits.extend([true, true, true]),
			_ => panic!("not a symbol: {symbol}"),
		}
	}
	let mut bytes = Vec::new();
	// each 32-bit word holds its bits lsb-aligned, read msb-first
	for word in bits.chunks(32) {
		let value = word
			.iter()
			.fold(0u32, |value, &bit| (value << 1) | u32::from(bit));
		bytes.extend(value.to_le_bytes());
	}
	(bytes, bits.len() as u32)
}

#[test]
fn pack_round_trips() {
	let symbols = "CCRCSLEC".repeat(9);
	let (bytes, bits) = pack(&symbols);
	let mut r = SymbolReader::new(&bytes, bits).unwrap();
	for symbol in symbols.chars() {
		let expected = match symbol {
			'C' => Symbol::Continue,
			'S' => Symbol::Split,
			'L' => Symbol::Left,
			'R' => Symbol::Right,
			_ => Symbol::End,
		};
		assert_eq!(r.read(), Ok(expected));
	}
	assert_eq!(r.read(), Err(BitCounterError::NotEnoughBits));
}
