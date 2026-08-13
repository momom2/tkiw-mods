//! Reading GameMaker `RValue`s out of the live process.
//!
//! An `RValue` is 16 bytes: an 8-byte payload, then flags, then a kind tag.
//! Everything the GML runtime hands back is one of these.
//!
//! Every field is read through a validated pointer. The mod derives these
//! addresses from a static analysis of the executable, and being wrong about
//! one must produce `None`, not an access violation in someone's game.

use crate::win;

pub const KIND_REAL: u32 = 0;
pub const KIND_STRING: u32 = 1;
pub const KIND_ARRAY: u32 = 2;
pub const KIND_UNDEFINED: u32 = 5;
pub const KIND_OBJECT: u32 = 6;
pub const KIND_INT32: u32 = 7;
pub const KIND_INT64: u32 = 10;
pub const KIND_BOOL: u32 = 13;
/// `VALUE_REF` — payload is `{high dword = ref type, low dword = id}`, where the
/// ref type names a runtime container kind (`ds_list`, `ds_map`, instance, …)
/// from the table at `.data 0x2974010`.
pub const KIND_REF: u32 = 15;

/// A string RValue's payload is not the characters -- it is a refcounted
/// descriptor, observed in the running game as:
///
/// ```text
///   offset 0   char*  data        ; for a literal, this points into .rdata
///   offset 8   u32    refcount
///   offset 12  u32    size        ; high bit set as a flag
/// ```
///
/// Confirmed against `REWARD_UNIT_CLASS_STAT`: size read back as 15, matching
/// `len("unit_class_stat")`, with `data` landing on the same `.rdata` literal
/// the offline analysis had already located.
const STR_DATA: usize = 0;
const STR_SIZE: usize = 12;
const STR_SIZE_MASK: u32 = 0x7FFF_FFFF;
/// Refuse anything longer than this rather than walk memory indefinitely.
const STR_MAX: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Real(f64),
    Int(i64),
    Bool(bool),
    Str(String),
    Array(usize),
    Object(usize),
    Ref { ref_type: u32, id: i32 },
    Undefined,
    Other { kind: u32, raw: u64 },
}

fn read<T: Copy>(addr: usize) -> Option<T> {
    if !win::readable(addr, core::mem::size_of::<T>()) {
        return None;
    }
    Some(unsafe { core::ptr::read_volatile(addr as *const T) })
}


/// A GameMaker array RValue's payload points at a container whose element
/// count sits here -- read straight out of the `array_length` builtin, which
/// is `movd xmm0, [rax + 0x24]` after checking `kind == 2`.
pub const ARRAY_LEN: usize = 0x24;
/// Where the contiguous 16-byte RValue elements begin, relative to the same
/// payload. **Confirmed in the running game at `+0x08`**: a Troops Training
/// choice read back three cards of
/// `{unit_class, stat_type, stat_amount}` with sensible values from that
/// offset. The other candidates are retained as a fallback so a layout change
/// in a future build degrades to a probe rather than to silent nonsense.
pub const ARRAY_DATA: usize = 0x08;
pub const ARRAY_DATA_CANDIDATES: [usize; 5] = [ARRAY_DATA, 0x00, 0x10, 0x18, 0x28];

pub fn read_i32(addr: usize) -> Option<i32> {
    read(addr)
}

pub fn read_usize(addr: usize) -> Option<usize> {
    read(addr)
}

/// Decode the 16-byte RValue at `addr`.
pub fn decode(addr: usize) -> Option<Value> {
    if !win::readable(addr, 16) {
        return None;
    }
    let raw: u64 = read(addr)?;
    let kind: u32 = read(addr + 12)?;
    Some(match kind {
        KIND_REAL => Value::Real(f64::from_bits(raw)),
        KIND_INT32 => Value::Int(raw as u32 as i32 as i64),
        KIND_INT64 => Value::Int(raw as i64),
        KIND_BOOL => Value::Bool(raw != 0),
        KIND_UNDEFINED => Value::Undefined,
        KIND_STRING => match string_at(raw as usize) {
            Some(s) => Value::Str(s),
            None => Value::Other { kind, raw },
        },
        KIND_ARRAY => Value::Array(raw as usize),
        KIND_OBJECT => Value::Object(raw as usize),
        KIND_REF => Value::Ref {
            ref_type: (raw >> 32) as u32,
            id: raw as u32 as i32,
        },
        _ => Value::Other { kind, raw },
    })
}

