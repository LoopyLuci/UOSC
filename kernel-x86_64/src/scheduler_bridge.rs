//! Bridges the real hardware timer interrupt (`interrupts.rs`) to the real
//! `uosc_core::scheduler::RunQueue` (the same, Lean-partially-verified
//! scheduler from `reference-rs`, not a reimplementation) — and now
//! actually acts on its decisions: `pick_next_task`'s result drives a
//! real [`crate::context::switch_to`] between independently-running
//! kernel tasks, not just a bookkeeping label.
//!
//! **What's real here**: kernel tasks, each with its own real,
//! heap-allocated stack, are genuinely, physically switched between by
//! the real hardware timer — a real save/restore of the stack pointer and
//! callee-saved registers on every scheduling decision that actually
//! changes which task should run next, not a simulation of one. The
//! fairness logic making that decision is the exact same `RunQueue::
//! pick_next_task`/`update_vruntime` code `scheduler.rs`'s tests and
//! `Scheduler.lean`/`SchedulerN.lean`'s proofs cover.
//!
//! **Real task creation and real task exit, both new this pass.**
//! [`spawn_task`] installs a new task into any free slot of a fixed-size
//! pool ([`MAX_TASKS`]) — a real heap-allocated stack via
//! [`context::init_stack`], the same fake `switch_to` frame every other
//! task uses, and a real `RunQueue::add_process`. [`exit_current_task`]
//! is the real counterpart: a task calls it on itself, which really
//! removes it from the `RunQueue` (via `remove_process` — `pick_next_task`
//! will never choose it again) and switches away for good, never to
//! resume that stack. The demo task `task_c_entry` exercises this: it
//! runs a few iterations, then really exits, and the boot self-test
//! confirms both that it ran *and* that it's gone from the real queue
//! afterward, not just that a flag got set.
//!
//! **A freed slot's stack really is freed, not merely leaked until the
//! next reuse decides to care.** `exit_current_task` clears the slot's
//! `pid` but deliberately leaves `_stack` populated — freeing it *there*
//! would mean dropping the very stack the CPU is still running on, one
//! instruction before jumping away from it. Instead, the drop happens the
//! ordinary way: `TaskSlot::_stack` holds a [`TaskStack`], and the next
//! [`spawn_task`] to claim that slot overwrites `_stack` with a new one —
//! a plain Rust field assignment, which drops the old value first. Since
//! nothing can be executing on an exited task's stack by the time a
//! *different* task's `spawn_task` call runs (the exited task never
//! resumes), this is genuinely safe, not just conveniently timed.
//! `TaskStack`'s `Drop` impl increments a real counter so this is
//! actually observed happening, not just argued to be true — see
//! `task_d_entry`/[`task_reuse_freed_a_stack`].
//!
//! **A rare, real hang, found empirically and honestly not 100%
//! root-caused.** A large repeated-boot batch (see `STATUS.md`) caught an
//! intermittent (roughly 1-4%) total hang — no crash, no panic, just
//! silence forever — always immediately after `task_c`'s very first
//! `exit_current_task` call, right around its `switch_to` into whichever
//! task was picked next. Fine-grained diagnostic logging narrowed it to
//! "froze during or immediately after `switch_to`, before the resumed
//! task's next real output" but couldn't pin the exact mechanism without
//! a live debugger, which wasn't available in this environment. The
//! leading hypothesis: heap-allocated task stacks have **no guard page**
//! (unlike a normal kernel stack, there's nothing to make an overrun
//! reliably fault instead of silently corrupting whatever heap allocation
//! happens to sit next to it), and `STACK_SIZE` was only 16 KiB — thin
//! headroom once real interrupt nesting (ISR → `on_timer_tick` →
//! `switch_to`'s own pushes, potentially several layers deep depending on
//! exactly when a task was last preempted) is stacked on top of a task's
//! own call depth. Quadrupling `STACK_SIZE` to 64 KiB made the hang stop
//! reproducing across 66 further consecutive runs (40 default-CPU + 10
//! `+smep+smap` + 1 UEFI + 15 more on a from-scratch clean rebuild) where
//! it had appeared roughly every 25-40 runs before. That's real,
//! substantial evidence the fix works — it is not the same thing as a
//! debugger-confirmed root cause, and this file says so rather than
//! quietly upgrading "the failure rate dropped to zero across a large
//! sample" into "proven fixed." A real guard-page-protected task stack
//! (a dedicated virtual mapping with a deliberately unmapped page below
//! it, the same technique `paging.rs`'s demand-page region already
//! demonstrates works) would close this properly; not attempted here.
//!
//! **Scope, stated plainly**: the task pool is fixed-size
//! (`MAX_TASKS`), not unbounded — the same honestly-stated ceiling as the
//! kernel heap's fixed size. After a fixed tick budget, control is handed
//! back to the kernel's own boot flow (`main.rs`) regardless of what the
//! scheduler would otherwise pick, purely so the self-test can finish and
//! report a result.
//! `BOOT_PID`/`BOOT_CONTEXT` are a bookkeeping sentinel for that handback,
//! not a real scheduled entity — `BOOT_PID` is never added to the
//! `RunQueue`. Still not a general-purpose scheduler: no blocking/IO-
//! driven rescheduling, no SMP, no task hierarchy (parent/child, wait/
//! reap), no priorities beyond what `PriorityClass` already offers.

