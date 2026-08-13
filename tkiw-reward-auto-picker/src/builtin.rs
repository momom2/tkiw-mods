//! Calling the runtime's own builtins.
//!
//! Used only for diagnostics, and sparingly. Reading memory is safe; calling
//! into the game is not, so this exists for the one job that reading cannot
//! do: asking a struct what its members are actually called, instead of
//! guessing names and believing whatever comes back.
//!
//! That distinction matters. Reading `unit_class` off a struct by name returns
//! a plausible small integer whether or not it is the field that means what we
//! think -- so a wrong guess looks exactly like a right one. Enumerating the
//! real member names removes the guess.
//!
//! **Calling convention.** Runtime builtins are
//! `f(RValue* result, void* self, void* other, int argc, RValue* args)` with
//! `args` a flat 16-byte-stride array. Compiled *GML* functions use a different
//! register order entirely; mixing them up corrupts memory on the first call.
//! Nothing here calls compiled GML.

use crate::rvalue::{self, Value};
use crate::win;

/// `variable_struct_get_names(struct) -> array of strings`, argc 1.
/// Recovered by name from the runtime's `Function_Add` table (analysis/builtins.py).
const VARIABLE_STRUCT_GET_NAMES_RVA: usize = 0x1b00560;

/// `variable_struct_get(struct, name) -> value`, argc 2.
///
/// Needed because not every member name has a *compile-time* variable id. The
/// mod resolves names through the exe's static variable table, which only holds
/// names that appear literally in the game's code. `resources.coin` is never
/// written that way -- the resource keys come from data -- so `var_id("coin")`
/// returns nothing and every read of the player's denarii silently gave zero.
const VARIABLE_STRUCT_GET_RVA: usize = 0x1b00380;

pub type Builtin = unsafe extern "system" fn(*mut RValueRaw, *mut u8, *mut u8, i32, *const RValueRaw);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RValueRaw {
    pub payload: u64,
    pub flags: u32,
    pub kind: u32,
}

const KIND_UNDEFINED: u32 = 5;

/// Read a struct member by *name*, without needing a variable id for it.
///
/// The name has to reach the runtime as a GML string, and allocating one would
/// mean asking the game for memory. It is not necessary:
/// `variable_struct_get_names` hands back an array of exactly the strings we
/// might want, so the matching element is passed straight back in as the
/// argument. Nothing is allocated and nothing leaks beyond the names array the
/// runtime made.
///
/// # Safety
/// Game thread only.
pub unsafe fn struct_get_by_name(
    base: usize,
    text: (usize, usize),
    strukt: &Value,
    name: &str,
) -> Option<Value> {
    let Value::Object(ptr) = strukt else { return None };
    if *ptr == 0 || !win::readable(*ptr, 8) {
        return None;
    }
    let names_fn = resolve(base, VARIABLE_STRUCT_GET_NAMES_RVA, text)?;
    let get_fn = resolve(base, VARIABLE_STRUCT_GET_RVA, text)?;

    let arg = RValueRaw { payload: *ptr as u64, flags: 0, kind: rvalue::KIND_OBJECT };
    let mut names = RValueRaw { payload: 0, flags: 0, kind: KIND_UNDEFINED };
    names_fn(&mut names, core::ptr::null_mut(), core::ptr::null_mut(), 1, &arg);
    if names.kind != rvalue::KIND_ARRAY {
        return None;
    }
    let payload = names.payload as usize;
    if !win::readable(payload, rvalue::ARRAY_LEN + 4) {
        return None;
    }
    let len = rvalue::read_i32(payload + rvalue::ARRAY_LEN)?;
    if !(0..512).contains(&len) {
        return None;
    }
    let items = rvalue::read_usize(payload + rvalue::ARRAY_DATA)?;
    if items == 0 || !win::readable(items, len as usize * 16) {
        return None;
    }

    for i in 0..len as usize {
        let at = items + i * 16;
        if !matches!(rvalue::decode(at), Some(Value::Str(ref s)) if s == name) {
            continue;
        }
        // Hand the runtime back its own string RValue, verbatim.
        let key = core::ptr::read_unaligned(at as *const RValueRaw);
        let args = [arg, key];
        let mut out = RValueRaw { payload: 0, flags: 0, kind: KIND_UNDEFINED };
        get_fn(&mut out, core::ptr::null_mut(), core::ptr::null_mut(), 2, args.as_ptr());
        // Decode it where it sits, so there is one decoder rather than two.
        return rvalue::decode(core::ptr::addr_of!(out) as usize);
    }
    None
}

/// An RValue as the runtime expects it on the stack.
pub fn raw_of(v: &Value) -> RValueRaw {
    match v {
        Value::Object(p) => RValueRaw { payload: *p as u64, flags: 0, kind: 6 },
        Value::Array(p) => RValueRaw { payload: *p as u64, flags: 0, kind: 2 },
        Value::Ref { ref_type, id } => RValueRaw {
            payload: ((*ref_type as u64) << 32) | (*id as u32 as u64),
            flags: 0,
            kind: 15,
        },
        Value::Real(f) => RValueRaw { payload: f.to_bits(), flags: 0, kind: 0 },
        Value::Int(i) => RValueRaw { payload: *i as u64, flags: 0, kind: 10 },
        Value::Bool(b) => RValueRaw { payload: *b as u64, flags: 0, kind: 13 },
        _ => RValueRaw { payload: 0, flags: 0, kind: 5 },
    }
}

pub fn resolve(base: usize, rva: usize, text: (usize, usize)) -> Option<Builtin> {
    let addr = base + rva;
    if addr < text.0 || addr >= text.1 {
        return None;
    }
    Some(unsafe { core::mem::transmute::<usize, Builtin>(addr) })
}

/// The member names a struct actually has.
///
/// # Safety
/// Must be called on the game's thread.
pub unsafe fn struct_member_names(
    base: usize,
    text: (usize, usize),
    strukt: &Value,
) -> Option<Vec<String>> {
    let Value::Object(ptr) = strukt else { return None };
    if *ptr == 0 || !win::readable(*ptr, 8) {
        return None;
    }
    let f = resolve(base, VARIABLE_STRUCT_GET_NAMES_RVA, text)?;

    let arg = RValueRaw { payload: *ptr as u64, flags: 0, kind: 6 };
    let mut result = RValueRaw { payload: 0, flags: 0, kind: KIND_UNDEFINED };
    f(&mut result, core::ptr::null_mut(), core::ptr::null_mut(), 1, &arg);

    if result.kind != rvalue::KIND_ARRAY {
        return None;
    }
    let payload = result.payload as usize;
    if !win::readable(payload, rvalue::ARRAY_LEN + 4) {
        return None;
    }
    let len = rvalue::read_i32(payload + rvalue::ARRAY_LEN)?;
    if !(0..512).contains(&len) {
        return None;
    }
    let items = rvalue::read_usize(payload + rvalue::ARRAY_DATA)?;
    if items == 0 || !win::readable(items, len as usize * 16) {
        return None;
    }

    let mut out = Vec::new();
    for i in 0..len as usize {
        match rvalue::decode(items + i * 16) {
            Some(Value::Str(s)) => out.push(s),
            other => out.push(format!("<{other:?}>")),
        }
    }
    Some(out)
}
