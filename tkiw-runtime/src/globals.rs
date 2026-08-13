//! Reading GML global variables from the live game.
//!
//! Compiled GML reaches a global through a container object with a small
//! virtual interface:
//!
//! ```text
//!   mov rcx, [rip + GLOBAL_CONTAINER]   ; the container
//!   mov rax, [rcx]                      ; its vtable
//!   mov edx, [rip + <variable slot>]    ; the variable id
//!   call qword ptr [rax + 8]            ; -> RValue*
//! ```
//!
//! `[rax+8]` reads and `[rax+0x10]` gets-for-write; this module only ever uses
//! the first. The pattern was confirmed across the reward code before being
//! relied on: 255 read sites, 196 write sites, and one dominant container
//! address used by 66 of them.
//!
//! **This is the mod's only baked address.** Everything else resolves by name.
//! There is no symbol for the container, so instead of pretending otherwise the
//! whole pointer chain is validated before use and a failed check disables the
//! feature rather than guessing.
//!
//! Calling into the runtime is only safe on the game's own thread, so every
//! entry point here must be reached from the frame hook.

use crate::rvalue::{self, Value};
use crate::win;

/// RVA of the pointer to the global variable container.
const GLOBAL_CONTAINER_RVA: usize = 0x2af7a08;
/// Vtable slot for "read this variable".
const VT_GET: usize = 8;

type GetVar = unsafe extern "system" fn(container: usize, var_id: u32) -> usize;

pub struct Globals {
    container: usize,
    get: usize,
}

impl Globals {
    /// Resolve and validate the whole chain. `text` bounds the game's code, so
    /// a function pointer that does not land in it is rejected before it can be
    /// called.
    pub fn resolve(base: usize, text: (usize, usize)) -> Result<Globals, String> {
        let slot = base + GLOBAL_CONTAINER_RVA;
        if !win::readable(slot, 8) {
            return Err(format!("container slot {slot:#x} not readable"));
        }
        let container = unsafe { core::ptr::read_volatile(slot as *const usize) };
        if container == 0 || !win::readable(container, 8) {
            return Err(format!("container pointer {container:#x} not readable"));
        }
        let vtable = unsafe { core::ptr::read_volatile(container as *const usize) };
        if vtable == 0 || !win::readable(vtable + VT_GET, 8) {
            return Err(format!("vtable {vtable:#x} not readable"));
        }
        let get = unsafe { core::ptr::read_volatile((vtable + VT_GET) as *const usize) };
        if get < text.0 || get >= text.1 {
            return Err(format!(
                "get-variable {get:#x} is outside the game's code {:#x}..{:#x}",
                text.0, text.1
            ));
        }
        Ok(Globals { container, get })
    }

    pub fn container(&self) -> usize {
        self.container
    }

    pub fn getter(&self) -> usize {
        self.get
    }

    /// Read a global by variable id.
    ///
    /// # Safety
    /// Must be called on the game's thread. GameMaker is single-threaded and
    /// this reaches into its runtime.
    pub unsafe fn get_raw(&self, var_id: u32) -> Option<usize> {
        let f: GetVar = core::mem::transmute(self.get);
        let p = f(self.container, var_id);
        (p != 0 && win::readable(p, 16)).then_some(p)
    }

    /// Read and decode a global by variable id.
    ///
    /// # Safety
    /// Must be called on the game's thread.
    pub unsafe fn get(&self, var_id: u32) -> Option<Value> {
        rvalue::decode(self.get_raw(var_id)?)
    }

    /// Read a variable off **an instance or a struct**, rather than off the global
    /// container.
    ///
    /// The single most useful property of this interface, from
    /// `runtime-structures.md`: the same `vtable+8` getter serves globals, instance
    /// variables and struct members. Only the first argument changes. So one resolved
    /// `Globals` reads all three, and a feature that has found an instance pointer via
    /// [`crate::instance`] can read its fields with no further machinery.
    ///
    /// `owner` is a `CInstance*` or a struct pointer. A wrong pointer produces `None`
    /// rather than a fault, because the result is validated before it is decoded —
    /// but note that this *calls into the GML runtime*, so the usual rule applies:
    /// reads through here are only safe on the game's own thread.
    ///
    /// # Safety
    /// Must be called on the game's thread, and `owner` must be a pointer the game
    /// itself would accept here — an instance or a struct, not an arbitrary address.
    pub unsafe fn get_on(&self, owner: usize, var_id: u32) -> Option<Value> {
        if owner == 0 || !win::readable(owner, 8) {
            return None;
        }
        let f: GetVar = core::mem::transmute(self.get);
        let p = f(owner, var_id);
        if p == 0 || !win::readable(p, 16) {
            return None;
        }
        rvalue::decode(p)
    }

    /// As [`Globals::get_on`], accepting any of the numeric kinds the game emits.
    ///
    /// Worth having as its own function because of a mistake that costs a session:
    /// fields that look like plain numbers frequently come back as **int64**, so a
    /// reader that only accepts kind 0 silently never matches — which looks exactly
    /// like the field being absent.
    ///
    /// # Safety
    /// As [`Globals::get_on`].
    pub unsafe fn num_on(&self, owner: usize, var_id: u32) -> Option<i64> {
        match self.get_on(owner, var_id)? {
            Value::Int(v) => Some(v),
            Value::Real(v) => Some(v as i64),
            Value::Bool(b) => Some(b as i64),
            _ => None,
        }
    }
}
