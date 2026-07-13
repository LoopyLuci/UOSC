# Status — honestly, not aspirationally

This crate is the first real slice of Phase 0 from the [UOSC architecture
roadmap](../README.md): a Rust reference implementation of the portable
kernel logic specified in `../kernel/*.ti` and `../drivers/*.ti`, kept in
sync with that specification rather than replacing it. `../proofs-lean4/`
sits alongside it: a real, mechanically-checked re-hosting of a subset of
`../proofs/kernel_security.ax`'s theorems.

**What "verified" means below**: every claim in this document is either a
number from a command actually run against this code, or explicitly marked
as not yet done. Nothing here says "proven" unless a machine checked it —
that's the standard this whole effort is trying to hold UOSC to, so it
applies to this document too.

## What's real right now

```
cargo build --lib             → compiles clean under #![no_std] (no warnings)
cargo test                    → 64 passed; 0 failed
cargo test --release          → 64 passed; 0 failed (same result under optimization)
cargo clippy --all-targets -- -W clippy::all   → 0 warnings
lean Capability.lean / Memory.lean / Scheduler.lean / SchedulerN.lean / Boot.lean / PageFault.lean   → all six exit 0 (proofs-lean4/)
```

Eight modules now, each a real port of one `.ti` file's portable-logic
subset:

