//! Pressing a button.
//!
//! This is the only code in the mod that *acts*. Everything else reads.
//!
//! **How, and why this way.** The game presses a button by invoking that
//! instance's `button_pressed_action` method -- `obj_cursor` detects the hover
//! and calls it. There is no flag to set, so something must invoke the method.
//!
//! Doing that through the compiled-GML calling convention would mean getting a
//! register order right that has never been exercised here, where a mistake
//! corrupts memory instead of failing cleanly. It is not necessary: the runtime
//! exposes `method_call` and `script_execute` as **builtins**, and the builtin
//! ABI is the one already proven by `variable_struct_get_names`. So the mod
//! calls a builtin, and the builtin calls the method.
//!
//! Every press is guarded:
//!
//! * the instance must still exist and be live
//! * the member must actually be a method, checked with the game's own
//!   `is_method`, not assumed
//! * the whole thing runs inside `catch_unwind`, on the game's own thread
//! * and it is rate-limited, because a press that does not take effect must not
//!   become a press repeated sixty times a second

use crate::builtin::{self, RValueRaw};
use crate::rvalue::Value;
use crate::{instance, logln, win, State};

/// `is_method(value) -> bool`, argc 1.
const IS_METHOD_RVA: usize = 0x1adab90;
/// `script_execute(callable, ...args)`, variadic.
///
/// Preferred over `method_call`, which wants its arguments as a GML array --
/// and allocating one would mean asking the runtime for memory just to express
/// "no arguments". `script_execute` takes the callable directly.
const SCRIPT_EXECUTE_RVA: usize = 0x1c4ebc0;

/// Never press the same button twice faster than this.
///
/// A press that does not visibly change anything is a bug to investigate, not
/// something to repeat sixty times a second. Rerolling presses the *same*
/// button repeatedly, so this tracks the pace the player set rather than a
/// fixed figure -- with a floor, because no configuration should be able to ask
/// for an unbounded press rate.
const REPRESS_FLOOR: std::time::Duration = std::time::Duration::from_millis(40);

static REPRESS_GUARD: std::sync::Mutex<std::time::Duration> =
    std::sync::Mutex::new(std::time::Duration::from_millis(100));

/// Set the minimum gap between presses of the same button, from `delay_ms`.
pub fn set_pace(delay_ms: u64) {
    if let Ok(mut g) = REPRESS_GUARD.lock() {
        *g = std::time::Duration::from_millis(delay_ms).max(REPRESS_FLOOR);
    }
}

fn repress_guard() -> std::time::Duration {
    REPRESS_GUARD.lock().map(|g| *g).unwrap_or(REPRESS_FLOOR)
}

/// Whether `v` really is a callable method, according to the game.
///
/// # Safety
/// Game thread only.
unsafe fn is_method(base: usize, text: (usize, usize), inst: usize, v: &Value) -> bool {
    let Some(f) = builtin::resolve(base, IS_METHOD_RVA, text) else {
        return false;
    };
    let arg = builtin::raw_of(v);
    let mut out = RValueRaw { payload: 0, flags: 0, kind: 5 };
    let me = inst as *mut u8;
    f(&mut out, me, me, 1, &arg);
    // a bool comes back as kind 13, or as a real 1/0
    match out.kind {
        13 => out.payload != 0,
        0 => f64::from_bits(out.payload) != 0.0,
        7 | 10 => out.payload != 0,
        _ => false,
    }
}

