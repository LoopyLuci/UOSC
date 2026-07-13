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
[cpu] NX=true (EFER.NXE) SMEP=false SMAP=false (CR4), each gated on a real CPUID check
[memory] real usable region 0x1473000..0x7fe0000, using 27501 pages for the real PhysicalAllocator
[PASS] CpuSecurity: real EFER.NXE/CR4.SMEP/CR4.SMAP match what CPUID said was supported
[PASS] EarlyBoot: GDT + IDT installed
[PASS] LateBoot: PIC/PIT/interrupts enabled
[PASS] Memory: real PhysicalAllocator over real bootloader memory map
[PASS] Paging: real kernel heap, mapped through real hardware page tables, holds real data
[page_fault] real #PF at 0x555555550000, demand-paged and resumed
[PASS] PageFault: a real #PF was triggered and demand-paged by the real handler, then resumed
[syscall] real int 0x80 trap from ring 3, rax=10 — returning to ring 3
[syscall] real int 0x80 trap from ring 3, rax=20 — returning to ring 3
[syscall] real int 0x80 trap from ring 3, rax=30 — returning to ring 3
[syscall] real int 0x80 EXIT trap from ring 3 — abandoning ring 3 for good
[PASS] Syscall: real CPL3 code made 3 real round-trip syscalls + 1 real exit trap via int 0x80
[PASS] Capability: real CapabilityBroker grants the issuer and denies a stranger
[scheduler_bridge] real RunQueue + 2 real task contexts initialized
[PASS] SchedulerBoot: real RunQueue initialized
[PASS] Sanctum: real vault created, entered, and region-isolated
[PASS] SanctumBoot phase
[PASS] Ipc: real capability-checked send/receive round-trip
[PASS] IpcBoot phase
[boot] BootSequencer ordering invariant holds: true
[task_a] real context switch resumed me, iteration 20
[task_b] real context switch resumed me, iteration 20
[task_a] real context switch resumed me, iteration 40
[task_b] real context switch resumed me, iteration 40
... (task_a/task_b keep alternating — real interleaving from real,
     hardware-timer-driven context switches, not a hardcoded print order —
     through iteration 200 each; note exactly *where* this interleaving
     falls in the log genuinely varies run to run — this specific run put
     it between the BootSequencer line and SyscallBoot below, an earlier
     run put it between SchedulerBoot and Sanctum — because the first
     real switch away from kernel_main's own flow happens whenever a
     timer tick first lands after scheduler_bridge::init(), which is real
     interrupt timing, not a fixed point in the code)
[boot] SyscallBoot: a real int 0x80 entry point now exists (see the Syscall check above) — BootSequencer's SyscallBoot phase itself is still not completed, since there is no general syscall ABI or process model behind it yet
[PASS] Scheduler: real context switches actually ran both kernel tasks, driven by the real hardware timer