| Module | Ports | Tests |
|---|---|---|
| `capability` | `kernel/capability.ti` | 9 |
| `memory` | `kernel/memory.ti` | 9 (incl. 1 property test) |
| `ipc` | `kernel/ipc.ti` | 8 (incl. 2 property tests, 1 real concurrent thread test) |
| `scheduler` | `kernel/scheduler.ti` | 7 (incl. 1 property test) |
| `sanctum` | `kernel/sanctum.ti` (vault lifecycle + isolation check; hardware TLB/cache setup excluded) | 6 (incl. 1 property test) |
| `boot` | new — gives `boot_sequence_integrity` a real referent (see bug #7) | 5 (incl. 1 property test) |
| `timer` | `drivers/timer.ti` (frequency/divisor/time-conversion arithmetic only; MMIO/MSR/port I/O excluded) | 7 (incl. 1 property test) |
| `console` | `drivers/console.ti` (CRLF translation, cursor/scroll math, printf scanner; serial/framebuffer I/O excluded) | 10 (incl. 1 property test) |

The property-based tests (`proptest`) each run 256 randomized cases by
default. The IPC concurrency test spawns two real OS threads and pushes
5,000 messages through the ring buffer, asserting every one arrives exactly
once, in order.

## Real, mechanically-checked proofs (new this pass)

`../proofs-lean4/` re-hosts 8 of the 10 numbered properties from
`kernel_security.ax` in actual Lean 4 (toolchain: `elan` 4.2.3 /
`lean` 4.31.0, installed via `scoop install elan`), checked against formal
models built to mirror what this crate's Rust code actually does — one of
those eight (Property 5) now has *two* independent proofs:

- Property 1 (`process_capability_confinement`)
- Property 2 (`memory_process_isolation`)
- Property 3 (`capability_revocation_effective`)
- Property 5 (`scheduler_no_starvation`) — **both** the two-task instance
  (`Scheduler.lean`) **and** the fully general n-task case, any number of
  tasks, any adversarial tie-break policy (`SchedulerN.lean`) — the latter
  was flagged in an earlier pass as "worked out but not yet formalized";
  it's now formalized, via a combined headroom-sum-and-tie-count measure
  that strictly decreases every step
- Property 7 (`page_fault_handler_correctness`) — **the spec's literal
  two-way claim is false**; `proofs-lean4/PageFault.lean` proves a
  concrete counterexample and the real three-way characterization that
  actually holds
- Property 8 (`capability_delegation_authentic`) — **permission-bound half
  only**; the signature/PKI half isn't modeled
- Property 9 (`sanctum_vault_isolation`) — same proof as Property 2, since
  a vault region and a process address space are the same shape of thing
- Property 10 (`boot_sequence_integrity`) — proved against `boot.rs`'s
  `BootSequencer`, the first time this theorem has had a real code
  referent at all (see bug #7)

Full detail, including exactly what's NOT covered and why (Properties 4, 6,
and the excluded half of 8), is in `../proofs-lean4/README.md` — this is
not "10/10 done," and that file says so explicitly.

## Real bugs found in the Titan specification while porting it

Porting forces you to actually run the logic, which is exactly why several
of these were invisible until now:

1. **`get_process_capability` in `kernel/ipc.ti:257-277` ignores its `pid`
   and `resource` arguments and unconditionally returns a fully-permissioned
   token.** Fixed in `ipc::PortTable::send_message`/`receive_message`,
   covered by `ipc::tests::send_without_any_capability_is_denied`.

2. **`has_allocation_capability` in `kernel/memory.ti:358-361` always
   returns `true`.** Fixed in `memory::ProcessMemoryContext::allocate_virtual`,
   covered by `memory::tests::allocate_virtual_is_denied_without_a_real_capability_grant`.

3. **`split_block` and `coalesce_blocks` in `kernel/memory.ti:333-341` are
   `Ok(())`.** Fixed in `memory::PhysicalAllocator`, covered by the property
   test `memory::tests::allocate_deallocate_cycles_never_leak_pages`.

4. **`RunQueue::pick_next_task` in `kernel/scheduler.ti:229-237` only
   implements the CFS half of the file's own "EDF + CFS hybrid"
   description.** Fixed in `scheduler::RunQueue::pick_next_task`, covered by
   `scheduler::tests::realtime_task_always_preempts_normal_regardless_of_vruntime`.

5. **The `RingBuffer` in `kernel/ipc.ti:299-354` cannot distinguish "full"
   from "empty" once the write index wraps past the read index.** Rewritten
   as a real SPSC lock-free queue; see
   `ipc::tests::real_concurrent_spsc_producer_consumer_loses_and_corrupts_nothing`.

6. **`RedBlackTree<K, V>` in `kernel/scheduler.ti:384-389` is an
   unimplemented stub.** `scheduler::RunQueue` uses `BTreeMap` instead.

7. **`kernel_security.ax`'s `theorem boot_sequence_integrity` describes a
   `BootPhase` type and a `boot_phase()` function that don't exist anywhere
   in `kernel/boot.ti`.** `boot.ti`'s `early_boot`/`late_boot` are a fixed,
   hardcoded call sequence with no phase tracking and no ordering
   enforcement at all — the theorem's own referent was never built. `boot.rs`
   is new code that actually builds it: `BootSequencer` refuses to mark a
   phase complete unless every earlier phase already is, covered by
   `boot::tests::skipping_a_phase_is_rejected` and the property test
   `boot::tests::ordering_invariant_holds_after_any_sequence_of_attempts`.

8. **Neither `init_pit_timer` nor `init_apic_timer`/`set_apic_frequency` in
   `drivers/timer.ti` validates `freq_hz != 0` before dividing by it**
   (`kernel/timer.ti:210`, `:234`) — a zero frequency divides by zero.
   A second, related bug in the same function: **`divisor * freq_hz` in
   `set_apic_frequency` is unchecked `u32` multiplication** that overflows
   for `freq_hz` above ~268 million. Both fixed in `timer::pit_divisor`/
   `timer::apic_initial_count`, which return `Err` instead of panicking or
   silently wrapping; covered by `timer::tests::zero_frequency_is_rejected_everywhere_not_panicked`
   and the property test `timer::tests::no_panics_for_any_nonzero_frequency`.

9. **`printf` in `drivers/console.ti:112-149` never actually writes its
   arguments.** The `%d`/`%u` match arm is `{ i += 2; }` and the `%s` arm is
   `{ if arg_index < args.len() { /* Format argument */ } arg_index += 1; i += 2; }`
   — both comments describing work that was never implemented, so every
   `printf` call in the Titan source silently drops every argument and
   prints only the literal text around them. Fixed in `console::format`,
   which actually substitutes each argument's `Display` output; covered by
   `console::tests::printf_substitutes_integer_arguments_in_order` and
   `console::tests::printf_substitutes_string_argument`.

10. **`kernel_security.ax`'s `theorem page_fault_handler_correctness` is
    itself wrong**, not just unproven — the only bug in this list found in
    the *proof* file rather than the `.ti` kernel source. It claims every
    fault resolves to `Success` or `PermissionDenied`, a two-way split. The
    real handler (correctly) has a third outcome: a fault outside every
    registered region is `Unmapped`, no permission decision involved at
    all. `proofs-lean4/PageFault.lean`'s `theorem_as_stated_is_false` is a
    machine-checked counterexample (an empty region list), followed by the
    real three-way characterization that actually holds.

## A real bug found in this crate itself, not the Titan source

Every bug above was in the specification this crate replaced. This one is
different: a real correctness/efficiency bug in `PhysicalAllocator::
seed_free_lists` (`memory.rs`) that survived this crate's own 64 passing
tests, `cargo clippy`, and every prior `kernel-x86_64` boot — because it
never actually broke anything running against a host machine's
effectively unlimited heap, or against the small alloc/dealloc self-tests
earlier `kernel-x86_64` passes happened to run.

The bound it used to pick each free block's size — `remaining.trailing_zeros()`
— finds the largest power of two that *evenly divides* `remaining`, not
the largest one that merely *fits within* it. For almost any oddly-bit-
patterned `total_pages` (a completely ordinary real value: 27437 real
pages), that forces single-page (order-0) blocks from the very first
iteration onward, cascading into a near-linear number of tiny blocks
instead of the intended O(log n) — tens of thousands of individual `Vec`
pushes for a real, multi-gigabyte memory region. `kernel-x86_64`'s new
real, hardware-page-mapped kernel heap (see its `STATUS.md`) is tightly
bounded (256 KiB for exactly this kind of bootstrap work), and that's
what finally turned an invisible inefficiency into a real, observed
`memory allocation of 262144 bytes failed` panic — a genuine integration
bug a correctness-only test suite was never going to catch, since
`seed_free_lists`'s *output* (an allocator that still allocates and frees
correctly) was never wrong, only wastefully constructed.

