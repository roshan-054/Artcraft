//! A standalone filter ABI v1 guest. All buffer access stays inside owned Rust vectors.
//! The host writes exported memory between calls; the guest never dereferences raw pointers.
use std::cell::RefCell;

use serde_json::Value;

const MANIFEST: &[u8] = include_bytes!("../manifest.json");
const MAX_ALLOCATED_BYTES: usize = 64 * 1024 * 1024;
const MAX_ALLOCATIONS: usize = 8;
const MAX_PARAMS_BYTES: usize = 64 * 1024;
const DEFAULT_GAIN: f32 = 1.25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
enum FilterError {
    Memory = 1,
    Geometry = 2,
    Format = 3,
    Params = 4,
    Samples = 5,
}

#[derive(Default)]
struct Allocations {
    blocks: Vec<Vec<u8>>,
    total: usize,
}

impl Allocations {
    fn allocate(&mut self, size: usize) -> u32 {
        let Some(total) = self.total.checked_add(size) else { return 0 };
        if size == 0 || total > MAX_ALLOCATED_BYTES || self.blocks.len() >= MAX_ALLOCATIONS {
            return 0;
        }
        if self.blocks.try_reserve(1).is_err() {
            return 0;
        }
        let mut block = Vec::new();
        if block.try_reserve_exact(size).is_err() {
            return 0;
        }
        block.resize(size, 0);
        let pointer = block.as_ptr() as usize as u32;
        if pointer == 0 {
            return 0;
        }
        self.blocks.push(block);
        self.total = total;
        pointer
    }

    fn bytes(&self, pointer: u32, length: u32) -> Result<&[u8], FilterError> {
        self.blocks.iter().find(|block| block.as_ptr() as usize as u32 == pointer).and_then(|block| block.get(..length as usize)).ok_or(FilterError::Memory)
    }

    fn bytes_mut(&mut self, pointer: u32, length: u32) -> Result<&mut [u8], FilterError> {
        self.blocks
            .iter_mut()
            .find(|block| block.as_ptr() as usize as u32 == pointer)
            .and_then(|block| block.get_mut(..length as usize))
            .ok_or(FilterError::Memory)
    }
}

thread_local! {
    // Each host band gets a fresh instance; these allocations die with that instance.
    static ALLOCATIONS: RefCell<Allocations> = RefCell::new(Allocations::default());
}

#[unsafe(no_mangle)]
pub extern "C" fn pc_abi_version() -> u32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn pc_manifest() -> u64 {
    ((MANIFEST.len() as u64) << 32) | MANIFEST.as_ptr() as usize as u32 as u64
}

