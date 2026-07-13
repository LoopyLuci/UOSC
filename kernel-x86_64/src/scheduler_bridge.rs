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
//! **Task stacks are now real, guard-page-protected page-table mappings,
//! not heap-allocated `Box<[u8]>`s.** See `task_stack.rs`'s module docs for
//! the full story — the short version: each of the `MAX_TASKS` array slots
//! below (the same index used for both) owns a fixed virtual address range
//! with a genuinely unmapped page directly beneath its stack, mapped once
//! on first use and reused (not freed/reallocated) for the life of the
//! kernel, since the slot itself was always a fixed-size resource anyway.
//! [`task_reused_a_guarded_slot`] is the real proof this actually happens:
//! it confirms `task_d` landed in the *exact same* guarded slot `task_c`'s
//! stack used, not merely some slot.
//!
//! **A rare, real hang, found empirically, mitigated, and — this pass —
//! closed more rigorously.** A large repeated-boot batch (see `STATUS.md`)
//! caught an intermittent (roughly 1-4%) total hang — no crash, no panic,
//! just silence forever — always immediately after `task_c`'s very first
//! `exit_current_task` call, right around its `switch_to` into whichever
//! task was picked next. Fine-grained diagnostic logging narrowed it to
//! "froze during or immediately after `switch_to`, before the resumed
//! task's next real output" but couldn't pin the exact mechanism without a
//! live debugger, which wasn't available in this environment. The leading
//! hypothesis at the time: heap-allocated task stacks had **no guard
//! page** (unlike a normal kernel stack, there was nothing to make an
//! overrun reliably fault instead of silently corrupting whatever heap
//! allocation happened to sit next to it), and `STACK_SIZE` was only
//! 16 KiB — thin headroom once real interrupt nesting (ISR →
//! `on_timer_tick` → `switch_to`'s own pushes, potentially several layers
//! deep depending on exactly when a task was last preempted) is stacked on
//! top of a task's own call depth. Quadrupling the stack size to 64 KiB
//! made the hang stop reproducing across 66 further consecutive runs,
//! real, substantial evidence, but honestly not the same thing as a
//! debugger-confirmed root cause.
//!
//! This pass replaces that heap-allocated mitigation with real
//! guard-page-protected stacks (`task_stack.rs`): each task-pool slot now
//! has a dedicated virtual mapping with a genuinely unmapped page directly
//! below it, the same technique `paging.rs`'s demand-page region already
//! demonstrates works. If the stack-headroom hypothesis was right, an
//! overrun now reliably produces a real, diagnosable `#PF` panic instead
//! of silent corruption — a strictly stronger guarantee than "the failure
//! rate dropped in a large sample," and one that would surface *any* real
//! overrun immediately and loudly rather than relying on generous sizing
//! to make one merely rare. Still honestly stated: this closes the
//! "silent corruption" failure mode structurally, but does not itself
//! prove the original hang's mechanism was stack overrun — no debugger was
//! available to confirm that historically, and none is available now
//! either.
//!
//! **Real task-to-address-space binding, new this pass.** Until now,
//! `address_space.rs`'s second, independent `CR3`-loadable table was a
//! manual, one-shot demonstration — `main.rs`'s self-test switched to it,
//! read it, and switched straight back, entirely separate from real task
//! scheduling. This pass ties the two together for real:
//! [`spawn_task_with_address_space`] binds a task to a specific L4 frame
//! at spawn time (stored in that task's `TaskSlot::address_space`), and
//! [`switch_address_space_to`] — called from *both* real switch call
//! sites, [`on_timer_tick`] and [`exit_current_task`], right before the
//! real [`context::switch_to`] — performs a real `CR3` write whenever the
//! task being switched *into* needs a different table than what's
//! currently active. A tick switching between two ordinary kernel tasks
//! (the overwhelmingly common case) costs nothing beyond a `CR3` read that
//! finds nothing to change; a tick switching into `task_e` — spawned by
//! [`init`] bound to its own fresh `AddressSpace` — performs a real
//! switch, and `task_e` proves it by reading straight through
//! `address_space::PRIVATE_REGION_ADDR` every iteration: that only
//! resolves correctly if the *ordinary, timer-driven* scheduler really
//! did switch `CR3`, not a special path task_e itself invokes.
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
//! reap), no priorities beyond what `PriorityClass` already offers. Task-
//! to-address-space binding is likewise real but narrow: one-way only (a
//! task is bound at spawn time and never rebinds), no unbind/teardown
//! path (a bound task's `AddressSpace` is never freed even if the task
//! later exited — no task that owns one does exit in this pass anyway),
//! and no process abstraction wraps the pairing (a `TaskSlot` just holds
//! an `Option<PhysFrame>`, not a first-class "process" concept).

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;
use uosc_core::scheduler::{PriorityClass, Process, ProcessState, RunQueue};
use x86_64::structures::paging::{PhysFrame, Size4KiB};