/// Invoke an instance's method by member name.
///
/// Returns `Err` with a reason rather than pressing anything it is not sure
/// about. A refusal is always safe; a wrong press is not.
///
/// # Safety
/// Game thread only.
pub unsafe fn invoke(
    state: &State,
    base: usize,
    inst: usize,
    member: &str,
) -> Result<(), String> {
    if !win::readable(inst, 0xC0) {
        return Err(format!("instance {inst:#x} is not readable"));
    }
    let Some(id) = state.syms.var_id(member) else {
        return Err(format!("no variable id for {member}"));
    };
    let Some(rv) = instance::get_var(inst, id) else {
        return Err(format!("{member} not present on {inst:#x}"));
    };
    let Some(v) = crate::rvalue::decode(rv) else {
        return Err(format!("{member} did not decode"));
    };
    if !is_method(base, state.text, inst, &v) {
        return Err(format!("{member} is not a method ({v:?})"));
    }

    let Some(f) = builtin::resolve(base, SCRIPT_EXECUTE_RVA, state.text) else {
        return Err("script_execute is not where it was expected".into());
    };
    // `self` MUST be the instance that owns the method.
    //
    // Passing null crashed the game: `button_pressed_action` opens with
    // `mov rcx,[rbp+0x70]; mov rax,[rcx]` -- it loads `self` and immediately
    // dereferences it to read `self.card_parent`. The earlier builtin call that
    // worked (`variable_struct_get_names`) simply never touches `self`, which
    // hid the omission.
    let arg = builtin::raw_of(&v);
    let mut out = RValueRaw { payload: 0, flags: 0, kind: 5 };
    let me = inst as *mut u8;
    f(&mut out, me, me, 1, &arg);
    Ok(())
}

/// Press a button instance, with the guards described above.
/// The operation currently in flight, written to the log before it starts.
///
/// The log records completed actions, so a crash shows what last *succeeded* --
/// not what was underway when it died. This narrows that to one line.
pub fn trace(what: &str) {
    // The phase buffer now lives in the shared runtime, where the crash reporter reads
    // it directly -- this mod no longer owns either half.
    tkiw_runtime::phase::note(what);
    if TRACING.load(std::sync::atomic::Ordering::Relaxed) {
        crate::logln!("[.] {what}");
    }
}

/// The phase buffer and its reader now live in `tkiw_runtime::phase`, together with
/// the crash reporter that is their only consumer. Both halves used to be here, which
/// meant the shared reporter had to reach back into this mod to find out what was
/// under way -- exactly the coupling the shared runtime exists to remove.
///
/// See `tkiw_runtime::phase` for why the phase is recorded unconditionally rather than
/// behind `trace`.
pub use tkiw_runtime::phase::copy as copy_phase;