=== UOSC boot self-test: 14/14 checks passed ===
```

QEMU's process exit code: `33`, which decodes (per `isa-debug-exit`'s
`(value << 1) | 1` convention) to `ExitCode::Success = 0x10` — a real,
scriptable pass signal, not a human reading a terminal. Reproduced across
independent BIOS runs (both on QEMU's default CPU model, where `SMEP`/
`SMAP` report unsupported and are correctly left off, and with
`-cpu qemu64,+smep,+smap` forcing both on — see "Real NX/SMEP/SMAP"
below) and an independent UEFI run (real EDK2 firmware, bundled with this
environment's QEMU install — see "UEFI boot" below), byte-for-byte
identical pass/fail shape each time, plus one clean rebuild from scratch
(`rm -rf target`) re-verified on the exact commit-bound code to rule out
stale-cache masking.

## What actually happens at boot, and what code runs it

1. **`bootloader_api`** (BIOS or UEFI path — both are boot-tested this
   pass, see "UEFI boot" below) loads the kernel ELF and hands control to
   `kernel_main`.
2. **Real hardware page tables, bootstrapped in two phases** — see "Real
   hardware page tables" below for the full mechanism and the two real
   bugs it took to get right. By the time anything else below runs, the
   kernel already has a real `OffsetPageTable` over the CPU's actual CR3
   and a real kernel heap backed by individually mapped pages, not a
   static BSS array.
3. **Real NX/SMEP/SMAP** (`cpu_features.rs`), immediately after the
   `OffsetPageTable` exists and before the first heap page is ever
   mapped — see "Real NX/SMEP/SMAP" below.
5. **Real GDT + IDT** (`gdt.rs`, `interrupts.rs`) — a genuine x86-64
   descriptor table setup, including a dedicated IST stack for double
   faults. This is the hardware-specific step `kernel/boot.ti`'s
   `init_gdt()`/`init_idt()` describes but `reference-rs` deliberately
   never implemented (no hardware to target from a portable `no_std` lib).
6. **Real PIC remap + PIT reprogram** (`interrupts.rs`, `pit.rs`) — and
   `pit.rs` calls `uosc_core::timer::pit_divisor` (the exact, tested
   function from `reference-rs`) to compute the divisor, then writes it to
   real hardware ports. First place in this whole effort where that
   "pure arithmetic" Phase-0 work drives real hardware.
7. **Real physical memory**: the bootloader's actual memory map is scanned
   for the largest usable region, and that real address range (minus the
   handful of frames the paging bootstrap already consumed) seeds the one
   real, persistent `uosc_core::memory::PhysicalAllocator` — the same
   buddy allocator `reference-rs`'s tests and the
   `allocate_deallocate_cycles_never_leak_pages` property test exercise,
   now allocating and freeing real physical pages, and now the same
   instance backing the heap and the page fault handler's demand paging
   too, not a disposable scratch copy.
8. **A real page fault, deliberately triggered and demand-paged.** See
   "Real hardware page tables" below.
9. **A real ring-3 → ring-0 privilege transition.** See "Real ring-3
   syscall" below.
10. **Real capability, sanctum, and IPC checks** — `uosc_core::capability`,
   `uosc_core::sanctum`, and `uosc_core::ipc` run their real logic (issue a
   token, grant the issuer, deny a stranger; create a vault, enter it,
   confirm inside-region access and outside-region denial; create a port,
   send and receive a capability-checked message) against real allocated
   state, not test fixtures.
11. **Real hardware-timer-driven scheduling, with a real context switch.**
   The PIT fires a real interrupt at 200 Hz; each one calls into
   `scheduler_bridge.rs`, which runs the real
   `uosc_core::scheduler::RunQueue::pick_next_task`/`update_vruntime` — the
   exact code `Scheduler.lean`'s two-task proof and `SchedulerN.lean`'s
   general n-task proof cover — and now, when that decision actually
   changes which task should run, calls `context::switch_to` to really
   switch the CPU to it. See "Real context switching" below for how.
12. **`uosc_core::boot::BootSequencer`** tracks real phase completion as
    each step above finishes, and `satisfies_ordering_invariant()` — the
    same function `Boot.lean` proves preserves Property 10 — is checked
    live at the end, not just in a unit test.

## Real hardware page tables

`paging.rs` is new this pass. A real `OffsetPageTable` over the CPU's
actual CR3 (built from `BootInfo::physical_memory_offset`, requested via
`Mapping::Dynamic` in `main.rs`'s `BOOTLOADER_CONFIG`), backing a real
kernel heap and a real, deliberately-triggered, demand-paged page fault —
not the in-memory `uosc_core::memory::PageTable` policy model standing in
for hardware any more.

**Two real bugs found getting this right**, both genuine and both fixed
before this landed, not smoothed over:

1. **A circular dependency.** The real, persistent
   `uosc_core::memory::PhysicalAllocator` allocates on the heap to build
   its own free-list bookkeeping (`Vec`-backed) — so the heap has to exist
   *before* that allocator does. First real attempt called
   `PhysicalAllocator::new()` with no heap set up at all: a real
   `memory allocation of 32 bytes failed` panic, straight from Rust's
   default OOM handler, the instant `seed_free_lists` tried its first
   `Vec::push`.
2. **A second, different real bug fixing the first one.** The next
   attempt used a small *static* bootstrap array for an initial heap, then
   tried to `linked_list_allocator::Heap::extend` into whatever virtual
   memory happened to sit right after that array — which turned out to be
   wherever the linker placed it within the kernel's own already-mapped
   image, not free virtual address space. Real result: `map_to` failed
   with a real `AccessViolation`, because the bootloader had already
   mapped something there.

**The actual fix**: a two-phase kernel heap (`allocator.rs`). Phase 1 maps
a handful of real pages at a fixed, deliberately unmapped virtual address
(`0x_4444_4444_0000`) using a trivial [`paging::BumpFrameAllocator`] —
just a counter, no `Vec`, no heap dependency at all — so the real heap
exists before `PhysicalAllocator::new()` ever runs. Phase 2, once that
allocator exists, extends the *same* live heap with more real pages
placed exactly at its own current top, which is now guaranteed to be free
virtual space too, since that whole region belongs to this module alone.
Nothing allocated during phase 1 — in particular the real, permanent
global `PhysicalAllocator` itself — is ever orphaned, because the heap is
grown in place and never replaced.

**Real, deliberately triggered page fault**: `main.rs`'s self-test writes
through a pointer into `paging::DEMAND_PAGE_REGION_START`, an address
nothing has mapped yet. That's a real `#PF` on real (emulated) hardware,
caught by `interrupts.rs`'s real `page_fault_handler`, which maps a real
page on the spot (via the same shared mapper/allocator everything else
uses) and returns — the faulting instruction genuinely retries and
succeeds. The handler only ever does this inside the designated demand-
page region (mirroring how `uosc_core::memory::
ProcessMemoryContext::handle_page_fault` already scopes lazy allocation to
a registered region in the portable model); anything else still panics,
so a wild pointer dereference stays a diagnosable fault instead of being
silently papered over.

