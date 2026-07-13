# Status — honestly, not aspirationally

This crate is the first real slice of Phase 0 from the [UOSC architecture
roadmap](../README.md): a Rust reference implementation of the portable
kernel logic specified in `../kernel/*.ti`, kept in sync with that
specification rather than replacing it.

**What "verified" means below**: every claim in this document is either a
number from a command actually run against this code, or explicitly marked
as not yet done. Nothing here says "proven" unless a machine checked it —
that's the standard this whole effort is trying to hold UOSC to, so it
applies to this document too.

## What's real right now

```
cargo build --lib     → compiles clean under #![no_std] (no warnings)
cargo test            → 33 passed; 0 failed
cargo clippy --all-targets -- -W clippy::all   → 0 warnings
```

Four modules, each a real port of one `.ti` file:

| Module | Ports | Tests |
|---|---|---|
| `capability` | `kernel/capability.ti` | 9 |
| `memory` | `kernel/memory.ti` | 9 (incl. 1 property test) |
| `ipc` | `kernel/ipc.ti` | 8 (incl. 2 property tests, 1 real concurrent thread test) |
| `scheduler` | `kernel/scheduler.ti` | 7 (incl. 1 property test) |

The property-based tests (`proptest`) each run 256 randomized cases by
default — `memory`'s and `ipc`'s exercise arbitrary interleavings of
allocate/deallocate and enqueue/dequeue, not just the hand-picked examples
in the unit tests next to them. The IPC concurrency test spawns two real OS
threads (a genuine producer and a genuine consumer, not a simulation) and
pushes 5,000 messages through the ring buffer, asserting every one arrives
exactly once, in order.

## Real bugs found in the Titan specification while porting it

Porting forces you to actually run the logic, which is exactly why several
of these were invisible until now:

1. **`get_process_capability` in `kernel/ipc.ti:257-277` ignores its `pid`
   and `resource` arguments and unconditionally returns a fully-permissioned
   token.** Nothing in the specified `send_message`/`receive_message` can
   ever actually deny access — the capability check is decorative. Fixed in
   `ipc::PortTable::send_message`/`receive_message`, which call the real
   `CapabilityBroker` and are covered by
   `ipc::tests::send_without_any_capability_is_denied`.

2. **`has_allocation_capability` in `kernel/memory.ti:358-361` always
   returns `true`.** Same class of bug — a capability check that can't deny
   anything isn't a capability check. Fixed in
   `memory::ProcessMemoryContext::allocate_virtual`, covered by
   `memory::tests::allocate_virtual_is_denied_without_a_real_capability_grant`.

3. **`split_block` and `coalesce_blocks` in `kernel/memory.ti:333-341` are
   `Ok(())`.** The buddy allocator's entire reason for existing — reusing
   freed memory efficiently — has no implementation. A real split/coalesce
   implementation is in `memory::PhysicalAllocator`, and
   `memory::tests::allocate_deallocate_cycles_never_leak_pages` (a property
   test) checks that arbitrary allocate/free sequences always return every
   page to the free pool.

4. **`RunQueue::pick_next_task` in `kernel/scheduler.ti:229-237` only
   implements the CFS half of the file's own "EDF + CFS hybrid"
   description** — the `deadline` field on `Process` and the `DeadlineQueue`
   type both exist and are never read anywhere. Fixed in
   `scheduler::RunQueue::pick_next_task`, covered by
   `scheduler::tests::realtime_task_always_preempts_normal_regardless_of_vruntime`.

5. **The `RingBuffer` in `kernel/ipc.ti:299-354` has a capacity check —
   `(write_pos + data.len()) % capacity == read_pos` — that cannot
   distinguish "full" from "empty" once the write index has wrapped past the
   read index, and its `dequeue` reads a stated "8 bytes total" length
   prefix from a 2-element array literal.** Rewritten as a real
   single-producer/single-consumer lock-free queue with an explicit `len`
   counter; see `ipc::RingBuffer`'s doc comment for the concurrency
   argument, and `ipc::tests::real_concurrent_spsc_producer_consumer_loses_and_corrupts_nothing`
   for the empirical check.

6. **`RedBlackTree<K, V>` in `kernel/scheduler.ti:384-389` is an
   unimplemented stub** (`insert`, `remove`, `min` all `/* ... */`).
   `scheduler::RunQueue` uses `BTreeMap` instead — the same
   O(log n) ordered-minimum behavior, without hand-deriving tree rotations
   for a property (ordering) that doesn't require that specific structure.

## What this deliberately does not claim

- **No bootable binary.** This is portable logic — no boot sequence, no
  hardware paging, no interrupt handling, no APIC/HPET/PIT timer code. A
  hardware bring-up effort (or a `bootloader`/`uefi-rs`-based binary crate
  built on top of this library) is separate, follow-on work.
- **`kernel/boot.ti`, `sanctum.ti`, `hypercall.ti`, `console.ti`, `timer.ti`,
  and the four `drivers/*` files are not ported here.** This pass scoped to
  the four subsystems the roadmap named as the capability/memory/IPC/
  scheduler core; the rest is real, separate follow-on work, not implied by
  anything above.
- **The ten theorems in `proofs/kernel_security.ax` are not re-hosted in
  Lean 4 or Isabelle/HOL in this pass.** No Lean/Isabelle toolchain was
  available in this environment, and writing proof scripts that can't
  actually be mechanically checked here would produce exactly the
  looks-verified-but-isn't artifact this whole effort exists to eliminate.
  What this crate offers instead, honestly labeled as what it is: property
  tests that check some of the same properties empirically (capability
  confinement, revocation, delegation bounds, no-starvation) over hundreds
  of randomized cases — real evidence, but evidence, not proof.
- **Cascading capability revocation is not implemented.** Revoking a source
  token does not revoke capabilities already delegated from it — this is
  demonstrated, not hidden, by
  `capability::tests::revoking_source_does_not_touch_an_independently_issued_delegate_token`,
  which passes today by asserting the delegate token *survives* its
  source's revocation. Closing this gap is real follow-on work, not a
  known-and-ignored bug.
- **`PageTable` here is an in-memory mapping table, not a hardware
  page-walker.** It's a real, correct model of the *policy* (what maps to
  what, with what access), built so a hardware-specific backend can be
  written underneath it later without changing anything that calls it.

## Why this is Phase 0 and not "done"

The roadmap frames Phase 0 as three things: a real reference
implementation, real proof-checking, and a real test suite. This delivers
the first and third for four of UOSC's nine subsystems, and is explicit
about not delivering the second. That's the honest scope of one pass —
not a claim that Phase 0 is complete.