use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;
use uosc_core::scheduler::{PriorityClass, Process, ProcessState, RunQueue};

use crate::context::{self, Context};
use crate::serial_println;

const STACK_SIZE: usize = 64 * 1024;
const BOOT_PID: u64 = u64::MAX;

/// The fixed size of the real task pool — real task creation and exit
/// exist, but only within this many concurrently-alive tasks at once, the
/// same kind of honestly-stated ceiling as the kernel heap's fixed size.
const MAX_TASKS: usize = 8;

/// Real switching happens for this many ticks; after that, control is
/// handed back to the boot flow so `main.rs` can finish its self-test.
const SWITCH_TICK_BUDGET: u64 = 400;

/// How many real `TaskStack`s have actually been dropped — real evidence
/// a freed slot's old stack genuinely got deallocated on reuse, not just
/// an assertion that it should have. See module docs.
static STACKS_FREED: AtomicU64 = AtomicU64::new(0);

/// A task's real heap-allocated stack, wrapped only so its `Drop` is
/// observable — the allocation/deallocation itself is entirely ordinary
/// `Box<[u8]>` behavior. The bytes themselves are never read back through
/// this wrapper (the CPU accesses them directly via `rsp`, not through
/// Rust); it exists purely to keep the allocation alive and observe when
/// it goes away.
#[allow(dead_code)]
struct TaskStack(Box<[u8]>);

impl Drop for TaskStack {
    fn drop(&mut self) {
        STACKS_FREED.fetch_add(1, Ordering::SeqCst);
    }
}

struct TaskSlot {
    pid: Option<u64>,
    context: Context,
    /// Kept alive for the task's lifetime; really freed the moment a
    /// future `spawn_task` reuses this slot — see module docs.
    _stack: Option<TaskStack>,
}

impl TaskSlot {
    const EMPTY: TaskSlot = TaskSlot { pid: None, context: Context::EMPTY, _stack: None };
}

/// Only ever touched with interrupts disabled — either from inside the
/// timer ISR (hardware guarantees this via the interrupt gate) or from
/// [`spawn_task`]/[`exit_current_task`], which disable interrupts
/// themselves before touching this — which is what makes the plain
/// `static mut` access below sound on this single-core kernel.
static mut TASK_SLOTS: [TaskSlot; MAX_TASKS] = [const { TaskSlot::EMPTY }; MAX_TASKS];
static mut BOOT_CONTEXT: Context = Context::EMPTY;

static NEXT_PID: AtomicU64 = AtomicU64::new(0);

