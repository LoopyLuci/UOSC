# Status — a real, booted kernel, not a simulation of one

This crate is UOSC's first real bootable binary. Everything in
`../reference-rs` compiled and ran under `cargo test`; nothing in this repo
had ever executed as an actual kernel on actual (emulated) hardware before
this. This does.

**What "verified" means here**: every claim below is a command that was
actually run, against the actual compiled binary, in actual QEMU — not a
narrated description of what it should do.

## What's real right now

```
cargo +nightly build --release --target x86_64-unknown-none \
  -Zbuild-std=core,alloc,compiler_builtins -Zbuild-std-features=compiler-builtins-mem
  → compiles clean, 0 warnings

cargo +nightly clippy --target x86_64-unknown-none \
  -Zbuild-std=core,alloc,compiler_builtins -Zbuild-std-features=compiler-builtins-mem -- -W clippy::all
  → 0 warnings
```

Building the bootable image (`../kernel-x86_64-builder`, a small `std`-based
tool using the `bootloader` crate):

```
cd ../kernel-x86_64-builder
cargo +nightly run -- <path-to-compiled-kernel-elf> <output-dir>
  → produces uosc-bios.img and uosc-uefi.img
```

Booting it for real, headless, with the serial console redirected to stdout
and QEMU's `isa-debug-exit` device wired up so the self-test's result comes
back as a real process exit code:

```
qemu-system-x86_64 -drive format=raw,file=uosc-bios.img \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 -serial stdio -display none -no-reboot
```

**Actual output of that command, this pass, verbatim:**

```
UOSC x86-64 — real boot starting
[PASS] EarlyBoot: GDT + IDT installed
[PASS] LateBoot: PIC/PIT/interrupts enabled
[memory] real usable region 0x1161000..0x7fe0000, using 28287 pages for PhysicalAllocator
[PASS] Memory: real PhysicalAllocator over real bootloader memory map
[PASS] Capability: real CapabilityBroker grants the issuer and denies a stranger
[scheduler_bridge] real RunQueue + 2 real task contexts initialized
[PASS] SchedulerBoot: real RunQueue initialized
[PASS] Sanctum: real vault created, entered, and region-isolated
[PASS] SanctumBoot phase
[PASS] Ipc: real capability-checked send/receive round-trip
[PASS] IpcBoot phase
[boot] BootSequencer ordering invariant holds: true
[boot] SyscallBoot intentionally left incomplete — no real syscall entry point in this pass
[task_a] real context switch resumed me, iteration 20
[task_b] real context switch resumed me, iteration 20
[task_a] real context switch resumed me, iteration 40
[task_b] real context switch resumed me, iteration 40
[task_b] real context switch resumed me, iteration 60
[task_a] real context switch resumed me, iteration 60
... (task_a/task_b keep alternating — real interleaving from real,
     hardware-timer-driven context switches, not a hardcoded print order —
     through iteration 200 each)
[PASS] Scheduler: real context switches actually ran both kernel tasks, driven by the real hardware timer

=== UOSC boot self-test: 10/10 checks passed ===
```

QEMU's process exit code: `33`, which decodes (per `isa-debug-exit`'s
`(value << 1) | 1` convention) to `ExitCode::Success = 0x10` — a real,
scriptable pass signal, not a human reading a terminal. Reproduced 3
consecutive independent runs, byte-for-byte identical pass/fail shape
each time (exact interleaving of task_a/task_b lines varies run to run,
as real interrupt timing means it should).

## What actually happens at boot, and what code runs it

1. **`bootloader_api`** (BIOS or UEFI path — both images are built; only
   the BIOS path has actually been boot-tested in this pass, see below)
   loads the kernel ELF and hands control to `kernel_main`.
2. **Real GDT + IDT** (`gdt.rs`, `interrupts.rs`) — a genuine x86-64
   descriptor table setup, including a dedicated IST stack for double
   faults. This is the hardware-specific step `kernel/boot.ti`'s
   `init_gdt()`/`init_idt()` describes but `reference-rs` deliberately
   never implemented (no hardware to target from a portable `no_std` lib).