Fixed by using `remaining.ilog2()` — the largest power of two *not
exceeding* `remaining` — which is what "largest aligned power-of-two block
that fits" actually requires. All 64 existing tests still pass unchanged
(this was never a correctness bug from the test suite's point of view,
only from a real, memory-constrained caller's).

## What this deliberately does not claim

- **No bootable binary.** This is portable logic — no boot sequence, no
  hardware paging, no interrupt handling, no APIC/HPET/PIT timer code
  actually driving hardware, no serial/framebuffer I/O. A hardware
  bring-up effort (or a `bootloader`/`uefi-rs`-based binary crate built on
  top of this library) is separate, follow-on work.
- **`kernel/hypercall.ti` and `drivers/{block,input,network,graphics}.ti`
  are not represented here at all**, not even partially. Unlike
  `boot`/`console`/`sanctum`/`timer` (each of which had a real portable-
  logic subset worth extracting), these files are close to 100% direct
  hardware/hypervisor dispatch — `hypercall.ti` marshals a struct and
  executes `asm!("vmcall")`/`asm!("syscall")`/MSR reads with no
  computation to get right or wrong; the driver files are almost entirely
  MMIO/port I/O. There was nothing portable-logic-shaped to port.
- **8 of the 10 theorems in `proofs/kernel_security.ax` are now mechanically
  re-hosted** in `../proofs-lean4/` (Lean 4, no Mathlib, real `lean`
  compiler runs, not hand-waved) — Property 5 has two independent proofs
  (two-task and general n-task), which is why six `.lean` files cover eight
  property numbers. The remaining 2 — full detail in
  `proofs-lean4/README.md` — are Properties 4 (`ipc_message_atomicity`) and
  6 (`interrupt_handler_safety`), both of which need a real operational
  semantics of hardware/concurrency this codebase doesn't model and has no
  code referent for; `ipc.rs`'s real concurrent-thread test is the
  empirical evidence this codebase has for Property 4 instead. Property 8's
  signature/PKI half is also not covered — see `proofs-lean4/README.md` for
  why.
- **Cascading capability revocation is not implemented.** Demonstrated,
  not hidden, by `capability::tests::revoking_source_does_not_touch_an_independently_issued_delegate_token`
  and its Lean mirror, `Capability.lean`'s
  `revocation_is_per_capability_not_per_resource`.
- **`PageTable` and the `sanctum` vault registry are in-memory policy
  models, not hardware page-walkers or real TLB/cache isolation.** They
  model *who is allowed to translate what*, built so a hardware-specific
  backend can be written underneath them later without changing anything
  that calls them.
- **`sanctum::attest` is a real deterministic checksum, not a
  cryptographic HMAC-SHA256** the way `kernel/sanctum.ti`'s comments
  describe. It's real enough to catch in-place tampering between
  attestations (the only property `enter_vault` actually needs), but it is
  not a security-grade hash and shouldn't be treated as one outside this
  test-scale model.

## Why this is Phase 0 and not "done"

The roadmap frames Phase 0 as three things: a real reference
implementation, real proof-checking, and a real test suite. This pass now
delivers meaningful coverage of all three — 8 of UOSC's ~11 kernel/driver
files have a real ported logic subset, 8 of 10 specified theorems have a
real machine-checked result (one of which is a proof that the theorem as
originally stated is false, and one of which — the scheduler — is proved
for both a two-task instance and the fully general n-task case), and 64
tests (up from the original pass's 33) back all of it. It is still not
complete: 3 files have zero portable logic to port and are honestly
excluded rather than faked, 2 theorems (Properties 4 and 6) remain
unformalized because they need hardware/concurrency machinery this codebase
doesn't have and has no code referent for, and Property 8's signature/PKI
half isn't modeled. No hardware bring-up has been attempted at all.
Extending further — a real interrupt operational semantics, a PKI model for
delegation authenticity — is real, substantial, separate work.