#[unsafe(no_mangle)]
pub extern "C" fn pc_alloc(size: u32) -> u32 {
    ALLOCATIONS.with(|allocations| allocations.try_borrow_mut().map_or(0, |mut allocations| allocations.allocate(size as usize)))
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn pc_filter(buffer: u32, buffer_len: u32, width: u32, height: u32, channels: u32, format: u32, params: u32, params_len: u32) -> i32 {
    let result = ALLOCATIONS.with(|allocations| {
        let mut allocations = allocations.try_borrow_mut().map_err(|_| FilterError::Memory)?;
        if buffer == params {
            return Err(FilterError::Memory);
        }
        // Parse the separate parameter allocation before borrowing the pixel block mutably.
        let gain = parse_gain(allocations.bytes(params, params_len)?)?;
        process_band(allocations.bytes_mut(buffer, buffer_len)?, width, height, channels, format, gain)
    });
    match result {
        Ok(()) => 0,
        Err(error) => error as i32,
    }
}

fn parse_gain(params: &[u8]) -> Result<f32, FilterError> {
    if params.len() > MAX_PARAMS_BYTES {
        return Err(FilterError::Params);
    }
    let value: Value = serde_json::from_slice(params).map_err(|_| FilterError::Params)?;
    let object = value.as_object().ok_or(FilterError::Params)?;
    // `_image` is host-supplied band metadata. This point filter does not need it.
    if object.keys().any(|key| key != "gain" && key != "_image") {
        return Err(FilterError::Params);
    }
    let Some(gain) = object.get("gain").filter(|value| !value.is_null()) else { return Ok(DEFAULT_GAIN) };
    let gain = gain.as_f64().filter(|gain| gain.is_finite() && (0.0..=2.0).contains(gain)).ok_or(FilterError::Params)?;
    Ok(gain as f32)
}

fn sample(bytes: &[u8]) -> Result<f32, FilterError> {
    let bytes: [u8; 4] = bytes.try_into().map_err(|_| FilterError::Geometry)?;
    Ok(f32::from_le_bytes(bytes))
}

fn process_band(pixels: &mut [u8], width: u32, height: u32, channels: u32, format: u32, gain: f32) -> Result<(), FilterError> {
    if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
        return Err(FilterError::Params);
    }
    let depth = format & 0xff;
    let alpha = (format >> 8) & 1;
    let color_channels = match (format >> 16) & 0xff {
        1 => 1,
        3 => 3,
        _ => return Err(FilterError::Format),
    };
    if !matches!(depth, 8 | 16 | 32) || format & !0x00ff_01ff != 0 || channels != color_channels + alpha {
        return Err(FilterError::Format);
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(channels as usize))
        .and_then(|samples| samples.checked_mul(4))
        .ok_or(FilterError::Geometry)?;
    if width == 0 || height == 0 || expected > MAX_ALLOCATED_BYTES || pixels.len() != expected {
        return Err(FilterError::Geometry);
    }
    let is_color = |index: usize| index % (channels as usize) < color_channels as usize;
    // Validate all color samples before editing any bytes. Alpha is copied verbatim.
    for (index, bytes) in pixels.as_chunks::<4>().0.iter().enumerate() {
        if is_color(index) {
            let color = sample(bytes)?;
            if !color.is_finite() || !(color * gain).is_finite() {
                return Err(FilterError::Samples);
            }
        }
    }
    for (index, bytes) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        if is_color(index) {
            let color = sample(bytes)? * gain;
            let color = if depth == 32 { color } else { color.clamp(0.0, 1.0) };
            bytes.copy_from_slice(&color.to_le_bytes());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(samples: &[f32]) -> Vec<u8> {
        samples.iter().flat_map(|sample| sample.to_le_bytes()).collect()
    }

    fn decode(bytes: &[u8]) -> Vec<f32> {
        bytes.as_chunks::<4>().0.iter().map(|bytes| sample(bytes).unwrap()).collect()
    }

    fn format(mode: u32, depth: u32, alpha: bool) -> u32 {
        depth | u32::from(alpha) << 8 | mode << 16
    }

    #[test]
    fn rgb_supports_all_depths_and_preserves_alpha() {
        for depth in [8, 16, 32] {
            let mut pixels = encode(&[0.1, 0.25, 0.75, 0.375, 0.2, 0.3, 0.4, 1.0]);
            process_band(&mut pixels, 2, 1, 4, format(3, depth, true), 2.0).unwrap();
            let expected_blue = if depth == 32 { 1.5 } else { 1.0 };
            assert_eq!(decode(&pixels), [0.2, 0.5, expected_blue, 0.375, 0.4, 0.6, 0.8, 1.0]);
        }
    }

    #[test]
    fn grayscale_and_rgb_without_alpha_work() {
        for depth in [8, 16, 32] {
            let mut gray = encode(&[0.25, 0.5]);
            process_band(&mut gray, 2, 1, 1, format(1, depth, false), 0.5).unwrap();
            assert_eq!(decode(&gray), [0.125, 0.25]);
            let mut rgb = encode(&[0.2, 0.4, 0.8]);
            process_band(&mut rgb, 1, 1, 3, format(3, depth, false), 0.5).unwrap();
            assert_eq!(decode(&rgb), [0.1, 0.2, 0.4]);
            let mut gray_alpha = encode(&[0.25, 0.375]);
            process_band(&mut gray_alpha, 1, 1, 2, format(1, depth, true), 2.0).unwrap();
            assert_eq!(decode(&gray_alpha), [0.5, 0.375]);
        }
    }

    #[test]
    fn float_samples_keep_hdr_and_negative_values() {
        let mut pixels = encode(&[2.0, -0.25, 0.5]);
        process_band(&mut pixels, 1, 1, 3, format(3, 32, false), 1.5).unwrap();
        assert_eq!(decode(&pixels), [3.0, -0.375, 0.75]);
    }

    #[test]
    fn gain_defaults_and_metadata_are_accepted() {
        assert_eq!(parse_gain(br#"{}"#).unwrap(), 1.25);
        assert_eq!(parse_gain(br#"{"gain":null}"#).unwrap(), 1.25);
        assert_eq!(parse_gain(br#"{"gain":0,"_image":{"x":0}}"#).unwrap(), 0.0);
        assert_eq!(parse_gain(br#"{"gain":2}"#).unwrap(), 2.0);
    }

    #[test]
    fn malformed_json_wrong_types_and_invalid_gains_fail() {
        for params in [
            b"".as_slice(),
            b"{",
            b"[]",
            b"null",
            br#"{"gain":"1"}"#,
            br#"{"gain":true}"#,
            br#"{"gain":-1}"#,
            br#"{"gain":2.1}"#,
            br#"{"gain":1e999}"#,
            br#"{"unexpected":1}"#,
        ] {
            assert_eq!(parse_gain(params), Err(FilterError::Params));
        }
        assert_eq!(parse_gain(&vec![b' '; MAX_PARAMS_BYTES + 1]), Err(FilterError::Params));
        for gain in [f32::NAN, f32::INFINITY, -0.1, 2.1] {
            let mut pixels = encode(&[0.5]);
            assert_eq!(process_band(&mut pixels, 1, 1, 1, format(1, 8, false), gain), Err(FilterError::Params));
            assert_eq!(decode(&pixels), [0.5]);
        }
    }

    #[test]
    fn unsupported_modes_depths_and_channels_fail_without_edits() {
        for mode in [0, 2, 4, 7, 8, 9, 255] {
            let mut pixels = encode(&[0.5]);
            assert_eq!(process_band(&mut pixels, 1, 1, 1, format(mode, 8, false), 1.0), Err(FilterError::Format));
            assert_eq!(decode(&pixels), [0.5]);
        }
        for (channels, value) in [(1, format(1, 24, false)), (3, format(3, 8, true)), (0, format(1, 8, false)), (1, format(1, 8, false) | (1 << 31))] {
            assert_eq!(process_band(&mut [], 1, 1, channels, value, 1.0), Err(FilterError::Format));
        }
    }

    #[test]
    fn invalid_geometry_and_nonfinite_samples_fail_without_edits() {
        for (width, height, length) in [(0, 1, 4), (1, 0, 4), (1, 1, 3), (1, 1, 8), (u32::MAX, u32::MAX, 0)] {
            assert_eq!(process_band(&mut vec![0; length], width, height, 1, format(1, 8, false), 1.0), Err(FilterError::Geometry));
        }
        let mut pixels = encode(&[0.5, f32::INFINITY, 0.5]);
        let before = pixels.clone();
        assert_eq!(process_band(&mut pixels, 1, 1, 3, format(3, 32, false), 2.0), Err(FilterError::Samples));
        assert_eq!(pixels, before);
        let mut pixels = encode(&[f32::MAX]);
        assert_eq!(process_band(&mut pixels, 1, 1, 1, format(1, 32, false), 2.0), Err(FilterError::Samples));
    }

    #[test]
    fn allocator_keeps_separate_bounded_blocks_and_checks_ranges() {
        let mut allocations = Allocations::default();
        let pixels = allocations.allocate(12);
        let params = allocations.allocate(4);
        assert_ne!(pixels, 0);
        assert_ne!(params, 0);
        assert_ne!(pixels, params);
        assert_eq!(allocations.bytes(pixels, 12).unwrap(), [0; 12]);
        assert_eq!(allocations.bytes(pixels, 13), Err(FilterError::Memory));
        assert_eq!(allocations.bytes(0, 1), Err(FilterError::Memory));
        assert_eq!(allocations.allocate(0), 0);
        assert_eq!(allocations.allocate(MAX_ALLOCATED_BYTES), 0);
        allocations.bytes_mut(params, 4).unwrap().copy_from_slice(b"{}  ");
        assert_eq!(parse_gain(allocations.bytes(params, 4).unwrap()).unwrap(), DEFAULT_GAIN);
        for _ in allocations.blocks.len()..MAX_ALLOCATIONS {
            assert_ne!(allocations.allocate(1), 0);
        }
        assert_eq!(allocations.allocate(1), 0);
    }
}
