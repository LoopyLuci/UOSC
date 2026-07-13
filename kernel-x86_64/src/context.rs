//! Real x86-64 context switching between independently-running kernel
//! tasks: a genuine save/restore of a task's stack pointer and
//! callee-saved registers, not a simulation of a scheduling decision.
//!
//! This is what `scheduler_bridge.rs` was missing before: `RunQueue::
//! pick_next_task` already decided *which* task should run next, but
//! nothing acted on that decision by physically switching the CPU to a
//! different instruction stream. This module is what performs that
//! switch, for real, on real (emulated) hardware.
//!
//! **How it's safe to call from inside the timer interrupt handler**: a
//! task being suspended is not resumed later via `iretq` — it's resumed
//! via a second `ret` out of [`switch_to`], landing right after the
//! `call switch_to` that suspended it. That call site is itself inside
//! `on_timer_tick`, inside the timer ISR, running on that task's own
//! stack, with its original hardware interrupt frame still intact
//! underneath. Unwinding back out through the ISR's compiler-generated
//! epilogue then performs the real `iretq`, restoring that task's flags
//! and resuming it exactly where it left off. A task's *first*
//! resumption instead lands in [`task_trampoline`], since there is no
//! earlier `call switch_to` on its stack yet to return into.

use core::arch::naked_asm;

#[repr(C)]
pub struct Context {
    pub rsp: u64,
}

impl Context {
    pub const EMPTY: Context = Context { rsp: 0 };
}

/// Saves the current stack pointer and callee-saved registers into
/// `*current_rsp`, then loads `next_rsp` and returns into whatever that
/// stack's top says to return into.
///
/// **May not return to its caller for an arbitrarily long time** — not
/// until something later switches back to the context just suspended.
/// Every lock must be dropped before calling this: holding one across the
/// call leaves it locked for as long as this context stays suspended, and
/// the very next task scheduled in is likely to want the same lock.
#[unsafe(naked)]
pub unsafe extern "C" fn switch_to(current_rsp: *mut u64, next_rsp: u64) {
    naked_asm!(
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov [rdi], rsp",
        "mov rsp, rsi",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "ret",
    )
}

/// Where a freshly-initialized task's stack `ret`s into the first time
/// it's switched to. [`init_stack`] stashes the real entry point in r15
/// (the last register `switch_to`'s epilogue pops before `ret`, so it
/// lands in the real r15 register here). Interrupts are still disabled
/// at this point — this task has never gone through a real `iretq`, so
/// nothing has restored flags for it yet — hence the explicit `sti`
/// before jumping into real task code.
#[unsafe(naked)]
unsafe extern "C" fn task_trampoline() {
    naked_asm!("sti", "jmp r15")
}

/// Builds the initial stack layout for a task that has never run: a fake
/// [`switch_to`] epilogue frame whose r15 slot holds `entry` and whose
/// return-address slot points at [`task_trampoline`], so the first real
/// switch into this context jumps straight into `entry` with interrupts
/// freshly enabled.
pub fn init_stack(stack: &mut [u8], entry: extern "C" fn() -> !) -> Context {
    let base = stack.as_mut_ptr() as u64;
    let top16 = (base + stack.len() as u64) & !0xF;
    // switch_to's final `ret` must land in task_trampoline with the same
    // stack alignment a real `call` would have produced (rsp % 16 == 8 at
    // function entry) — offsetting the reference point by 8 here achieves
    // that once the 7 slots below are popped/ret'd through.
    let stack_top = top16 - 8;
    let mut sp = stack_top;
    unsafe {
        sp -= 8;
        (sp as *mut u64).write(task_trampoline as unsafe extern "C" fn() as usize as u64); // return address
        sp -= 8;
        (sp as *mut u64).write(0); // rbp
        sp -= 8;
        (sp as *mut u64).write(0); // rbx
        sp -= 8;
        (sp as *mut u64).write(0); // r12
        sp -= 8;
        (sp as *mut u64).write(0); // r13
        sp -= 8;
        (sp as *mut u64).write(0); // r14
        sp -= 8;
        (sp as *mut u64).write(entry as usize as u64); // r15 — the real entry point
    }
    Context { rsp: sp }
}
