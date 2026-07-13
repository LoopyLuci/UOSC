//! Bridges the real hardware timer interrupt (`interrupts.rs`) to the real
//! `uosc_core::scheduler::RunQueue` (the same, Lean-partially-verified
//! scheduler from `reference-rs`, not a reimplementation) — and now
//! actually acts on its decisions: `pick_next_task`'s result drives a
//! real [`crate::context::switch_to`] between two independently-running
//! kernel tasks, not just a bookkeeping label.
//!
//! **What's real here**: two dedicated kernel tasks (`task_a`/`task_b`),
//! each with its own real stack, are genuinely, physically switched
//! between by the real hardware timer — a real save/restore of the stack
//! pointer and callee-saved registers on every scheduling decision that
//! actually changes which task should run next, not a simulation of one.
//! The fairness logic making that decision is the exact same
//! `RunQueue::pick_next_task`/`update_vruntime` code `scheduler.rs`'s
//! tests and `Scheduler.lean`/`SchedulerN.lean`'s proofs cover.
//!
//! **Scope, stated plainly**: after a fixed tick budget, control is
//! handed back to the kernel's own boot flow (`main.rs`) regardless of
//! what the scheduler would otherwise pick, purely so the self-test can
//! finish and report a result. `BOOT_PID` is a bookkeeping sentinel for
//! that handback, not a real scheduled entity — it is never added to the
//! `RunQueue`. This still isn't a general-purpose scheduler: two static,
//! hardcoded tasks, no task creation/exit, no blocking/IO-driven
//! rescheduling, no SMP.

use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use uosc_core::scheduler::{PriorityClass, Process, ProcessState, RunQueue};

use crate::context::{self, Context};
use crate::serial_println;

const STACK_SIZE: usize = 16 * 1024;
const TASK_A_PID: u64 = 0;
const TASK_B_PID: u64 = 1;
const BOOT_PID: u64 = u64::MAX;

/// Real switching happens for this many ticks; after that, control is
/// handed back to the boot flow so `main.rs` can finish its self-test.
const SWITCH_TICK_BUDGET: u64 = 400;

static mut TASK_STACKS: [[u8; STACK_SIZE]; 2] = [[0; STACK_SIZE]; 2];
static mut CONTEXTS: [Context; 3] = [Context::EMPTY, Context::EMPTY, Context::EMPTY];

static COUNTER_A: AtomicU64 = AtomicU64::new(0);
static COUNTER_B: AtomicU64 = AtomicU64::new(0);

static SCHEDULER: Mutex<Option<RunQueue>> = Mutex::new(None);
static CURRENT: Mutex<u64> = Mutex::new(BOOT_PID);
static TICKS: Mutex<u64> = Mutex::new(0);

fn context_index(pid: u64) -> usize {
    match pid {
        TASK_A_PID => 0,
        TASK_B_PID => 1,
        _ => 2,
    }
}

extern "C" fn task_a_entry() -> ! {
    loop {
        let n = COUNTER_A.fetch_add(1, Ordering::SeqCst) + 1;
        if n.is_multiple_of(20) {
            serial_println!("[task_a] real context switch resumed me, iteration {n}");
        }
        x86_64::instructions::hlt();
    }
}

extern "C" fn task_b_entry() -> ! {
    loop {
        let n = COUNTER_B.fetch_add(1, Ordering::SeqCst) + 1;
        if n.is_multiple_of(20) {
            serial_println!("[task_b] real context switch resumed me, iteration {n}");
        }
        x86_64::instructions::hlt();
    }
}

pub fn init() {
    let mut rq = RunQueue::new();
    rq.add_process(Process { pid: TASK_A_PID, class: PriorityClass::Normal, state: ProcessState::Ready, vruntime: 0 });
    rq.add_process(Process { pid: TASK_B_PID, class: PriorityClass::Normal, state: ProcessState::Ready, vruntime: 0 });
    *SCHEDULER.lock() = Some(rq);

    unsafe {
        let stacks: *mut [[u8; STACK_SIZE]; 2] = &raw mut TASK_STACKS;
        let a_ctx = context::init_stack(&mut (*stacks)[0], task_a_entry);
        let b_ctx = context::init_stack(&mut (*stacks)[1], task_b_entry);
        let contexts: *mut [Context; 3] = &raw mut CONTEXTS;
        (*contexts)[0] = a_ctx;
        (*contexts)[1] = b_ctx;
    }

    serial_println!("[scheduler_bridge] real RunQueue + 2 real task contexts initialized");
}

/// Called from the timer ISR with interrupts disabled — the only place
/// `CONTEXTS`/`TASK_STACKS` are ever touched, which is what makes the
/// plain `static mut` access below sound on this single-core kernel:
/// nothing else can run concurrently with this function.
pub fn on_timer_tick() {
    let mut ticks = TICKS.lock();
    *ticks += 1;
    let tick = *ticks;
    drop(ticks);

    let mut guard = SCHEDULER.lock();
    let Some(rq) = guard.as_mut() else { return };

    let mut current = CURRENT.lock();
    let prev = *current;

    let next = if tick > SWITCH_TICK_BUDGET {
        BOOT_PID
    } else {
        rq.pick_next_task().unwrap_or(BOOT_PID)
    };

    if prev != BOOT_PID {
        rq.update_vruntime(prev, 1);
    }

    if prev == next {
        return;
    }
    *current = next;
    drop(current);
    drop(guard);

    // Every lock above is dropped before this point — required, since
    // switch_to may not return here until this exact context is resumed,
    // which could be many ticks (and many other lock acquisitions) later.
    unsafe {
        let contexts: *mut [Context; 3] = &raw mut CONTEXTS;
        let prev_ptr = (&raw mut (*contexts)[context_index(prev)]) as *mut u64;
        let next_rsp = (*contexts)[context_index(next)].rsp;
        context::switch_to(prev_ptr, next_rsp);
    }
}

/// Read back for the boot self-test: did both tasks actually run — i.e.
/// did real context switches really transfer control into each one, not
/// just get selected by `pick_next_task` on paper?
pub fn both_tasks_made_real_progress() -> bool {
    COUNTER_A.load(Ordering::SeqCst) > 0 && COUNTER_B.load(Ordering::SeqCst) > 0
}
