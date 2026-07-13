//! Process scheduling — real port of `kernel/scheduler.ti`.
//!
//! The Titan source's `Process` struct carries a `deadline` field and the
//! file's own header comment says "EDF ... for real-time + CFS ... for
//! normal tasks," but `RunQueue::pick_next_task` (`kernel/scheduler.ti:229-237`)
//! only ever does the CFS half — it reads the red-black tree's minimum
//! `vruntime` unconditionally. A `RealTime` task's deadline is stored and
//! never consulted, so nothing about the scheduler is actually EDF. This
//! port implements the hybrid for real: any ready `RealTime` task always
//! preempts the `Normal` class, chosen by earliest deadline; among
//! `Normal` tasks, the smallest-`vruntime` task runs next, exactly as
//! specified.
//!
//! `RedBlackTree<K, V>` in the Titan source is an unimplemented stub
//! (`fn insert`, `fn remove`, `fn min` all `/* ... */`). This port uses
//! `BTreeMap`, which gives the same O(log n) ordered-min-extraction
//! behavior a red-black tree would, without re-deriving tree rotations by
//! hand for a property that's about ordering, not the specific data
//! structure that provides it.

use alloc::collections::BTreeMap;

pub type Pid = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PriorityClass {
    /// Lower deadline value = runs sooner. Ordered first so `RealTime`
    /// always sorts before `Normal`/`Idle` when classes are compared.
    RealTime(u64),
    Normal,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Ready,
    Running,
    Sleeping,
    Zombie,
}

#[derive(Debug, Clone)]
pub struct Process {
    pub pid: Pid,
    pub class: PriorityClass,
    pub state: ProcessState,
    pub vruntime: u64,
}

#[derive(Debug, Default)]
pub struct RunQueue {
    processes: BTreeMap<Pid, Process>,
    /// EDF ordering key: (deadline, pid) → pid. Keying by `(deadline, pid)`
    /// rather than just `deadline` keeps ties between two RealTime tasks
    /// with the same deadline deterministic instead of one silently
    /// overwriting the other's queue slot.
    real_time_order: BTreeMap<(u64, Pid), ()>,
    /// CFS ordering key: (vruntime, pid) → pid, same tie-breaking reason.
    normal_order: BTreeMap<(u64, Pid), ()>,
}

impl RunQueue {
    pub fn new() -> Self {
        RunQueue::default()
    }

    pub fn add_process(&mut self, process: Process) {
        match process.class {
            PriorityClass::RealTime(deadline) => {
                self.real_time_order.insert((deadline, process.pid), ());
            }
            PriorityClass::Normal => {
                self.normal_order.insert((process.vruntime, process.pid), ());
            }
            PriorityClass::Idle => {}
        }
        self.processes.insert(process.pid, process);
    }

    pub fn remove_process(&mut self, pid: Pid) -> Option<Process> {
        let process = self.processes.remove(&pid)?;
        match process.class {
            PriorityClass::RealTime(deadline) => {
                self.real_time_order.remove(&(deadline, pid));
            }
            PriorityClass::Normal => {
                self.normal_order.remove(&(process.vruntime, pid));
            }
            PriorityClass::Idle => {}
        }
        Some(process)
    }

    /// The real hybrid: earliest-deadline RealTime task first; otherwise
    /// smallest-vruntime Normal task; otherwise nobody is ready to run.
    pub fn pick_next_task(&self) -> Option<Pid> {
        if let Some(((_, pid), _)) = self.real_time_order.iter().next() {
            return Some(*pid);
        }
        self.normal_order.iter().next().map(|((_, pid), _)| *pid)
    }

    /// CFS vruntime accounting — re-keys the ordering entry since it's
    /// keyed by the old vruntime value.
    pub fn update_vruntime(&mut self, pid: Pid, time_used: u64) {
        let Some(process) = self.processes.get_mut(&pid) else { return };
        if process.class != PriorityClass::Normal {
            return;
        }
        let old_key = (process.vruntime, pid);
        process.vruntime += time_used;
        let new_key = (process.vruntime, pid);
        self.normal_order.remove(&old_key);
        self.normal_order.insert(new_key, ());
    }

