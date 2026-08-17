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

/// The RValue the getters want for an owner: a struct pointer as an object,
/// or an instance by **id** -- `variable_instance_get_names` and friends are
/// one implementation with the struct variants, but an instance handed over
/// as a raw pointer comes back `undefined`; only the id form works. Learned
/// from a live session where every unit enumeration failed.
fn owner_arg(owner: &Value) -> Option<RValueRaw> {
    match owner {
        Value::Object(ptr) => {
            if *ptr == 0 || !win::readable(*ptr, 8) {
                return None;
            }
            Some(RValueRaw { payload: *ptr as u64, flags: 0, kind: rvalue::KIND_OBJECT })
        }
        Value::Int(id) if *id >= 100_000 => {
            Some(RValueRaw { payload: (*id as f64).to_bits(), flags: 0, kind: rvalue::KIND_REAL })
        }
        _ => None,
    }
}

/// Read a struct member or instance variable by *name*, without needing a
/// variable id for it. `owner` is a struct pointer (`Value::Object`) or an
/// instance id (`Value::Int`).
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
    let arg = owner_arg(strukt)?;
    let names_fn = resolve(base, VARIABLE_STRUCT_GET_NAMES_RVA, text)?;
    let get_fn = resolve(base, VARIABLE_STRUCT_GET_RVA, text)?;

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
    // Units carry many hundreds of members, far past the old 512 sanity bound;
    // a real session hit it. Still bounded, against garbage rather than size.
    if !(0..8192).contains(&len) {
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

/// Every member name and value of a struct or instance, in one enumeration.
///
/// Exists because [`struct_get_by_name`] re-enumerates the whole names array
/// per call -- fine for one field, O(n^2) for a full dump, and a 625-member
/// unit held the game's thread for 373ms that way. This walks the names once
/// and does one get per member. Returns the pairs and the true member count,
/// so a caller that caps can say so.
///
/// # Safety
/// Game thread only.
pub unsafe fn struct_members(
    base: usize,
    text: (usize, usize),
    owner: &Value,
    cap: usize,
) -> Option<(Vec<(String, Option<Value>)>, usize)> {
    let arg = owner_arg(owner)?;
    let names_fn = resolve(base, VARIABLE_STRUCT_GET_NAMES_RVA, text)?;
    let get_fn = resolve(base, VARIABLE_STRUCT_GET_RVA, text)?;

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
    if !(0..8192).contains(&len) {
        return None;
    }
    let items = rvalue::read_usize(payload + rvalue::ARRAY_DATA)?;
    if items == 0 || !win::readable(items, len as usize * 16) {
        return None;
    }

    let mut out = Vec::with_capacity((len as usize).min(cap));
    for i in 0..(len as usize).min(cap) {
        let at = items + i * 16;
        let Some(Value::Str(name)) = rvalue::decode(at) else { continue };
        // Hand the runtime back its own string RValue as the key, verbatim.
        let key = core::ptr::read_unaligned(at as *const RValueRaw);
        let args = [arg, key];
        let mut v = RValueRaw { payload: 0, flags: 0, kind: KIND_UNDEFINED };
        get_fn(&mut v, core::ptr::null_mut(), core::ptr::null_mut(), 2, args.as_ptr());
        out.push((name, rvalue::decode(core::ptr::addr_of!(v) as usize)));
    }
    Some((out, len as usize))
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

/// The RVA of a named runtime builtin, from the generated table.
///
/// Linear over ~2,800 entries; callers that resolve in a loop should keep the
/// result. Names with two spellings (`draw_set_colour`/`draw_set_color`) each
/// have their own row, so either finds it.
pub fn by_name(name: &str) -> Option<u32> {
    crate::builtins_table::BUILTINS
        .iter()
        .find(|(_, n)| *n == name)
        .map(|(rva, _)| *rva)
}

/// Call a named builtin and decode its result.
///
/// For **read-only lookups taking numeric arguments** -- `font_exists`,
/// `font_get_name`, `display_get_gui_width`. Nothing here can pass a GML
/// string, and nothing should: allocating one means asking the game for
/// memory. At most eight arguments, which is more than any lookup wants.
///
/// # Safety
/// Game thread only. The caller vouches that the named builtin is a pure
/// lookup -- this function cannot know what the callee would mutate.
pub unsafe fn call_by_name(
    base: usize,
    text: (usize, usize),
    name: &str,
    args: &[RValueRaw],
) -> Option<Value> {
    let f = resolve(base, by_name(name)? as usize, text)?;
    let mut buf = [RValueRaw { payload: 0, flags: 0, kind: KIND_UNDEFINED }; 8];
    if args.len() > buf.len() {
        return None;
    }
    buf[..args.len()].copy_from_slice(args);
    let mut out = RValueRaw { payload: 0, flags: 0, kind: KIND_UNDEFINED };
    f(&mut out, core::ptr::null_mut(), core::ptr::null_mut(), args.len() as i32, buf.as_ptr());
    rvalue::decode(core::ptr::addr_of!(out) as usize)
}

/// The member names a struct or instance actually has. `owner` as in
/// [`struct_get_by_name`]: a struct pointer, or an instance id.
///
/// # Safety
/// Must be called on the game's thread.
pub unsafe fn struct_member_names(
    base: usize,
    text: (usize, usize),
    strukt: &Value,
) -> Option<Vec<String>> {
    let arg = owner_arg(strukt)?;
    let f = resolve(base, VARIABLE_STRUCT_GET_NAMES_RVA, text)?;

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
    // Units carry many hundreds of members, far past the old 512 sanity bound;
    // a real session hit it. Still bounded, against garbage rather than size.
    if !(0..8192).contains(&len) {
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
