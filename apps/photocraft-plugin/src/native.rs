//! Header-only native target validation, independent of the worker's executable behavior.
use crate::{Result, fail};

fn u16_le(bytes: &[u8], offset: usize) -> Result<u16> {
    let bytes: [u8; 2] = bytes.get(offset..offset.saturating_add(2)).ok_or_else(|| fail("truncated executable header"))?.try_into()?;
    Ok(u16::from_le_bytes(bytes))
}

fn u32_word(bytes: &[u8], offset: usize, big_endian: bool) -> Result<u32> {
    let bytes: [u8; 4] = bytes.get(offset..offset.saturating_add(4)).ok_or_else(|| fail("truncated executable header"))?.try_into()?;
    Ok(if big_endian { u32::from_be_bytes(bytes) } else { u32::from_le_bytes(bytes) })
}

pub(crate) fn check(bytes: &[u8], platform: &str) -> Result<()> {
    let (os, arch) = platform.split_once('-').ok_or_else(|| fail("invalid declared native platform"))?;
    let valid = match os {
        "windows" => {
            if !bytes.starts_with(b"MZ") {
                return Err(fail("declared Windows worker is not a PE executable"));
            }
            let offset = u32_word(bytes, 0x3c, false)? as usize;
            let signature = bytes.get(offset..offset.saturating_add(4));
            let machine = u16_le(bytes, offset.saturating_add(4))?;
            signature == Some(b"PE\0\0") && matches!((arch, machine), ("x86_64", 0x8664) | ("aarch64", 0xaa64))
        }
        "linux" => {
            if !bytes.starts_with(b"\x7fELF\x02\x01") {
                return Err(fail("declared Linux worker must be a 64-bit little-endian ELF executable"));
            }
            let machine = u16_le(bytes, 18)?;
            matches!((arch, machine), ("x86_64", 62) | ("aarch64", 183))
        }
        "macos" => {
            let cpu = match arch {
                "x86_64" => 0x0100_0007,
                "aarch64" => 0x0100_000c,
                _ => return Err(fail("unsupported Mach-O architecture")),
            };
            if bytes.starts_with(b"\xcf\xfa\xed\xfe") {
                u32_word(bytes, 4, false)? == cpu
            } else if bytes.starts_with(b"\xca\xfe\xba\xbe") || bytes.starts_with(b"\xca\xfe\xba\xbf") {
                let count = u32_word(bytes, 4, true)? as usize;
                if count == 0 || count > 32 {
                    return Err(fail("invalid universal Mach-O architecture count"));
                }
                let stride: usize = if bytes.get(3) == Some(&0xbf) { 32 } else { 20 };
                let mut found = false;
                for index in 0..count {
                    let offset = index.checked_mul(stride).and_then(|n| n.checked_add(8)).ok_or_else(|| fail("Mach-O architecture offset overflow"))?;
                    found |= u32_word(bytes, offset, true)? == cpu;
                }
                found
            } else {
                return Err(fail("declared macOS worker is not a supported 64-bit Mach-O executable"));
            }
        }
        _ => return Err(fail("unsupported native platform")),
    };
    if valid { Ok(()) } else { Err(fail("native worker header does not match its declared platform/architecture")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_and_wrong_architecture_workers_are_rejected() {
        assert!(check(b"MZ", "windows-x86_64").is_err());
        assert!(check(b"\x7fELF\x02\x01", "linux-x86_64").is_err());
        assert!(check(b"not an executable", "macos-aarch64").is_err());
        let mut elf = vec![0u8; 64];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[18..20].copy_from_slice(&62u16.to_le_bytes());
        check(&elf, "linux-x86_64").unwrap();
        assert!(check(&elf, "linux-aarch64").is_err());
    }

    #[test]
    fn universal_macho_must_contain_the_declared_architecture() {
        let mut macho = vec![0u8; 28];
        macho[..4].copy_from_slice(b"\xca\xfe\xba\xbe");
        macho[4..8].copy_from_slice(&1u32.to_be_bytes());
        macho[8..12].copy_from_slice(&0x0100_000cu32.to_be_bytes());
        check(&macho, "macos-aarch64").unwrap();
        assert!(check(&macho, "macos-x86_64").is_err());
        macho[4..8].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(check(&macho, "macos-aarch64").is_err());
    }
}