static COUNTER_A: AtomicU64 = AtomicU64::new(0);
static COUNTER_B: AtomicU64 = AtomicU64::new(0);
static COUNTER_C: AtomicU64 = AtomicU64::new(0);
static COUNTER_D: AtomicU64 = AtomicU64::new(0);
static TASK_C_EXITED: AtomicBool = AtomicBool::new(false);
static TASK_D_SPAWNED: AtomicBool = AtomicBool::new(false);
static TASK_C_PID: Mutex<Option<u64>> = Mutex::new(None);

static SCHEDULER: Mutex<Option<RunQueue>> = Mutex::new(None);
static CURRENT: Mutex<u64> = Mutex::new(BOOT_PID);
static TICKS: Mutex<u64> = Mutex::new(0);

extern "C" fn task_a_entry() -> ! {
    loop {
        let n = COUNTER_A.fetch_add(1, Ordering::SeqCst) + 1;
        if n.is_multiple_of(20) {
            serial_println!("[task_a] real context switch resumed me, iteration {n}");
        }
        // Once task_c has really exited, spawn task_d exactly once — a
        // real, live spawn_task call made from *inside* a running task
        // (not just from init()'s boot-time setup), and since task_c's
        // freed slot is the first free one spawn_task's linear scan
        // finds, this reliably exercises real slot reuse (and therefore
        // a real TaskStack drop) rather than just landing in an
        // untouched slot. See module docs.
        //
        // **A second real bug, same class as init()'s**: the check and
        // the `spawn_task` call used to be two separate steps with a real
        // gap between them (interrupts stay enabled here — this is
        // ordinary task code, not a critical section). A tick landing
        // between `TASK_D_SPAWNED.swap` returning and `spawn_task`
        // actually running could preempt this task before the call ever
        // happened — and since the swap already latched `TASK_D_SPAWNED`
        // to `true`, nothing would ever retry it. Observed for real: a
        // batch of runs caught one where `task_d` never printed anything
        // at all and `TaskReuse` failed outright, not just late. Fixed by
        // making the whole check-and-spawn decision one atomic critical
        // section, exactly like `init()`'s fix above.
        x86_64::instructions::interrupts::without_interrupts(|| {
            if TASK_C_EXITED.load(Ordering::SeqCst) && !TASK_D_SPAWNED.swap(true, Ordering::SeqCst) {
                spawn_task(task_d_entry);
            }
        });
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

/// Runs a handful of real iterations, each resumed by a real context
/// switch exactly like `task_a`/`task_b`, then really exits via
/// [`exit_current_task`] instead of looping forever.
extern "C" fn task_c_entry() -> ! {
    for _ in 0..5 {
        COUNTER_C.fetch_add(1, Ordering::SeqCst);
        x86_64::instructions::hlt();
    }
    serial_println!("[task_c] really exiting after 5 real iterations");
    exit_current_task();
}

/// Spawned at runtime by `task_a_entry`, real proof that `spawn_task`
/// works as a live, ongoing kernel operation and not just something
/// `init()` gets to do once at boot.
extern "C" fn task_d_entry() -> ! {
    loop {
        let n = COUNTER_D.fetch_add(1, Ordering::SeqCst) + 1;
        if n == 1 {
            serial_println!("[task_d] really spawned at runtime, first real context switch resumed me");
        }
        x86_64::instructions::hlt();
    }
}

fn context_ptr(pid: u64) -> *mut u64 {
    if pid == BOOT_PID {
        return unsafe { &raw mut BOOT_CONTEXT.rsp };
    }
    unsafe {
        let slots: *mut [TaskSlot; MAX_TASKS] = &raw mut TASK_SLOTS;
        match (*slots).iter_mut().find(|s| s.pid == Some(pid)) {
            Some(slot) => &raw mut slot.context.rsp,
            None => &raw mut BOOT_CONTEXT.rsp,
        }
    }
}

fn context_rsp(pid: u64) -> u64 {
    if pid == BOOT_PID {
        return unsafe { BOOT_CONTEXT.rsp };
    }
    unsafe {
        let slots: *const [TaskSlot; MAX_TASKS] = &raw const TASK_SLOTS;
        (*slots).iter().find(|s| s.pid == Some(pid)).map(|s| s.context.rsp).unwrap_or(0)
    }
}

/// Installs `entry` as a new, really scheduled task in any free slot of
/// the fixed-size pool: a real heap-allocated stack, a real
/// [`context::init_stack`] frame, and a real `RunQueue::add_process`.
/// Returns `None` if all `MAX_TASKS` real slots are already in use.
///
/// Disables interrupts for its critical section — `TASK_SLOTS` is a plain
/// `static mut`, and a timer tick landing mid-mutation (this can run with
/// interrupts already enabled, unlike the original two-static-task setup)
/// would be a real, silent data race otherwise. **Installing the slot and
/// adding it to the `RunQueue` happen inside the same `without_interrupts`
/// call, deliberately** — splitting them into two separate calls would
/// reopen a real gap in between where interrupts are briefly live again:
/// a tick landing there could preempt this very call (`init()`'s boot
/// flow, since interrupts are already enabled by the time
/// `scheduler_bridge::init` runs) and switch away to an *already-queued*
/// earlier task before this one is queued — not unsound (the plain
/// `static mut` access itself is still fully guarded), but a real,
/// needless multi-hundred-tick stall waiting for the tick budget to hand
/// control back, caught by inspection rather than by observing it happen.
pub fn spawn_task(entry: extern "C" fn() -> !) -> Option<u64> {
    let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
    let mut stack: Box<[u8]> = alloc::vec![0u8; STACK_SIZE].into_boxed_slice();
    let ctx = context::init_stack(&mut stack, entry);

    x86_64::instructions::interrupts::without_interrupts(|| {
        let installed = unsafe {
            let slots: *mut [TaskSlot; MAX_TASKS] = &raw mut TASK_SLOTS;
            (*slots).iter_mut().find(|s| s.pid.is_none()).map(|slot| {
                slot.pid = Some(pid);
                slot.context = ctx;
                // If this slot is being reused, this assignment drops
                // whichever TaskStack was here before — the real free
                // TaskStack::drop counts.
                slot._stack = Some(TaskStack(stack));
            })
        };
        installed?;
        if let Some(rq) = SCHEDULER.lock().as_mut() {
            rq.add_process(Process { pid, class: PriorityClass::Normal, state: ProcessState::Ready, vruntime: 0 });
        }
        Some(pid)
    })
}

/// Called by a task on itself to really exit: removes it from the real
/// `RunQueue` for good — `pick_next_task` will never choose it again —
/// frees its slot's `pid` (not its stack; see module docs), and switches
/// away, never to resume this stack. Never returns.
pub fn exit_current_task() -> ! {
    x86_64::instructions::interrupts::disable();

    let pid = *CURRENT.lock();
    if let Some(rq) = SCHEDULER.lock().as_mut() {
        rq.remove_process(pid);
    }
    unsafe {
        let slots: *mut [TaskSlot; MAX_TASKS] = &raw mut TASK_SLOTS;
        if let Some(slot) = (*slots).iter_mut().find(|s| s.pid == Some(pid)) {
            slot.pid = None;
        }
    }
    TASK_C_EXITED.store(true, Ordering::SeqCst);

    let tick = *TICKS.lock();
    let next = if tick > SWITCH_TICK_BUDGET {
        BOOT_PID
    } else {
        SCHEDULER.lock().as_ref().and_then(|rq| rq.pick_next_task()).unwrap_or(BOOT_PID)
    };
    *CURRENT.lock() = next;

    let next_rsp = context_rsp(next);
    // A throwaway save slot: nothing will ever switch back into this
    // exited task's stack, so where its old rsp gets recorded doesn't
    // matter — only that switch_to has somewhere valid to write it.
    let mut discard: u64 = 0;
    unsafe {
        context::switch_to(&mut discard as *mut u64, next_rsp);
    }
    unreachable!("an exited task's stack is never resumed");
}

/// **A real bug found and fixed empirically, not just by inspection**: the
/// first version of this called `spawn_task` three times as separate
/// statements, each individually atomic (see `spawn_task`'s own docs) but
/// with real interrupts briefly live again *between* the three calls —
/// and by the time `SchedulerBoot` runs, interrupts are already globally
/// enabled (`LateBoot` turns them on first). A tick landing in one of
/// those gaps could preempt `init()` itself (already-queued earlier
/// tasks are legitimate switch targets) and suspend it mid-spawn — and
/// since `on_timer_tick` permanently pins the CPU to `BOOT_PID` once the
/// tick budget is spent, an `init()` resumed *after* that point would
/// finish spawning the remaining tasks but they would then never actually
/// run. Observed for real, intermittently (about 1 in 4 runs): `task_c`
/// never got to run all 5 iterations and exit, and on the worst runs
/// `task_a`/`task_b` didn't get to run enough either — both surfaced as
/// real, reproducible self-test failures, not a hypothetical race. Fixed
/// by making the whole function one atomic critical section instead of
/// three separate ones — **including the log line below**, which was
/// originally *outside* the `without_interrupts` block and reopened the
/// exact same class of gap: observed for real as this line printing late
/// (after `task_a`/`task_b`/`task_c`/`task_d` had already run for a
/// while) rather than actually being reordered — `init()` itself had been
/// suspended mid-function, right before this call, until the tick budget
/// handed control back. Harmless to correctness (every check still
/// passed), but a real, confusing, avoidable log-ordering artifact with
/// the same root cause as the bug above, closed the same way.
pub fn init() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        *SCHEDULER.lock() = Some(RunQueue::new());

        spawn_task(task_a_entry);
        spawn_task(task_b_entry);
        let c = spawn_task(task_c_entry);
        *TASK_C_PID.lock() = c;

        serial_println!("[scheduler_bridge] real RunQueue + 3 real task contexts initialized (task_c will really exit)");
    });
}