**A third real bug, found this later pass but rooted here**: adding
`syscall.rs` grew the kernel binary enough to shift `PhysicalAllocator`'s
real starting page count to 27437 — and that specific, perfectly ordinary
value made `reference-rs`'s `PhysicalAllocator::seed_free_lists` blow up
into tens of thousands of tiny `Vec` pushes instead of a handful of large
ones, overflowing the 256 KiB bootstrap heap with a real `memory
allocation of 262144 bytes failed` panic. Not a bug in this crate — a real
bug in the already-"64 tests passing" `reference-rs` library itself,
invisible until a real, tightly-bounded kernel heap finally made the
difference between "wasteful" and "fails." Full writeup and fix in
`../reference-rs/STATUS.md`.

## Real ring-3 syscall

`syscall.rs` is new this pass: a real ring-3 (CPL3) → ring-0 privilege
transition, not CPL0 code merely pretending to be "userspace." A real
hand-assembled program runs at CPL3 on real hardware, makes **three real
round-trip syscalls plus one real exit syscall**, and the real handler
observes each value in order. Full mechanism in `syscall.rs`'s module
docs; short version:

- `enter_ring3` (naked `asm!`) saves the kernel's stack pointer and
  callee-saved registers — the same convention as
  [`context::switch_to`] — then builds a real `iretq` frame with real
  ring-3 GDT selectors (`gdt.rs`'s new `Descriptor::user_code_segment()`/
  `user_data_segment()`) and executes `iretq` to actually drop to CPL3.
- The ring-3 program is hand-assembled bytes, not a compiled Rust
  function's address — its exact length has to be known with certainty
  when copying it into a freshly mapped, real, user-accessible page, and
  hand-assembling avoids any risk of position-dependent relocations
  breaking when the bytes move to a new address. It makes four real
  traps in sequence: `mov eax,10/20/30; int 0x80` three times, then
  `mov eax,999; int 0x80` (the exit syscall).
- `int 0x80` traps into `syscall_entry_stub`, installed by raw address
  (`Entry::set_handler_addr`, not the typed `extern "x86-interrupt"`
  convention, since reading `rax` at the trap needs the real register
  file) with DPL set to `Ring3` so a real `int 0x80` from CPL3 doesn't
  raise a real `#GP` first.
- **Two genuinely different real returns**, not one: for the three
  ordinary syscalls, the stub does the textbook thing — record the
  value, then `iretq` using the *same* hardware-pushed interrupt frame
  the trap already left on the stack, resuming ring 3 at the very next
  instruction after `int 0x80`. Nothing is reconstructed; that's what
  `iretq` is for. Only the exit syscall abandons ring 3 — exactly like
  `scheduler_bridge.rs`'s `BOOT_PID` handback, loading the kernel stack
  pointer `enter_ring3` saved and `ret`ing straight back into
  `run_demo_syscall`'s caller.