/// A GameMaker string, given the descriptor pointer an RValue carries.
pub fn string_at(ptr: usize) -> Option<String> {
    if ptr == 0 || !win::readable(ptr, 16) {
        return None;
    }
    let data: usize = read(ptr + STR_DATA)?;
    if data == 0 || !win::readable(data, 1) {
        return None;
    }

    // Trust the size field when it is sane, and fall back to scanning for the
    // NUL when it is not -- a string built at runtime may not be laid out the
    // same way as a compiled-in literal.
    let size = read::<u32>(ptr + STR_SIZE).map(|s| (s & STR_SIZE_MASK) as usize);
    if let Some(n) = size {
        if n > 0 && n <= STR_MAX && win::readable(data, n) {
            let mut buf = vec![0u8; n];
            for (i, b) in buf.iter_mut().enumerate() {
                *b = unsafe { core::ptr::read_volatile((data + i) as *const u8) };
            }
            if let Ok(s) = String::from_utf8(buf) {
                return Some(s);
            }
        }
        if n == 0 {
            return Some(String::new());
        }
    }
    cstr(data, STR_MAX)
}

/// A NUL-terminated UTF-8 string, read one page-checked byte at a time.
///
/// Only the fallback: the size field above is authoritative when it is sane.
fn cstr(addr: usize, limit: usize) -> Option<String> {
    if !win::readable(addr, 1) {
        return None;
    }
    let mut out = Vec::new();
    for i in 0..limit {
        // re-check on every page boundary; the string may run off the end of
        // a committed region partway through
        if i == 0 || (addr + i) % 0x1000 == 0 {
            if !win::readable(addr + i, 1) {
                return None;
            }
        }
        let b: u8 = unsafe { core::ptr::read_volatile((addr + i) as *const u8) };
        if b == 0 {
            return String::from_utf8(out).ok();
        }
        // reject binary early rather than returning mojibake that looks like data
        if b < 0x09 {
            return None;
        }
        out.push(b);
    }
    None
}

/// Hex + ASCII of memory around a pointer, for working out an unknown layout.
///
/// Reads defensively: anything unmapped shows as `--` rather than faulting.
pub fn dump(addr: usize, before: usize, len: usize) -> String {
    let start = addr.saturating_sub(before);
    let mut out = String::new();
    for row in 0..len.div_ceil(16) {
        let base = start + row * 16;
        let mut hex = String::new();
        let mut txt = String::new();
        for i in 0..16 {
            let a = base + i;
            if win::readable(a, 1) {
                let b: u8 = unsafe { core::ptr::read_volatile(a as *const u8) };
                hex.push_str(&format!("{b:02x} "));
                txt.push(if (0x20..0x7F).contains(&b) { b as char } else { '.' });
            } else {
                hex.push_str("-- ");
                txt.push(' ');
            }
        }
        let marker = if base <= addr && addr < base + 16 { "<-" } else { "  " };
        out.push_str(&format!("\n    {base:#018x}{marker} {hex} |{txt}|"));
    }
    out
}

/// Describe how a pointer is mapped, for diagnostics.
pub fn region(addr: usize) -> String {
    match win::query(addr) {
        None => "VirtualQuery failed".into(),
        Some(m) => format!(
            "base {:#x} size {:#x} state {:#x} protect {:#x}",
            m.base_address as usize, m.region_size, m.state, m.protect
        ),
    }
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Real(v) => Some(*v),
            Value::Int(v) => Some(*v as f64),
            Value::Bool(b) => Some(*b as u8 as f64),
            _ => None,
        }
    }
}