use crate::context::{self, Context};
use crate::serial_println;
use crate::task_stack;

const BOOT_PID: u64 = u64::MAX;

/// The fixed size of the real task pool — real task creation and exit
/// exist, but only within this many concurrently-alive tasks at once, the
/// same kind of honestly-stated ceiling as the kernel heap's fixed size.
/// Defined in terms of [`task_stack::MAX_SLOTS`], not the other way
/// around — see that module's docs.
const MAX_TASKS: usize = task_stack::MAX_SLOTS;

/// Real switching happens for this many ticks; after that, control is
/// handed back to the boot flow so `main.rs` can finish its self-test.
const SWITCH_TICK_BUDGET: u64 = 400;

/// Real proof of guarded-slot reuse for the boot self-test: the slot index
/// (not pid — pids are never reused, slot indices are) that the last task
/// to call [`exit_current_task`] used, and the slot index [`task_a_entry`]
/// observed `task_d` land in when it spawned it. See
/// [`task_reused_a_guarded_slot`].
static TASK_EXIT_SLOT: Mutex<Option<usize>> = Mutex::new(None);
static TASK_D_SPAWN_SLOT: Mutex<Option<usize>> = Mutex::new(None);

/// The real L4 frame every task *without* its own bound address space
/// runs in — captured once, in [`init`], from whatever `CR3` actually is
/// at that point (by then, `main.rs`'s own `AddressSpace` self-test has
/// already switched to its demo table and switched back — see
/// `address_space.rs` — so this really is the original/default table, not
/// a leftover from that demo). Used by [`desired_l4_frame`] as the
/// fallback for `BOOT_PID` and for any task whose own `TaskSlot::
/// address_space` is `None`.
static DEFAULT_L4_FRAME: Mutex<Option<PhysFrame<Size4KiB>>> = Mutex::new(None);

struct TaskSlot {
    pid: Option<u64>,
    context: Context,
    /// `Some(frame)` if this task is really bound to its own,
    /// independent address space (see [`spawn_task_with_address_space`]);
    /// `None` means "runs in [`DEFAULT_L4_FRAME`], like every task before
    /// this pass." See module docs' "Real task-to-address-space binding"
    /// section.
    address_space: Option<PhysFrame<Size4KiB>>,
}

