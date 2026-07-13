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
lean Boot.lean         # exits 0
lean PageFault.lean    # exits 0
```

No Mathlib, no `lakefile`, no network access needed — these five files use
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
| `Scheduler.lean` | `eventually_picked` / `eventually_picked'` | Property 5 (two-task case — see file header for the worked-out, not-yet-formalized n-task argument) | `RunQueue::pick_next_task` |
| `Boot.lean` | `complete_preserves_invariant`, `completing_in_order_reaches_full_completion` | Property 10 | `boot::BootSequencer` (new — the spec's own referent didn't exist before this pass, see bug #7) |
| `PageFault.lean` | `theorem_as_stated_is_false`, plus the real three-way characterization | Property 7 — **the spec's literal two-way claim is false against the real model**, see below | `ProcessMemoryContext::handle_page_fault` |

8 of the 10 numbered properties (1, 2, 3, 5-partial, 7, 8-partial, 9, 10)
now have a real machine-checked result behind them, against formal models
chosen to mirror what the Rust code actually does, not idealized
restatements. One of those eight (Property 7) isn't a proof that the
spec's theorem holds — it's a proof that the spec's theorem, taken
literally, is **false**, plus the corrected statement that actually holds.
That's a more useful outcome than either silently "fixing" the theorem
text to match reality or skipping it, and it's exactly the kind of finding
this whole re-hosting effort exists to surface.

## What's still not done, and why

- **Property 4 (`ipc_message_atomicity`)** and **Property 6
  (`interrupt_handler_safety`)** describe hardware/concurrency semantics
  (atomic memory operations, CPU interrupt execution) that would need a
  real operational-semantics model of the machine to state honestly — not
  a data-structure property like the ones above. `ipc.rs`'s
  `real_concurrent_spsc_producer_consumer_loses_and_corrupts_nothing` test
  is the empirical evidence this codebase actually has for Property 4;
  nothing here upgrades it to a proof.
- **Property 5's n-task case** (arbitrary queue size, vs. the two-task case
  actually proved) has a complete, correct mathematical argument written
  out in `Scheduler.lean`'s header — a `restMin`-over-all-opponents
  monovariant that avoids the tie-breaking trap a naive pairwise
  generalization falls into — but it is not yet formalized. Doing so
  needs list-index bookkeeping and a bounded "at most n-1 retirements"
  sub-induction that has a real chance of costing another multi-iteration
  debugging pass the way the two-task proof did; recorded precisely enough
  to be a bounded follow-on task rather than an open question.
- **`revocation_is_per_capability_not_per_resource`** is not a gap in this
  proof effort — it's a proof *of* a gap. Cascading revocation was never
  implemented (`capability.rs`'s own test says so), and this theorem is the
  formal version of that same admission, not a hidden shortcut.
- **Property 8's non-permission half** (delegation carries an authentic
  cryptographic signature proving the sender's identity) needs a PKI model
  this codebase doesn't have; only the permission-bound half is proved.

Extending coverage further (the n-task scheduler case, a real interrupt
operational semantics, a PKI model for delegation authenticity) is real,
substantial, separate work — not implied as "basically done" by anything
above.