3. **Real PIC remap + PIT reprogram** (`interrupts.rs`, `pit.rs`) — and
   `pit.rs` calls `uosc_core::timer::pit_divisor` (the exact, tested
   function from `reference-rs`) to compute the divisor, then writes it to
   real hardware ports. First place in this whole effort where that
   "pure arithmetic" Phase-0 work drives real hardware.
4. **Real physical memory**: the bootloader's actual memory map is scanned
   for the largest usable region, and that real address range seeds a real
   `uosc_core::memory::PhysicalAllocator` — the same buddy allocator
   `reference-rs`'s tests and the `allocate_deallocate_cycles_never_leak_pages`
   property test exercise, now allocating and freeing real physical pages.
5. **Real capability, sanctum, and IPC checks** — `uosc_core::capability`,
   `uosc_core::sanctum`, and `uosc_core::ipc` run their real logic (issue a
   token, grant the issuer, deny a stranger; create a vault, enter it,
   confirm inside-region access and outside-region denial; create a port,
   send and receive a capability-checked message) against real allocated
   state, not test fixtures.
6. **Real hardware-timer-driven scheduling, with a real context switch.**
   The PIT fires a real interrupt at 200 Hz; each one calls into
   `scheduler_bridge.rs`, which runs the real
   `uosc_core::scheduler::RunQueue::pick_next_task`/`update_vruntime` — the
   exact code `Scheduler.lean`'s two-task proof and `SchedulerN.lean`'s
   general n-task proof cover — and now, when that decision actually
   changes which task should run, calls `context::switch_to` to really
   switch the CPU to it. See "Real context switching" below for how.
7. **`uosc_core::boot::BootSequencer`** tracks real phase completion as
   each step above finishes, and `satisfies_ordering_invariant()` — the
   same function `Boot.lean` proves preserves Property 10 — is checked live
   at the end, not just in a unit test.

## Real context switching

`context.rs` is new this pass. Two dedicated kernel tasks (`task_a`,
`task_b` in `scheduler_bridge.rs`), each with its own real 16 KiB stack,
are genuinely, physically switched between — a real save/restore of the
stack pointer and callee-saved registers (`rbp`, `rbx`, `r12`–`r15`) via a
hand-written naked-`asm!` `switch_to` routine, not a simulation of
switching. The mechanism, in short (full reasoning is in `context.rs`'s
module docs):

- A task being suspended is *not* resumed later via `iretq`. It's resumed
  via a second `ret` out of `switch_to` itself, landing right back inside
  `on_timer_tick`/`timer_interrupt_handler` on that task's own stack, with
  its original hardware interrupt frame still intact underneath. Only
  *then* does the ISR's own epilogue run its `iretq`, restoring that
  task's flags and resuming its normal code exactly where it left off.
- A task's *first* ever resumption has no earlier `call switch_to` to
  return into, so its stack is pre-built (`init_stack`) to `ret` into a
  small trampoline that does `sti` (nothing has gone through a real
  `iretq` for this task yet to otherwise restore that flag) and then jumps
  straight into its entry function.
- `interrupts.rs`'s timer handler was reordered to send the PIC's EOI
  *before* calling into the scheduler, not after — `on_timer_tick` may not
  return for many ticks once it switches away, and the 8259 PIC won't
  raise IRQ0 again until EOI'd, so sending it after would have silently
  stalled the timer for whichever task ends up running.
- `serial.rs`'s `_print` now wraps its lock in
  `x86_64::instructions::interrupts::without_interrupts` — real
  preemption means task code can be switched out mid-print while holding
  that lock, and the task switched into may want it too; a real deadlock,
  not a hypothetical one, the first time both tasks call
  `serial_println!` around the same time.

**Observed result**: `task_a` and `task_b` print alternately, genuinely
interleaved by real interrupt timing (see the boot output above), each
having independently reached iteration 200 by the time the tick budget
hands control back to the boot flow. No double fault, no triple fault, on
any of 3 independent runs.

## The one real bug this pass found and fixed

