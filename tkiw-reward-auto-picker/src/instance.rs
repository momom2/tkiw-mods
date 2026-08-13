//! Finding a live object instance, using only reads.
//!
//! The reward queue lives on `obj_gameplay_controller` as an instance variable,
//! so the mod needs a `self` pointer. The obvious way to get one is to hook a
//! function belonging to the object and take the `self` it is handed -- but
//! that means patching the game's code.
//!
//! It turns out not to be necessary. The runtime keeps a name-keyed object
//! registry, and its own `Object_Find` is a leaf function that does nothing but
//! walk it. Reproducing that walk here means the mod never writes to `.text`
//! and never calls a game function just to locate an instance.
//!
//! ```text
//! registry = *(base + 0x2b011d8)   { void** buckets @0, i32 mask @8, i32 count @0xc }
//! slot     = buckets[(objindex & mask) * 2]        ; slots are 16-byte {head,tail}
//! node     = { next @0x08, i32 key @0x10, void* value @0x18 }
//! ```
//!
//! Every hop is validated and every failure is `None`. Outside gameplay the
//! controller genuinely does not exist, so `None` is a normal answer here, not
//! an error.

use crate::win;

/// RVA of the object registry descriptor.
const OBJECT_REGISTRY_RVA: usize = 0x2b011d8;

// CObjectGML
const OBJ_NAME: usize = 0x00;
const OBJ_INSTANCE_LIST: usize = 0x68;
const OBJ_INDEX: usize = 0x94;
// instance list node
const NODE_NEXT: usize = 0x00;
const NODE_INSTANCE: usize = 0x10;
// registry node
const RNODE_NEXT: usize = 0x08;
const RNODE_KEY: usize = 0x10;
const RNODE_VALUE: usize = 0x18;
// CInstance
const INST_OBJECT: usize = 0x90;
const INST_FLAGS: usize = 0xB8;
const INST_ID: usize = 0xBC;

const FLAG_ALIVE: u32 = 0x4;
const FLAG_DEAD: u32 = 0x0010_0003;
const MIN_INSTANCE_ID: i32 = 100_000;

/// Guards against a corrupt or unexpected registry sending us into a long walk.
const MAX_BUCKETS: i64 = 1 << 20;
const MAX_CHAIN: usize = 4096;

fn read<T: Copy>(addr: usize) -> Option<T> {
    if !win::readable(addr, core::mem::size_of::<T>()) {
        return None;
    }
    Some(unsafe { core::ptr::read_volatile(addr as *const T) })
}

fn c_str_eq(addr: usize, want: &str) -> bool {
    let bytes = want.as_bytes();
    if !win::readable(addr, bytes.len() + 1) {
        return false;
    }
    for (i, w) in bytes.iter().enumerate() {
        let b: u8 = unsafe { core::ptr::read_volatile((addr + i) as *const u8) };
        if b != *w {
            return false;
        }
    }
    let term: u8 = unsafe { core::ptr::read_volatile((addr + bytes.len()) as *const u8) };
    term == 0
}

pub struct Registry {
    buckets: usize,
    mask: i64,
}

impl Registry {
    pub fn open(base: usize) -> Option<Registry> {
        let desc: usize = read(base + OBJECT_REGISTRY_RVA)?;
        if desc == 0 {
            return None;
        }
        let buckets: usize = read(desc)?;
        let mask: i32 = read(desc + 8)?;
        if buckets == 0 || mask <= 0 || mask as i64 > MAX_BUCKETS {
            return None;
        }
        Some(Registry { buckets, mask: mask as i64 })
    }

    /// Every `(object index, CObjectGML*)` in the registry.
    pub fn objects(&self) -> Vec<(i32, usize)> {
        let mut out = Vec::new();
        for slot in 0..=self.mask {
            let Some(mut node) = read::<usize>(self.buckets + (slot as usize * 2) * 8) else {
                continue;
            };
            let mut hops = 0;
            while node != 0 && hops < MAX_CHAIN {
                if let (Some(key), Some(value)) =
                    (read::<i32>(node + RNODE_KEY), read::<usize>(node + RNODE_VALUE))
                {
                    if value != 0 {
                        out.push((key, value));
                    }
                }
                node = read::<usize>(node + RNODE_NEXT).unwrap_or(0);
                hops += 1;
            }
        }
        out
    }