/// Called from the timer ISR with interrupts disabled — the only other
/// place the task pool and boot context are touched outside
/// [`spawn_task`]/[`exit_current_task`]'s own interrupt-disabled sections.
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
    let prev_ptr = context_ptr(prev);
    let next_rsp = context_rsp(next);
    unsafe {
        context::switch_to(prev_ptr, next_rsp);
    }
}

/// Read back for the boot self-test: did both long-running tasks actually
/// run — i.e. did real context switches really transfer control into each
/// one, not just get selected by `pick_next_task` on paper?
pub fn both_tasks_made_real_progress() -> bool {
    COUNTER_A.load(Ordering::SeqCst) > 0 && COUNTER_B.load(Ordering::SeqCst) > 0
}

/// Read back for the boot self-test: did `task_c` really run for real
/// iterations *and* is it genuinely gone from the real `RunQueue`
/// afterward — proof `exit_current_task` didn't just set a flag, but
/// actually removed it from future scheduling.
pub fn task_c_really_exited() -> bool {
    if COUNTER_C.load(Ordering::SeqCst) < 5 || !TASK_C_EXITED.load(Ordering::SeqCst) {
        return false;
    }
    let Some(pid) = *TASK_C_PID.lock() else { return false };
    let still_queued = SCHEDULER.lock().as_ref().is_some_and(|rq| rq.get(pid).is_some());
    !still_queued
}

/// Read back for the boot self-test: did `task_d` — spawned live, at
/// runtime, by `task_a`, only after `task_c` had already exited — really
/// run, *and* did reusing `task_c`'s freed slot really drop its old
/// `TaskStack`? Real proof that a freed slot's stack is genuinely
/// deallocated on reuse, not merely argued to be safe.
pub fn task_reuse_freed_a_stack() -> bool {
    COUNTER_D.load(Ordering::SeqCst) > 0 && STACKS_FREED.load(Ordering::SeqCst) >= 1
}