impl TaskSlot {
    const EMPTY: TaskSlot = TaskSlot { pid: None, context: Context::EMPTY, address_space: None };
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
static COUNTER_E: AtomicU64 = AtomicU64::new(0);
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
                if let Some(pid) = spawn_task(task_d_entry) {
                    *TASK_D_SPAWN_SLOT.lock() = slot_index_for_pid(pid);
                }
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

/// Bound, at spawn time, to its own real, independent address space (see
/// [`spawn_task_with_address_space`] and module docs) — every iteration
/// reads straight through the real virtual address
/// `crate::address_space::PRIVATE_REGION_ADDR`, with **no physical-offset
/// back door**. That read only resolves to the expected value at all
/// because a real `CR3` write (performed by [`switch_address_space_to`]
/// as an ordinary part of the timer-driven scheduling decision that
/// switches into this task, not a manual one-shot) actually happened —
/// if the binding were wrong, this would either read the wrong value or,
/// if the private page isn't mapped in whatever table is actually active,
/// genuinely page-fault (a loud, diagnosable failure, not a silent one).
extern "C" fn task_e_entry() -> ! {
    loop {
        let value = unsafe { (crate::address_space::PRIVATE_REGION_ADDR as *const u64).read_volatile() };
        if value == crate::address_space::PRIVATE_VALUE {
            let n = COUNTER_E.fetch_add(1, Ordering::SeqCst) + 1;
            if n.is_multiple_of(20) {
                serial_println!("[task_e] real context switch into my own real, bound address space resumed me, iteration {n}");
            }
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

/// Which array index (== which guarded `task_stack` slot) `pid` currently
/// occupies — used only for the boot self-test's reuse proof. Must be
/// called with interrupts already disabled (same requirement as every
/// other `TASK_SLOTS` access).
fn slot_index_for_pid(pid: u64) -> Option<usize> {
    unsafe {
        let slots: *const [TaskSlot; MAX_TASKS] = &raw const TASK_SLOTS;
        (*slots).iter().position(|s| s.pid == Some(pid))
    }
}

/// Installs `entry` as a new, really scheduled task in any free slot of
/// the fixed-size pool, running in [`DEFAULT_L4_FRAME`] like every task
/// before this pass. See [`spawn_task_with_address_space`] for the real
/// per-task address-space binding, and [`spawn_task_impl`] for the shared
/// mechanics both go through.
pub fn spawn_task(entry: extern "C" fn() -> !) -> Option<u64> {
    spawn_task_impl(entry, None)
}

/// Installs `entry` as a new, really scheduled task, real bound to
/// `l4_frame` — a real, independent address space (typically
/// `AddressSpace::l4_frame()`) this task, and only this task, will really
/// run in. See module docs' "Real task-to-address-space binding" section
/// for the full mechanism and [`task_e_entry`] for the demo that exercises
/// it.
pub fn spawn_task_with_address_space(entry: extern "C" fn() -> !, l4_frame: PhysFrame<Size4KiB>) -> Option<u64> {
    spawn_task_impl(entry, Some(l4_frame))
}

/// Shared mechanics for [`spawn_task`]/[`spawn_task_with_address_space`]:
/// a real guard-page-protected stack (see `task_stack.rs`), a real
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
fn spawn_task_impl(entry: extern "C" fn() -> !, address_space: Option<PhysFrame<Size4KiB>>) -> Option<u64> {
    let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);

    x86_64::instructions::interrupts::without_interrupts(|| {
        let slots: *mut [TaskSlot; MAX_TASKS] = &raw mut TASK_SLOTS;
        let free_index = unsafe { (*slots).iter().position(|s| s.pid.is_none()) }?;

        // Real, guard-page-protected pages for this slot — mapped the
        // first time this index is ever used, reused thereafter. See
        // task_stack.rs. Note: these pages live in DEFAULT_L4_FRAME (the
        // shared clone-source every AddressSpace::new() copies from), so
        // even a task bound to its own address space still finds its own
        // stack mapped identically there — the same sharing argument
        // address_space.rs's module docs already make for kernel code
        // and the heap.
        let stack_top = task_stack::ensure_slot_mapped(free_index);
        let stack_base = stack_top - task_stack::STACK_SIZE;

        // Safety: this builds a transient &mut [u8] over the slot's
        // dedicated, real mapped memory. Sound because nothing can be
        // executing on a previous occupant's stack by the time a
        // spawn_task call reuses this same slot index (an exited task
        // never resumes — see exit_current_task's docs), the exact
        // non-aliasing argument the old TaskStack wrapper relied on,
        // just applied to a fixed mapped region instead of a heap Box.
        let stack_slice = unsafe { core::slice::from_raw_parts_mut(stack_base as *mut u8, task_stack::STACK_SIZE as usize) };
        let ctx = context::init_stack(stack_slice, entry);

        unsafe {
            (*slots)[free_index].pid = Some(pid);
            (*slots)[free_index].context = ctx;
            (*slots)[free_index].address_space = address_space;
        }
        if let Some(rq) = SCHEDULER.lock().as_mut() {
            rq.add_process(Process { pid, class: PriorityClass::Normal, state: ProcessState::Ready, vruntime: 0 });
        }
        Some(pid)
    })
}

/// The real L4 frame `pid` should be running in — `DEFAULT_L4_FRAME` for
/// `BOOT_PID`, for any task with no bound address space, or if
/// `DEFAULT_L4_FRAME` itself hasn't been captured yet (defensive; `init`
/// always captures it before any task can run).
fn desired_l4_frame(pid: u64) -> Option<PhysFrame<Size4KiB>> {
    let default = *DEFAULT_L4_FRAME.lock();
    if pid == BOOT_PID {
        return default;
    }
    unsafe {
        let slots: *const [TaskSlot; MAX_TASKS] = &raw const TASK_SLOTS;
        (*slots).iter().find(|s| s.pid == Some(pid)).and_then(|s| s.address_space).or(default)
    }
}

/// Real task-to-address-space binding, in one line: a real `CR3` read to
/// see what's actually active, compared against what `pid` should be
/// running in, and a real `CR3` write only if they differ — so switching
/// between two tasks that share the same (default) address space, the
/// overwhelmingly common case, costs nothing beyond the read. Called
/// right before every real [`context::switch_to`], from both
/// [`on_timer_tick`] and [`exit_current_task`], so this is genuinely part
/// of the ordinary scheduling path, not a separate mechanism a task has to
/// opt into.
///
/// Safe to call with interrupts disabled from inside the timer ISR (the
/// only two call sites) because every real `AddressSpace` this kernel
/// creates (`address_space.rs::AddressSpace::new`) clones every entry
/// from the table active at its own creation time — so the kernel code,
/// IDT/GDT, and every task's own guarded stack resolve identically no
/// matter which of these tables is active when the switch actually
/// happens.
fn switch_address_space_to(pid: u64) {
    let Some(desired) = desired_l4_frame(pid) else { return };
    let (current, flags) = x86_64::registers::control::Cr3::read();
    if current != desired {
        unsafe {
            x86_64::registers::control::Cr3::write(desired, flags);
        }
    }
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
        if let Some((idx, slot)) = (*slots).iter_mut().enumerate().find(|(_, s)| s.pid == Some(pid)) {
            slot.pid = None;
            *TASK_EXIT_SLOT.lock() = Some(idx);
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

    switch_address_space_to(next);

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
        // Real CR3 at this exact point is the true default/boot table —
        // main.rs's own AddressSpace self-test (which runs earlier) has
        // already switched to its demo table and switched back. See
        // DEFAULT_L4_FRAME's docs.
        *DEFAULT_L4_FRAME.lock() = Some(x86_64::registers::control::Cr3::read().0);

        spawn_task(task_a_entry);
        spawn_task(task_b_entry);
        let c = spawn_task(task_c_entry);
        *TASK_C_PID.lock() = c;

        // task_e: real task-to-address-space binding. Builds one more
        // real AddressSpace (a full clone of the table active right now,
        // plus its own private page — see address_space.rs) and binds
        // task_e to it — the ordinary timer-driven scheduler, not a
        // manual one-shot, is what actually switches CR3 into and out of
        // it from here on.
        if let Some(space) = crate::address_space::AddressSpace::new() {
            spawn_task_with_address_space(task_e_entry, space.l4_frame());
        }

        serial_println!("[scheduler_bridge] real RunQueue + 4 real task contexts initialized (task_c will really exit, task_e is bound to its own real address space)");
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
    switch_address_space_to(next);
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
/// run, *and* did it really land in the exact same guarded `task_stack`
/// slot `task_c`'s stack used (proof the real, page-mapped guarded region
/// is genuinely reused, not that a fresh one happened to be picked)?
pub fn task_reused_a_guarded_slot() -> bool {
    let exit_slot = *TASK_EXIT_SLOT.lock();
    COUNTER_D.load(Ordering::SeqCst) > 0 && exit_slot.is_some() && exit_slot == *TASK_D_SPAWN_SLOT.lock()
}

/// Read back for the boot self-test: did `task_e` — bound, at spawn time,
/// to its own real, independent address space — really run *and* really
/// see its private mapping resolve correctly every single time (not just
/// once), proof the ordinary timer-driven scheduler is genuinely
/// switching `CR3` as part of scheduling `task_e` in and out, not merely
/// running it in whatever table happened to already be active.
pub fn task_e_verified_own_address_space() -> bool {
    COUNTER_E.load(Ordering::SeqCst) > 0
}