    /// The `CObjectGML*` for a named object, with its index.
    ///
    /// Cached: this walks all ~1,750 registry entries, and doing that per
    /// lookup per poll made the game unplayable. Object records are created at
    /// load and do not move, so the cache is good for the life of the process
    /// -- but the name is re-checked on every hit, so a stale or wrong entry
    /// falls back to a fresh walk instead of being trusted.
    pub fn find_object(&self, name: &str) -> Option<(i32, usize)> {
        if let Ok(c) = cache().lock() {
            match c.get(name) {
                // a name that is not an object stays not an object; caching the
                // miss matters as much as caching the hit, or every lookup for
                // it walks the whole registry again
                Some(None) => return None,
                Some(&Some((index, obj))) => {
                    if read::<usize>(obj + OBJ_NAME).is_some_and(|p| p != 0 && c_str_eq(p, name))
                        && read::<i32>(obj + OBJ_INDEX) == Some(index)
                    {
                        return Some((index, obj));
                    }
                }
                None => {}
            }
        }

        let all = self.objects();
        let found = self.find_in(&all, name);
        if let Ok(mut c) = cache().lock() {
            // only record a miss once the registry is clearly populated, so a
            // lookup made too early cannot poison the cache for the session
            if found.is_some() || all.len() > 100 {
                c.insert(name.to_string(), found);
            }
        }
        found
    }

    fn find_in(&self, all: &[(i32, usize)], name: &str) -> Option<(i32, usize)> {
        for &(index, obj) in all {
            let Some(name_ptr) = read::<usize>(obj + OBJ_NAME) else { continue };
            if name_ptr == 0 || !c_str_eq(name_ptr, name) {
                continue;
            }
            if read::<i32>(obj + OBJ_INDEX) != Some(index) {
                return None;
            }
            return Some((index, obj));
        }
        None
    }

}

type ObjectCache = std::sync::Mutex<std::collections::HashMap<String, Option<(i32, usize)>>>;

fn cache() -> &'static ObjectCache {
    static CACHE: std::sync::OnceLock<ObjectCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// The single live instance of a named object, or `None`.
///
/// `None` is the correct answer whenever the object is not instantiated -- in
/// menus, or between runs -- and callers must treat it as ordinary.
pub fn find_singleton(base: usize, name: &str) -> Option<usize> {
    let reg = Registry::open(base)?;
    let (_index, obj) = reg.find_object(name)?;

    let mut node: usize = read(obj + OBJ_INSTANCE_LIST)?;
    let mut hops = 0;
    while node != 0 && hops < MAX_CHAIN {
        if let Some(inst) = read::<usize>(node + NODE_INSTANCE) {
            if inst == 0 {
                return None; // sentinel tail
            }
            if is_live(inst, obj) {
                return Some(inst);
            }
        }
        node = read::<usize>(node + NODE_NEXT).unwrap_or(0);
        hops += 1;
    }
    None
}

/// Four independent checks, so a layout change in a game update produces a
/// clean `None` rather than a wild pointer.
fn is_live(inst: usize, obj: usize) -> bool {
    if !win::readable(inst, INST_ID + 4) {
        return false;
    }
    let Some(flags) = read::<u32>(inst + INST_FLAGS) else { return false };
    if flags & FLAG_ALIVE == 0 || flags & FLAG_DEAD != 0 {
        return false;
    }
    if read::<usize>(inst + INST_OBJECT) != Some(obj) {
        return false; // back-pointer must agree
    }
    match read::<i32>(inst + INST_ID) {
        Some(id) if id >= MIN_INSTANCE_ID => {}
        _ => return false,
    }
    // must have a plausible vtable, since that is what variable access uses
    match read::<usize>(inst) {
        Some(vt) if vt != 0 && win::readable(vt + 0x10, 8) => true,
        _ => false,
    }
}