    pub fn get(&self, pid: Pid) -> Option<&Process> {
        self.processes.get(&pid)
    }

    pub fn ready_count(&self) -> usize {
        self.processes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn rt(pid: Pid, deadline: u64) -> Process {
        Process { pid, class: PriorityClass::RealTime(deadline), state: ProcessState::Ready, vruntime: 0 }
    }
    fn normal(pid: Pid, vruntime: u64) -> Process {
        Process { pid, class: PriorityClass::Normal, state: ProcessState::Ready, vruntime }
    }

    #[test]
    fn realtime_task_always_preempts_normal_regardless_of_vruntime() {
        let mut rq = RunQueue::new();
        rq.add_process(normal(1, 0)); // smallest possible vruntime
        rq.add_process(rt(2, 1_000_000)); // far deadline, but still RealTime
        assert_eq!(rq.pick_next_task(), Some(2), "EDF class must win over CFS class even with a distant deadline");
    }

    #[test]
    fn among_realtime_tasks_earliest_deadline_wins() {
        let mut rq = RunQueue::new();
        rq.add_process(rt(1, 500));
        rq.add_process(rt(2, 100));
        rq.add_process(rt(3, 300));
        assert_eq!(rq.pick_next_task(), Some(2));
    }

    #[test]
    fn among_normal_tasks_smallest_vruntime_wins() {
        let mut rq = RunQueue::new();
        rq.add_process(normal(1, 50));
        rq.add_process(normal(2, 10));
        rq.add_process(normal(3, 30));
        assert_eq!(rq.pick_next_task(), Some(2));
    }

    #[test]
    fn empty_queue_picks_nothing() {
        let rq = RunQueue::new();
        assert_eq!(rq.pick_next_task(), None);
    }

    #[test]
    fn no_starvation_every_normal_task_eventually_runs() {
        // theorem scheduler_no_starvation, exercised directly: run the
        // scheduler loop with only Normal-class tasks and assert every
        // one of them gets picked at least once within a bounded number
        // of ticks, since running always increases the running task's
        // vruntime past the others.
        let mut rq = RunQueue::new();
        for pid in 1..=5u64 {
            rq.add_process(normal(pid, 0));
        }

        let mut ran: alloc::collections::BTreeSet<Pid> = alloc::collections::BTreeSet::new();
        let max_ticks = 200;
        for _ in 0..max_ticks {
            if ran.len() == 5 {
                break;
            }
            let Some(pid) = rq.pick_next_task() else { break };
            ran.insert(pid);
            rq.update_vruntime(pid, 10);
        }
        assert_eq!(ran.len(), 5, "every ready Normal-class task must run within a bounded number of ticks");
    }

    proptest! {
        /// theorem scheduler_no_starvation, generalized: for any set of
        /// Normal-class processes, repeatedly picking-and-charging vruntime
        /// visits every process within `n * max_reasonable_spread` ticks —
        /// no process is skipped forever purely because others keep being
        /// preferred.
        #[test]
        fn no_normal_task_starves_for_arbitrary_task_counts(n in 1usize..12) {
            let mut rq = RunQueue::new();
            for pid in 0..n as u64 {
                rq.add_process(normal(pid, 0));
            }
            let mut ran: alloc::collections::BTreeSet<Pid> = alloc::collections::BTreeSet::new();
            let max_ticks = (n as u32) * 50 + 10;
            for _ in 0..max_ticks {
                if ran.len() == n {
                    break;
                }
                let Some(pid) = rq.pick_next_task() else { break };
                ran.insert(pid);
                rq.update_vruntime(pid, 10);
            }
            prop_assert_eq!(ran.len(), n);
        }
    }

    #[test]
    fn remove_process_updates_both_orderings_consistently() {
        let mut rq = RunQueue::new();
        rq.add_process(normal(1, 5));
        rq.add_process(rt(2, 5));
        assert_eq!(rq.ready_count(), 2);

        rq.remove_process(2);
        assert_eq!(rq.pick_next_task(), Some(1), "removing the RealTime task must let CFS take over");
        assert_eq!(rq.ready_count(), 1);
    }
}