Setting up a brand-new GDT and switching `CS` to it is not enough on
x86-64: **`DS`/`ES`/`FS`/`GS`/`SS` keep whatever selectors they held before
the switch**, now pointing at slots in a descriptor table that no longer
exists. Nothing faults immediately — segment registers aren't checked
again until something implicitly consults one, which is exactly what
hardware interrupt entry/exit does. The result was a real, reproducible
double fault on the very first timer interrupt, with the actual timer
handler running to completion (confirmed via serial trace) and the fault
occurring on the `iretq` back to normal execution. Fixed by reloading all
five registers with the null selector immediately after loading the new
GDT (`gdt.rs::init`) — the standard, correct fix for 64-bit long mode,
where a null data segment is valid at CPL0. This is real hardware
behavior a portable `no_std` library has no way to exercise or catch;
it only showed up once this crate actually booted real (emulated)
hardware.

## What this deliberately does not claim

- **The context switch is real but narrow.** Exactly two statically
  defined kernel tasks exist; there is no task creation, no task exit, no
  blocking/IO-driven rescheduling (a task can only ever yield by being
  timer-preempted), and no SMP (the switch code's soundness argument in
  `context.rs`/`scheduler_bridge.rs` explicitly leans on "single core,
  only ever touched with interrupts disabled" — a second CPU would break
  that invariant and needs real synchronization, not attempted here).
  Handing control back to the boot flow after a fixed tick budget is a
  hardcoded sentinel (`BOOT_PID`), not the scheduler genuinely managing
  the kernel's own boot thread as a task.
- **No hardware page tables.** CR3 still points at whatever mapping the
  bootloader set up (identity or offset-mapped, per `bootloader_api`'s own
  default). `uosc_core::memory::PageTable` remains the in-memory policy
  model `reference-rs`'s `STATUS.md` already described — it is not wired
  to CR3, and no page fault has been deliberately triggered and handled by
  this binary's own fault path (the IDT has a real `page_fault_handler`
  installed and ready, but nothing in this self-test exercises it yet).
- **The kernel heap (`allocator.rs`) is a static array in the kernel's own
  BSS**, not memory obtained through `PhysicalAllocator` and mapped via a
  real page table. A heap actually backed by the real allocator is real,
  separate follow-on work once page tables exist.
- **UEFI boot is built but not tested.** `uosc-uefi.img` is produced by the
  same builder and should work — `bootloader_api` handles both paths with
  the same kernel binary — but no OVMF UEFI firmware was available in this
  environment to actually boot-test it in QEMU. Only the BIOS path above
  has a real, observed, passing run behind it.
- **No userspace, no syscall entry point, no filesystem, no network, no
  SMP.** `BootSequencer`'s `SyscallBoot` phase is deliberately left
  incomplete — there is no real syscall handler in this pass, and the live
  self-test output says so rather than silently marking it done.
- **Not RISC-V, not AArch64, not on real (non-emulated) hardware.** QEMU is
  a real, high-fidelity emulator, not a rubber stamp — but it is not a
  substitute for real hardware bring-up, which needs real boards, real
  firmware quirks, and testing this crate doesn't have access to.
- **Not post-quantum-anything, no secure boot, no live patching, no
  federation protocol.** None of that was attempted in this pass. Those
  remain exactly what they were before: real, substantial, separate
  engineering efforts — a security-reviewed cryptographic library
  integration, a hardware bring-up campaign, a distributed-systems
  protocol design — not something that follows from a bootable demo
  kernel, however real that demo is.

## Why this milestone matters anyway

Every subsystem exercised above — capability, memory, sanctum, ipc,
scheduler, boot ordering — is the *exact same code* covered by
`reference-rs`'s 64 tests and 8 of `proofs-lean4`'s 10 machine-checked
theorems. Until this pass, all of that evidence was about code running
under `cargo test` on the host OS. Now there is a second, independent kind
of evidence for the same code: it boots, initializes real hardware, and
runs correctly when driven by a real interrupt on real (emulated) x86-64 —
closing exactly the gap `reference-rs/STATUS.md` named as out of scope
("no bootable binary... is separate, follow-on work") the first time that
follow-on work was actually done.