/// Every live instance of a named object.
///
/// Option cards come in threes, so the singleton helper is not enough.
pub fn find_all(base: usize, name: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let Some(reg) = Registry::open(base) else { return out };
    let Some((_index, obj)) = reg.find_object(name) else { return out };

    let Some(mut node) = read::<usize>(obj + OBJ_INSTANCE_LIST) else { return out };
    let mut hops = 0;
    while node != 0 && hops < MAX_CHAIN {
        match read::<usize>(node + NODE_INSTANCE) {
            Some(0) | None => break, // sentinel tail
            Some(inst) => {
                if is_live(inst, obj) {
                    out.push(inst);
                }
            }
        }
        node = read::<usize>(node + NODE_NEXT).unwrap_or(0);
        hops += 1;
    }
    out
}

/// How many live instances a named object has, without collecting them.
///
/// Counted in place rather than through `find_all`: the mod asks this of every
/// card object every frame, and building a `Vec` to take its length was a heap
/// allocation per question, fifteen or so per frame, for a number.
pub fn count(base: usize, name: &str) -> usize {
    let Some(reg) = Registry::open(base) else { return 0 };
    let Some((_index, obj)) = reg.find_object(name) else { return 0 };
    let Some(mut node) = read::<usize>(obj + OBJ_INSTANCE_LIST) else { return 0 };

    let mut n = 0;
    let mut hops = 0;
    while node != 0 && hops < MAX_CHAIN {
        match read::<usize>(node + NODE_INSTANCE) {
            Some(0) | None => break, // sentinel tail
            Some(inst) => {
                if is_live(inst, obj) {
                    n += 1;
                }
            }
        }
        node = read::<usize>(node + NODE_NEXT).unwrap_or(0);
        hops += 1;
    }
    n
}

/// Read an instance variable, using the same interface as globals.
///
/// # Safety
/// Must be called on the game's thread.
pub unsafe fn get_var(inst: usize, var_id: u32) -> Option<usize> {
    let vt: usize = read(inst)?;
    let get: usize = read(vt + 8)?;
    if get == 0 {
        return None;
    }
    let f: unsafe extern "system" fn(usize, u32) -> usize = core::mem::transmute(get);
    let p = f(inst, var_id);
    (p != 0 && win::readable(p, 16)).then_some(p)
}

// Instance-id lookup. Instance references carry an id (>= 100000), not a
// pointer, so pressing a button a card refers to means resolving the id first.
const INSTANCE_HASH_RVA: usize = 0x2974450;
const INSTANCE_MASK_RVA: usize = 0x2974458;
const IHASH_NEXT: usize = 0x08;
const IHASH_ID: usize = 0x10;
const IHASH_INSTANCE: usize = 0x18;

/// The live `CInstance*` for an instance id, or `None`.
///
/// Walks the runtime's own id hash. Validated like every other hop: a dead or
/// recycled id yields `None` rather than a stale pointer.
pub fn by_id(base: usize, id: i32) -> Option<usize> {
    if id < MIN_INSTANCE_ID {
        return None;
    }
    let buckets: usize = read(base + INSTANCE_HASH_RVA)?;
    let mask: i32 = read(base + INSTANCE_MASK_RVA)?;
    if buckets == 0 || mask <= 0 {
        return None;
    }
    let slot = (id & mask) as usize;
    let mut node: usize = read(buckets + slot * 16)?;
    let mut hops = 0;
    while node != 0 && hops < MAX_CHAIN {
        if read::<i32>(node + IHASH_ID) == Some(id) {
            let inst: usize = read(node + IHASH_INSTANCE)?;
            if inst != 0 && win::readable(inst, INST_ID + 4) {
                // the instance must agree that it has this id
                if read::<i32>(inst + INST_ID) == Some(id) {
                    return Some(inst);
                }
            }
            return None;
        }
        node = read::<usize>(node + IHASH_NEXT).unwrap_or(0);
        hops += 1;
    }
    None
}
