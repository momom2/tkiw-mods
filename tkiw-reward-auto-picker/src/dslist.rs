//! Reading `ds_list` and `ds_map` references, without calling the game.
//!
//! The reward queue is `pending_rewards_list`, a `ds_list`. A kind-15 RValue
//! carries `{high dword = ref type, low dword = id}`; the id indexes a table of
//! containers held by the runtime.
//!
//! The runtime's own accessors are available as builtins, but they are not used
//! here. `ds_list_find_value` and friends route through a validator that calls
//! `YYError` on a bad reference -- a fatal dialog, not an error return -- and
//! the accessors themselves turn out to be trivial:
//!
//! ```text
//! ds_list_size(l)      -> *(i32*)(l + 0x08)
//! ds_list_find_value   -> index bounds-checked against +0x08,
//!                         then *(RValue**)(l + 0x18) + index * 16
//! ```
//!
//! So the mod reads them directly. Every hop is validated, nothing is called,
//! and a malformed reference produces `None` rather than a dialog box in
//! someone's game.

use crate::rvalue::{self, Value};
use crate::win;

/// Ref-type tags, from the runtime's own table at `.data 0x2974010`.
pub const REF_DS_LIST: u32 = 0x0200_0001;
pub const REF_DS_MAP: u32 = 0x0200_0002;

/// Manager globals, lifted from `ds_list_size`.
const DS_LIST_TABLE_RVA: usize = 0x2affde8;
const DS_LIST_COUNT_RVA: usize = 0x2affdf0;
/// And from `ds_map_find_value`.
const DS_MAP_TABLE_RVA: usize = 0x2affdd8;
const DS_MAP_COUNT_RVA: usize = 0x2affdcc;

const LIST_SIZE: usize = 0x08;
const LIST_ITEMS: usize = 0x18;

/// Refuse absurd sizes rather than walk memory for a long time.
const MAX_ELEMENTS: i32 = 100_000;

fn read<T: Copy>(addr: usize) -> Option<T> {
    if !win::readable(addr, core::mem::size_of::<T>()) {
        return None;
    }
    Some(unsafe { core::ptr::read_volatile(addr as *const T) })
}

/// Split a kind-15 payload into its ref type and id.
pub fn split_ref(raw: u64) -> (u32, i32) {
    ((raw >> 32) as u32, raw as u32 as i32)
}

fn container(base: usize, table_rva: usize, count_rva: usize, id: i32) -> Option<usize> {
    let count: i32 = read(base + count_rva)?;
    if id < 0 || count <= 0 || id >= count {
        return None;
    }
    let table: usize = read(base + table_rva)?;
    if table == 0 {
        return None;
    }
    let slot: usize = read(table + id as usize * 8)?;
    (slot != 0).then_some(slot)
}

pub struct DsList {
    ptr: usize,
    len: i32,
    items: usize,
}

impl DsList {
    /// Resolve a `ds_list` from the RValue that names it.
    pub fn from_value(base: usize, v: &Value) -> Option<DsList> {
        let Value::Ref { ref_type, id } = v else { return None };
        if *ref_type != REF_DS_LIST {
            return None;
        }
        Self::from_id(base, *id)
    }

    pub fn from_id(base: usize, id: i32) -> Option<DsList> {
        let ptr = container(base, DS_LIST_TABLE_RVA, DS_LIST_COUNT_RVA, id)?;
        let len: i32 = read(ptr + LIST_SIZE)?;
        if len < 0 || len > MAX_ELEMENTS {
            return None;
        }
        let items: usize = read(ptr + LIST_ITEMS)?;
        if len > 0 && (items == 0 || !win::readable(items, len as usize * 16)) {
            return None;
        }
        Some(DsList { ptr, len, items })
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn ptr(&self) -> usize {
        self.ptr
    }

    /// Address of element `i`, exactly as `ds_list_find_value` computes it.
    pub fn element_addr(&self, i: usize) -> Option<usize> {
        (i < self.len as usize).then(|| self.items + i * 16)
    }

    pub fn get(&self, i: usize) -> Option<Value> {
        rvalue::decode(self.element_addr(i)?)
    }
}

/// A `ds_map`'s backing pointer, for the library maps.
pub fn ds_map_ptr(base: usize, v: &Value) -> Option<usize> {
    let Value::Ref { ref_type, id } = v else { return None };
    if *ref_type != REF_DS_MAP {
        return None;
    }
    container(base, DS_MAP_TABLE_RVA, DS_MAP_COUNT_RVA, *id)
}

// ds_map hash table, from the lookup at 0x1aeae50.
const MAP_TABLE: usize = 0x00;
const HT_BUCKETS: usize = 0x00;
const HT_MASK: usize = 0x08;
const BUCKET_NEXT: usize = 0x08;
const BUCKET_PAIR: usize = 0x18;
const PAIR_VALUE: usize = 0x10;

/// Every `(key, value)` in a `ds_map`, by walking its buckets.
///
/// Used to enumerate the game's own libraries -- `REWARDS`, `RESOURCES`,
/// `ARTIFACTS` and friends are all `ds_map<id, struct>`. Walking beats calling
/// `ds_map_find_first`/`_next` because it needs no calls at all, and the
/// runtime's own accessors fault fatally on a bad reference.
pub fn ds_map_entries(base: usize, v: &Value, limit: usize) -> Option<Vec<(Value, Value)>> {
    let map = ds_map_ptr(base, v)?;
    let ht: usize = read(map + MAP_TABLE)?;
    if ht == 0 {
        return None;
    }
    let buckets: usize = read(ht + HT_BUCKETS)?;
    let mask: i32 = read(ht + HT_MASK)?;
    if buckets == 0 || mask <= 0 || mask > 1 << 20 {
        return None;
    }

    let mut out = Vec::new();
    for slot in 0..=mask as usize {
        let Some(mut node) = read::<usize>(buckets + slot * 16) else { continue };
        let mut hops = 0;
        while node != 0 && hops < 4096 && out.len() < limit {
            if let Some(pair) = read::<usize>(node + BUCKET_PAIR) {
                if pair != 0 && win::readable(pair, 32) {
                    if let (Some(k), Some(val)) =
                        (rvalue::decode(pair), rvalue::decode(pair + PAIR_VALUE))
                    {
                        out.push((k, val));
                    }
                }
            }
            node = read::<usize>(node + BUCKET_NEXT).unwrap_or(0);
            hops += 1;
        }
        if out.len() >= limit {
            break;
        }
    }
    Some(out)
}

/// Read a named member from a struct-valued RValue (kind 6), using the same
/// virtual interface as instances and globals.
///
/// # Safety
/// Must be called on the game's thread.
pub unsafe fn struct_member(v: &Value, var_id: u32) -> Option<Value> {
    let Value::Object(obj) = v else { return None };
    let obj = *obj;
    if obj == 0 || !win::readable(obj, 8) {
        return None;
    }
    let vt: usize = read(obj)?;
    let get: usize = read(vt + 8)?;
    if get == 0 {
        return None;
    }
    let f: unsafe extern "system" fn(usize, u32) -> usize = core::mem::transmute(get);
    let p = f(obj, var_id);
    if p == 0 || !win::readable(p, 16) {
        return None;
    }
    rvalue::decode(p)
}