- The branch between those two paths happens *before* calling into Rust,
  comparing the trapped `rax` directly — `record_syscall` is a normal
  `extern "C"` function, free to clobber `rax` as scratch, so relying on
  it surviving the call would have been a real bug waiting to happen.

**Two real bugs found building the original one-shot version**, on top of
the `seed_free_lists` bug above, plus one near-miss avoided while
extending it to a real round trip:

1. A real link failure: `sym KERNEL_RETURN_RSP` used as `lea rax,
   [{kernel_rsp}]` produced `relocation R_X86_64_32S cannot be used
   against local symbol` — this kernel links as a PIE binary (`-pie` in
   the real `rust-lld` invocation), so a `sym` reference needs explicit
   RIP-relative addressing (`lea rax, [rip + {kernel_rsp}]`), not the
   absolute-looking form that works for non-PIE targets.
2. **A real, reproducible hang**, found only after the link fix: the
   `int 0x80` IDT entry is (by `set_handler_addr`'s own documented
   default) an *interrupt* gate, which clears `IF` on entry — normally
   undone by the `iretq` a typed handler ends with. The original one-shot
   stub deliberately never `iretq`d, so without an explicit `sti`, `IF`
   stayed cleared *permanently* after the syscall demo returned — every
   later timer tick silently stopped firing, and `main.rs`'s `hlt()` loop
   (which only wakes on NMI when `IF=0`) hung forever the first time it
   ran. No crash, no panic — just a real, silent, total loss of
   interrupts, caught by noticing the self-test never finished rather
   than by any error message. Fixed by an explicit `sti` before the
   final `ret` on the exit path — the *continue* path added this pass
   doesn't need it, since its `iretq` already restores the saved
   `RFLAGS` (with `IF=1`) the normal way.
