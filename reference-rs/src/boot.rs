//! Boot-phase ordering invariant — new code giving **Property 10**
//! (`boot_sequence_integrity`) something real to be about.
//!
//! **Real bug found while porting (bug #7)**: `kernel/boot.ti`'s
//! `early_boot`/`late_boot` are a fixed, hardcoded call sequence
//! (`init_gdt()?; init_idt()?; memory::init_paging(...)?; ...`) with no
//! `BootPhase` type, no `boot_phase()` function, and no enforced dependency
//! ordering anywhere in the file — despite `kernel_security.ax` stating a
//! full theorem, by induction over `BootPhase`, about exactly that
//! machinery (`theorem boot_sequence_integrity`). The theorem's own
//! referent doesn't exist in the source it claims to describe: nothing
//! stops a future refactor of `boot.ti` from calling `scheduler::init_scheduler()`
//! before `memory::init_paging()` and nothing would notice.
//!
//! [`BootSequencer`] below is a real, minimal state machine that actually
//! enforces the property the theorem claims: a phase can only be marked
//! complete once every phase ordered before it already is. It does not
//! reach into hardware — no GDT/IDT/paging/SMP init is performed here, only
//! the ordering discipline those calls are supposed to follow.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BootPhase {
    EarlyBoot,
    LateBoot,
    SchedulerBoot,
    SanctumBoot,
    IpcBoot,
    SyscallBoot,
}

impl BootPhase {
    /// Mirrors the actual call order in `kernel/boot.ti`'s `early_boot` /
    /// `late_boot`: paging + capabilities before SMP/timer, scheduler
    /// before sanctum, sanctum before IPC, IPC before the syscall handler.
    const ORDER: [BootPhase; 6] = [
        BootPhase::EarlyBoot,
        BootPhase::LateBoot,
        BootPhase::SchedulerBoot,
        BootPhase::SanctumBoot,
        BootPhase::IpcBoot,
        BootPhase::SyscallBoot,
    ];

    fn index(self) -> usize {
        Self::ORDER.iter().position(|p| *p == self).expect("BootPhase::ORDER is exhaustive")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootError {
    OutOfOrder { attempted: BootPhase, missing: BootPhase },
    AlreadyComplete(BootPhase),
}

/// Real port of the invariant `boot_sequence_integrity` describes:
/// `∀ phase, boot_phase(phase) = Complete → ∀ earlier < phase, boot_phase(earlier) = Complete`.
#[derive(Debug)]
pub struct BootSequencer {
    completed: [bool; 6],
}

impl Default for BootSequencer {
    fn default() -> Self {
        Self::new()
    }
}

impl BootSequencer {
    pub fn new() -> Self {
        BootSequencer { completed: [false; 6] }
    }

    /// The actual enforcement: refuses to mark `phase` complete unless
    /// every phase ordered before it already is. This is what makes
    /// `boot_sequence_integrity` true by construction rather than by
    /// convention — the Titan source relies entirely on the latter.
    pub fn complete(&mut self, phase: BootPhase) -> Result<(), BootError> {
        let idx = phase.index();
        if self.completed[idx] {
            return Err(BootError::AlreadyComplete(phase));
        }
        for earlier in &BootPhase::ORDER[..idx] {
            if !self.completed[earlier.index()] {
                return Err(BootError::OutOfOrder { attempted: phase, missing: *earlier });
            }
        }
        self.completed[idx] = true;
        Ok(())
    }

    pub fn is_complete(&self, phase: BootPhase) -> bool {
        self.completed[phase.index()]
    }

    /// The theorem's exact statement, checkable directly: every phase
    /// ordered before a complete phase is itself complete.
    pub fn satisfies_ordering_invariant(&self) -> bool {
        BootPhase::ORDER.iter().enumerate().all(|(idx, _phase)| {
            !self.completed[idx] || BootPhase::ORDER[..idx].iter().all(|earlier| self.completed[earlier.index()])
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn phases_complete_in_order_succeed() {
        let mut seq = BootSequencer::new();
        for phase in BootPhase::ORDER {
            assert!(seq.complete(phase).is_ok());
        }
        assert!(seq.is_complete(BootPhase::SyscallBoot));
    }

    #[test]
    fn skipping_a_phase_is_rejected() {
        let mut seq = BootSequencer::new();
        seq.complete(BootPhase::EarlyBoot).unwrap();
        // Skips LateBoot.
        let result = seq.complete(BootPhase::SchedulerBoot);
        assert_eq!(
            result,
            Err(BootError::OutOfOrder { attempted: BootPhase::SchedulerBoot, missing: BootPhase::LateBoot })
        );
    }

    #[test]
    fn completing_the_very_first_phase_out_of_order_is_impossible() {
        let mut seq = BootSequencer::new();
        // SyscallBoot attempted with nothing done yet: first missing phase reported is EarlyBoot.
        let result = seq.complete(BootPhase::SyscallBoot);
        assert_eq!(
            result,
            Err(BootError::OutOfOrder { attempted: BootPhase::SyscallBoot, missing: BootPhase::EarlyBoot })
        );
    }

    #[test]
    fn double_completing_a_phase_is_rejected() {
        let mut seq = BootSequencer::new();
        seq.complete(BootPhase::EarlyBoot).unwrap();
        assert_eq!(seq.complete(BootPhase::EarlyBoot), Err(BootError::AlreadyComplete(BootPhase::EarlyBoot)));
    }

    proptest! {
        /// theorem boot_sequence_integrity, exercised directly: whatever
        /// sequence of (possibly invalid) completion attempts is thrown at
        /// the sequencer, the ordering invariant holds after every single
        /// one — accepted transitions never violate it, and rejected ones
        /// never get applied partially.
        #[test]
        fn ordering_invariant_holds_after_any_sequence_of_attempts(
            attempts in prop::collection::vec(0usize..6, 0..30)
        ) {
            let mut seq = BootSequencer::new();
            for idx in attempts {
                let phase = BootPhase::ORDER[idx];
                let _ = seq.complete(phase);
                prop_assert!(seq.satisfies_ordering_invariant());
            }
        }
    }
}