/// Whether `[global] trace` is on, so the trace points scattered through the
/// resolve path do not each have to be handed the config to ask.
static TRACING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_tracing(on: bool) {
    TRACING.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn press(state: &State, base: usize, inst: usize, what: &str) -> Result<(), String> {
    {
        static LAST: std::sync::Mutex<Option<(usize, std::time::Instant)>> =
            std::sync::Mutex::new(None);
        let mut g = match LAST.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        if let Some((prev, when)) = *g {
            if prev == inst && when.elapsed() < repress_guard() {
                return Err("already pressed this button moments ago".into());
            }
        }
        *g = Some((inst, std::time::Instant::now()));
    }

    // The log records what *succeeded*, so a crash during a press left the
    // `[PICK]` of the previous one as the last line and nothing to say the next
    // press had even begun.
    trace(&format!("pressing {what} @{inst:#x}"));
    let result = std::panic::catch_unwind(|| unsafe {
        invoke(state, base, inst, "button_pressed_action")
    });
    match result {
        Err(_) => {
            crate::shut_down(format!("panic while pressing {what}"));
            Err("panicked".into())
        }
        Ok(Err(e)) => Err(e),
        Ok(Ok(())) => {
            logln!("[PRESSED] {what}");
            Ok(())
        }
    }
}

/// The runtime's own method invoker: `(self, other, result, argc, method, args)`.
///
/// Taken from how the game calls `resolve_reroll_cost`:
///
/// ```text
///   call [rax+8]          ; fetch the method -> RValue* in rax
///   mov  [rsp+0x28], r15  ; args = null
///   mov  [rsp+0x20], rax  ; the method, BY POINTER
///   xor  r9d, r9d         ; argc = 0
///   lea  r8, [rbp-0x50]   ; result
///   mov  rdx, r12         ; other
///   mov  rcx, rdi         ; self
///   call 0x1aa47f0
/// ```
///
/// This is what `script_execute` was standing in for, and evidently not doing
/// for a bound method -- the cost came back as nothing every time. Using the
/// path the game itself uses removes the guess.
const METHOD_INVOKE_RVA: usize = 0x1aa47f0;

type MethodInvoke = unsafe extern "system" fn(
    *mut u8,            // self
    *mut u8,            // other
    *mut RValueRaw,     // result
    i32,                // argc
    *const RValueRaw,   // the method, by pointer
    *const RValueRaw,   // args
);

/// Invoke a method and hand back what it returned.
///
/// Used for `resolve_reroll_cost`, where the answer is the point. Same guards
/// as `press`, including passing the owning instance as `self`.
///
/// # Safety
/// Game thread only.
pub unsafe fn call_for_value(
    state: &State,
    base: usize,
    inst: usize,
    member: &str,
) -> Option<Value> {
    call_member(state, base, inst, member, true)
}

/// Invoke a method for its effect, discarding whatever it returns.
///
/// Used for the game's own UI teardown -- `hide_units_icons` and the like --
/// where the point is the side effect and "returned undefined" is the normal
/// answer, not something to report.
///
/// # Safety
/// Game thread only.
pub unsafe fn run_method(state: &State, base: usize, inst: usize, member: &str) -> bool {
    call_member(state, base, inst, member, false);
    state.syms.var_id(member).is_some()
}

unsafe fn call_member(
    state: &State,
    base: usize,
    inst: usize,
    member: &str,
    want_value: bool,
) -> Option<Value> {
    let id = state.syms.var_id(member)?;
    let rv = instance::get_var(inst, id)?;
    let Some(v) = crate::rvalue::decode(rv) else {
        crate::logln!("[call] {member}: the member did not decode as an RValue");
        return None;
    };
    if !is_method(base, state.text, inst, &v) {
        crate::logln!("[call] {member}: is_method says no; it is {v:?}");
        return None;
    }
    let addr = base + METHOD_INVOKE_RVA;
    if addr < state.text.0 || addr >= state.text.1 {
        return None;
    }
    let f: MethodInvoke = core::mem::transmute(addr);
    let mut out = RValueRaw { payload: 0, flags: 0, kind: 5 };
    let me = inst as *mut u8;
    // `rv` is the RValue* the variable getter returned -- the method is passed
    // by pointer, exactly as the game does it.
    f(me, me, &mut out, 0, rv as *const RValueRaw, core::ptr::null());
    if !want_value {
        return None;
    }
    match out.kind {
        0 => Some(Value::Real(f64::from_bits(out.payload))),
        7 => Some(Value::Int(out.payload as u32 as i32 as i64)),
        10 => Some(Value::Int(out.payload as i64)),
        13 => Some(Value::Bool(out.payload != 0)),
        k => {
            // Say what actually came back. "returned nothing" twice in a row
            // told me only that my reading was wrong, not how.
            crate::logln!(
                "[call] {member}: returned kind {k} payload {:#x} - not a number",
                out.payload
            );
            None
        }
    }
}

/// The button instance a card's `select_button` refers to.
///
/// It is an instance *reference* (ref type `0x04000001`), not a pointer, so it
/// has to be resolved through the instance table before it can be pressed.
pub fn select_button_of(state: &State, base: usize, card: usize) -> Option<usize> {
    let id = state.syms.var_id("select_button")?;
    let rv = unsafe { instance::get_var(card, id) }?;
    match crate::rvalue::decode(rv)? {
        Value::Ref { ref_type: 0x0400_0001, id } => instance::by_id(base, id),
        _ => None,
    }
}