3. Extending to a round trip could easily have reintroduced bug 2 in a
   new shape (forgetting that only the *exit* path needs the manual
   `sti`, since the *continue* path's `iretq` already handles it) — kept
   out by branching to two clearly separate code paths in
   `syscall_entry_stub` rather than trying to share one tail between
   them, and verified directly: all three "returning to ring 3" log
   lines plus the one "abandoning ring 3" line appear on every run,
   proving each `iretq` really did resume ring 3 and each subsequent
   trap really did fire again, not just that the boot self-test happened
   to finish.

## Real NX/SMEP/SMAP

`cpu_features.rs` is new this pass. Real `EFER.NXE`, real `CR4.SMEP`, and
real `CR4.SMAP` — each independently gated on a real CPUID check (`CPUID.
80000001H:EDX.bit20` for NX, `CPUID.7.0:EBX.bit7`/`.bit20` for SMEP/SMAP),
never set blind. `main.rs` runs this immediately after `paging::init`,
before `allocator::init_bootstrap` maps the first heap page — setting
`PageTableFlags::NO_EXECUTE` before `EFER.NXE=1` is a real reserved-bit
`#PF` the first time hardware walks into that entry, not a hypothetical
one, so the ordering is load-bearing.

- **NX** is applied to every data page this kernel maps after boot: the
  kernel heap (`allocator.rs`, both bootstrap and real phases), the
  demand-paged region (`interrupts.rs`), and the ring-3 demo's user stack
  (`syscall.rs`) — all via `cpu_features::nx_flag()`, which returns the
  real flag only if CPUID actually confirmed support. The ring-3 demo's
  *code* page deliberately keeps NX off, since real ring-3 code has to
  actually execute out of it every boot.
- **SMEP** is enabled (via a real, CPUID-gated `CR4` write) but has no
  real trap to trigger in this codebase — the kernel never attempts to
  execute an instruction from a user-accessible page anywhere. Verified
  only as "the real bit is set when CPUID says it's supported" (the
  `CpuSecurity` check below), not empirically fault-tested. Said plainly
  rather than silently claimed as more than it is.
- **SMAP** *is* genuinely exercised: `syscall.rs`'s one real kernel write
  into a user-accessible page (copying the hand-assembled `USER_PROGRAM`
  into the freshly mapped code page) is wrapped in real `stac`/`clac`
  (`x86_64::instructions::smap::Smap`). This was verified the same way
  the syscall round-trip bugs above were — by actually breaking it on
  purpose and watching it fail: temporarily removing the `stac`/`clac`
  wrapping, rebuilding, and booting with `-cpu qemu64,+smep,+smap` (QEMU's
  default CPU model doesn't advertise SMEP/SMAP support at all, so forcing
  them on is the only way to actually exercise this path) produced a real,
  reproducible page fault:
  ```
  [PANIC] panicked at src\interrupts.rs:108:5:
  EXCEPTION: PAGE FAULT at 0x666666660016, error PageFaultErrorCode(PROTECTION_VIOLATION | CAUSED_BY_WRITE) — outside the demand-page region, cannot recover
  ```
  — a genuine supervisor-mode write to a `USER_ACCESSIBLE` page, correctly
  rejected by real `CR4.SMAP` hardware enforcement. Restoring the
  `stac`/`clac` wrapping and rebuilding returned the boot to a clean
  14/14, confirmed reproducibly on both the default CPU model (where
  SMAP is unsupported and therefore never enforced — the wrapping is a
  no-op there) and with `+smep,+smap` forced on (where it's real,
  load-bearing protection).
- **`CpuSecurity` self-test check**: reads back the real `EFER`/`CR4`
  hardware state after `cpu_features::init` runs and confirms every bit
  CPUID said was supported really did get set — a real register read-back,
  not just "the function was called."

## UEFI boot

Previously built but not boot-tested (no OVMF firmware available in that
environment). This pass found that the `qemu` package installed via
`scoop` already bundles real EDK2/OVMF firmware images
(`share/edk2-x86_64-code.fd` + `share/edk2-i386-vars.fd`, the latter
doubling as the x86_64 variable store) — no external download needed.
Booted with:

```
qemu-system-x86_64 \
  -drive if=pflash,format=raw,readonly=on,file=<...>/edk2-x86_64-code.fd \
  -drive if=pflash,format=raw,file=<writable copy of edk2-i386-vars.fd> \
  -drive format=raw,file=uosc-uefi.img \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 -serial stdio -display none -no-reboot
```

Real OVMF `BdsDxe` boot-manager output precedes the kernel's own, then the
same self-test runs and passes: **14/14 checks, exit code 33.** Both boot
paths are now real, observed, passing runs, not one tested and one merely
"should work."

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

## The GDT/segment-register bug (from the context-switching pass)

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
- **The page tables are real but narrow.** There's exactly one address
  space — everything (kernel code, heap, demand-paged region) lives in the
  single CR3 this kernel's own `OffsetPageTable` manages; there is no
  second, isolated address space, no process/kernel privilege separation
  (everything still runs at CPL0), and no unmapping/freeing path for
  pages once mapped (`paging::map_page`'s counterpart `unmap` doesn't
  exist yet). The physical allocator is still a single contiguous arena
  (the largest usable region reported by the bootloader, minus the
  handful of frames the paging bootstrap consumed) — a real multi-region
  allocator is still real, separate follow-on work, unchanged from
  `reference-rs/STATUS.md`'s original note.
- **The kernel heap is fixed-size** (256 KiB bootstrap + 1 MiB real =
  1.25 MiB total, `allocator.rs`'s `BOOTSTRAP_HEAP_SIZE`/`REAL_HEAP_SIZE`)
  — real, page-mapped memory, but a hardcoded ceiling, not something that
  grows on demand past that.
- **The ring-3 demo is real but narrow.** One hand-assembled program, four
  fixed traps (three real round trips back to ring 3, one real exit) —
  real, repeated privilege transitions, but still no general syscall ABI
  (no argument-passing convention beyond "the value happens to be in
  rax"), no process/exit semantics beyond this one hardcoded exit value,
  and only ever one ring-3 program existing at a time.
  `BootSequencer`'s `SyscallBoot` phase itself is still deliberately left
  incomplete — a real trap-and-handle mechanism now exists, but there is
  no general syscall ABI or process model behind it, and the live
  self-test output says so rather than silently marking the phase done.
- **NX/SMEP/SMAP are real but narrow.** NX and SMAP are both genuinely
  enforced and empirically exercised (see "Real NX/SMEP/SMAP" above); SMEP
  is enabled and CPUID-gated correctly but has no real fault-and-recover
  test in this codebase, since the kernel never attempts what it would
  catch. No page carries a fine-grained read-only/read-write distinction
  beyond what already existed (every mapped page here is still
  `WRITABLE`); no protection-key (`PKU`) support; no CET/shadow stacks.
- **No filesystem, no network, no SMP.**
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
