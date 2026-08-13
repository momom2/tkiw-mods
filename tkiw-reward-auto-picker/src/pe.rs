//! Just enough PE to find the game's sections.
//!
//! Symbol discovery reads the executable **from disk** rather than scraping it
//! out of memory. The discriminator for a variable-id slot is that it holds
//! `0xFFFFFFFF` before the game resolves it at startup -- which is true on disk
//! and false in a running process. Reading the file keeps the runtime resolver
//! byte-for-byte the same algorithm as the offline analysis in `analysis/`,
//! which is the version that has actually been validated. Live addresses come
//! from adding the ASLR slide afterwards.

use std::collections::HashMap;

pub struct Section {
    pub name: String,
    pub va: u32,
    pub vsize: u32,
    pub raw: u32,
    pub rawsize: u32,
}

pub struct Image {
    pub data: Vec<u8>,
    pub image_base: u64,
    /// SizeOfImage: how much address space the module occupies once loaded.
    /// Any live address the mod derives must fall inside `base .. base + this`.
    pub size_of_image: u32,
    pub sections: Vec<Section>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}
fn u64_at(d: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(d.get(o..o + 8)?.try_into().ok()?))
}

impl Image {
    pub fn load(path: &str) -> Option<Image> {
        let data = std::fs::read(path).ok()?;
        let peo = u32_at(&data, 0x3C)? as usize;
        if data.get(peo..peo + 4)? != b"PE\0\0" {
            return None;
        }
        let nsec = u16_at(&data, peo + 6)? as usize;
        let opt = peo + 24;
        if u16_at(&data, opt)? != 0x20B {
            return None; // not PE32+
        }
        let image_base = u64_at(&data, opt + 24)?;
        let size_of_image = u32_at(&data, opt + 56)?;
        let sectab = opt + u16_at(&data, peo + 20)? as usize;
        let mut sections = Vec::with_capacity(nsec);
        for i in 0..nsec {
            let o = sectab + i * 40;
            let raw_name = data.get(o..o + 8)?;
            let end = raw_name.iter().position(|&c| c == 0).unwrap_or(8);
            sections.push(Section {
                name: String::from_utf8_lossy(&raw_name[..end]).to_string(),
                vsize: u32_at(&data, o + 8)?,
                va: u32_at(&data, o + 12)?,
                rawsize: u32_at(&data, o + 16)?,
                raw: u32_at(&data, o + 20)?,
            });
        }
        Some(Image { data, image_base, size_of_image, sections })
    }

    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    pub fn rva_in(&self, rva: u32, name: &str) -> bool {
        match self.section(name) {
            Some(s) => rva >= s.va && rva < s.va + s.vsize.max(s.rawsize),
            None => false,
        }
    }

    /// File offset backing `rva`, if it is file-backed at all.
    pub fn rva2off(&self, rva: u32) -> Option<usize> {
        for s in &self.sections {
            if rva >= s.va && rva < s.va + s.vsize.max(s.rawsize) {
                let d = rva - s.va;
                if d < s.rawsize {
                    return Some((s.raw + d) as usize);
                }
            }
        }
        None
    }

    pub fn u32_rva(&self, rva: u32) -> Option<u32> {
        u32_at(&self.data, self.rva2off(rva)?)
    }

    pub fn u64_rva(&self, rva: u32) -> Option<u64> {
        u64_at(&self.data, self.rva2off(rva)?)
    }

    /// A printable NUL-terminated string at `rva`, bounded by `limit`.
    pub fn cstr(&self, rva: u32, limit: usize) -> Option<&str> {
        let o = self.rva2off(rva)?;
        let slice = self.data.get(o..(o + limit).min(self.data.len()))?;
        let end = slice.iter().position(|&c| c == 0)?;
        let s = std::str::from_utf8(&slice[..end]).ok()?;
        if s.bytes().all(|c| (0x20..0x7F).contains(&c)) {
            Some(s)
        } else {
            None
        }
    }

    pub fn va2rva(&self, va: u64) -> Option<u32> {
        va.checked_sub(self.image_base)?.try_into().ok()
    }

    /// Bounds of the initialised part of a section, as RVAs.
    pub fn initialised_range(&self, name: &str) -> Option<(u32, u32)> {
        let s = self.section(name)?;
        Some((s.va, s.va + s.rawsize))
    }
}

/// A name -> RVA table, plus the sections it was derived from.
pub type RvaMap = HashMap<String, u32>;

impl Image {
    /// RVA of the import-address-table slot holding `func` imported from `dll`.
    ///
    /// This is how the mod gets onto the game's thread: overwriting one pointer
    /// here redirects a call the game already makes every frame. No code is
    /// modified, nothing needs disassembling, and undoing it is writing the
    /// original pointer back.
    pub fn iat_slot(&self, dll: &str, func: &str) -> Option<u32> {
        let opt = {
            let peo = u32_at(&self.data, 0x3C)? as usize;
            peo + 24
        };
        let import_dir = u32_at(&self.data, opt + 120)?;
        let mut desc = self.rva2off(import_dir)?;

        loop {
            let ilt = u32_at(&self.data, desc)?;
            let name_rva = u32_at(&self.data, desc + 12)?;
            let iat_rva = u32_at(&self.data, desc + 16)?;
            if name_rva == 0 {
                return None;
            }
            let this_dll = self.cstr(name_rva, 260).unwrap_or("");
            if this_dll.eq_ignore_ascii_case(dll) {
                // walk the lookup table; the IAT is parallel to it
                let mut walk = self.rva2off(if ilt != 0 { ilt } else { iat_rva })?;
                let mut index = 0u32;
                loop {
                    let entry = u64_at(&self.data, walk)?;
                    if entry == 0 {
                        break;
                    }
                    // high bit set means imported by ordinal, which has no name
                    if entry & (1 << 63) == 0 {
                        let hint_name = (entry & 0x7FFF_FFFF) as u32;
                        // the name follows a 2-byte hint
                        if let Some(n) = self.cstr(hint_name + 2, 260) {
                            if n == func {
                                return Some(iat_rva + index * 8);
                            }
                        }
                    }
                    walk += 8;
                    index += 1;
                }
                return None;
            }
            desc += 20;
        }
    }
}
