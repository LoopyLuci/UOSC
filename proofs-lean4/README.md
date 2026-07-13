# Real, mechanically-checked proofs (Lean 4, no Mathlib)

`../proofs/kernel_security.ax` states 10 theorems in a hand-rolled "Axiom"
proof language that nothing checks — the proof bodies invoke lemma names
(`resource_manager_checks_capability`, `separate_page_tables`,
`starvation_contradicts_finiteness`, ...) that are never defined or proved
anywhere in the repository. It looks verified. It isn't.

This directory re-hosts a real subset of those theorems in actual Lean 4,
type-checked by an actual Lean compiler, with proof terms the kernel
mechanically accepts. It exists specifically to close the gap flagged in
`../reference-rs/STATUS.md` as not yet attempted.

## How to check this yourself

```
elan --version    # 4.2.3, installed via `scoop install elan`
lean --version    # 4.31.0, pinned by ./lean-toolchain
lean Capability.lean   # exits 0, no output = every proof accepted
lean Memory.lean       # exits 0
lean Scheduler.lean    # exits 0
```

No Mathlib, no `lakefile`, no network access needed — these three files use
only Lean 4's bundled core library (`Init`), so `lean <file>.lean` type-checks
each one standalone. That was a deliberate scope choice: Mathlib is a
multi-GB dependency that can take significant time to fetch/build on first
use, and every property proved here is combinatorial (lists, `Nat`,
`Bool`), not real-analysis — core Lean is sufficient and keeps "can I
actually re-verify this" a one-line command.

## What's proved, and exactly what it corresponds to

| File | Theorem | `kernel_security.ax` property | `reference-rs` code it matches |
|---|---|---|---|
| `Capability.lean` | `delegate_never_exceeds_source` | Property 8 (permission-bound half) | `CapabilityBroker::delegate` |
| `Capability.lean` | `process_capability_confinement` | Property 1 | `CapabilityBroker::check_access` |
| `Capability.lean` | `revoked_capability_denies_access` | Property 3 | `CapabilityBroker::revoke` |
| `Capability.lean` | `revocation_is_per_capability_not_per_resource` | (documents a real, undischarged gap — see below) | `capability.rs`'s own gap test |
| `Memory.lean` | `no_translation_no_access` | Property 2 and Property 9 (same shape — a vault's memory region is a process address space under a different name) | `PageTable::translates` |
| `Memory.lean` | `distinct_processes_cannot_translate_each_others_pages` | Property 2 | `ProcessMemoryContext` |
| `Memory.lean` | `allocate_removes_page_from_pool` | `allocate_physical_uniqueness` axiom | `PhysicalAllocator::allocate` |
| `Scheduler.lean` | `eventually_picked` / `eventually_picked'` | Property 5 (two-task case — see file header) | `RunQueue::pick_next_task` |

6 of the 10 numbered properties (1, 2, 3, 5, 8-partial, 9) now have a real
machine-checked proof behind them, against a formal model chosen to mirror
what the Rust code actually does, not an idealized restatement.

## What's still not done, and why

- **Property 4 (`ipc_message_atomicity`)** and **Property 6
  (`interrupt_handler_safety`)** describe hardware/concurrency semantics
  (atomic memory operations, CPU interrupt execution) that would need a
  real operational-semantics model of the machine to state honestly — not
  a data-structure property like the ones above. `ipc.rs`'s
  `real_concurrent_spsc_producer_consumer_loses_and_corrupts_nothing` test
  is the empirical evidence this codebase actually has for Property 4;
  nothing here upgrades it to a proof.
- **Property 7 (`page_fault_handler_correctness`)** and **Property 10
  (`boot_sequence_integrity`)**: Property 10's own referent — a
  `BootPhase` state machine — doesn't exist anywhere in `kernel/boot.ti`
  (see `reference-rs/STATUS.md`'s bug list); `boot.rs`'s
  `BootSequencer` is new code built to give this property something real
  to be about, and is tested in Rust, not (yet) proved in Lean. Property 7
  is partially covered by `memory.rs`'s `handle_page_fault` and its tests,
  again empirically, not formally.
- **`revocation_is_per_capability_not_per_resource`** is not a gap in this
  proof effort — it's a proof *of* the gap. Cascading revocation was never
  implemented (`capability.rs`'s own test says so), and this theorem is the
  formal version of that same admission, not a hidden shortcut.
- **Property 8's non-permission half** (delegation carries an authentic
  cryptographic signature proving the sender's identity) needs a PKI model
  this codebase doesn't have; only the permission-bound half is proved.

Extending coverage further (the n-task scheduler case, a real interrupt
operational semantics, a PKI model for delegation authenticity) is real,
substantial, separate work — not implied as "basically done" by anything
above.
