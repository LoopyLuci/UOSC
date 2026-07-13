//! Bridges the real hardware timer interrupt (`interrupts.rs`) to the real
//! `uosc_core::scheduler::RunQueue` (the same, Lean-partially-verified
//! scheduler from `reference-rs`, not a reimplementation).
//!
//! **Scope, stated plainly**: this drives real scheduling *decisions* —
//! every PIT tick, the real `RunQueue::pick_next_task` runs against real
//! demo tasks and the result is observable on the serial console. It does
//! **not** perform a real context switch (saving/restoring a second
//! independent register/stack state and actually jumping between two
//! running instruction streams) — that's a substantial, separate,
//! assembly-heavy piece of work (per-task kernel stacks, a `switch_to`
//! routine, careful interrupt-safety around the switch) that risks
//! introducing a genuinely dangerous bug (a bad stack swap triple-faults
//! the machine) if rushed. What's proved here: the real scheduler's
//! fairness logic — the same code covered by `scheduler.rs`'s tests and
//! `Scheduler.lean`/`SchedulerN.lean`'s proofs — actually runs, correctly,
//! driven by a real hardware interrupt, not a test harness.

use spin::Mutex;
use uosc_core::scheduler::{PriorityClass, Process, ProcessState, RunQueue};

use crate::serial_println;

static SCHEDULER: Mutex<Option<RunQueue>> = Mutex::new(None);
static TICKS: Mutex<u64> = Mutex::new(0);

const DEMO_TASK_COUNT: u64 = 3;
const REPORT_EVERY_N_TICKS: u64 = 50;

pub fn init() {
    let mut rq = RunQueue::new();
    for pid in 0..DEMO_TASK_COUNT {
        rq.add_process(Process { pid, class: PriorityClass::Normal, state: ProcessState::Ready, vruntime: 0 });
    }
    *SCHEDULER.lock() = Some(rq);
    serial_println!(
        "[scheduler_bridge] real RunQueue initialized with {} demo Normal-class tasks",
        DEMO_TASK_COUNT
    );
}

pub fn on_timer_tick() {
    let mut ticks = TICKS.lock();
    *ticks += 1;
    let tick = *ticks;
    drop(ticks);

    let mut guard = SCHEDULER.lock();
    let Some(rq) = guard.as_mut() else { return };

    let Some(picked) = rq.pick_next_task() else { return };
    rq.update_vruntime(picked, 1);

    if tick.is_multiple_of(REPORT_EVERY_N_TICKS) {
        serial_println!("[scheduler_bridge] tick {tick}: real RunQueue picked pid {picked}");
    }
}

/// Read back the current state for the boot self-test report.
pub fn ran_every_demo_task_at_least_once() -> bool {
    let mut guard = SCHEDULER.lock();
    let Some(rq) = guard.as_mut() else { return false };
    (0..DEMO_TASK_COUNT).all(|pid| rq.get(pid).is_some_and(|p| p.vruntime > 0))
}
