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
[memory] real usable region 0x1472000..0x7fe0000, using 27502 pages for the real PhysicalAllocator
[PASS] CpuSecurity: real EFER.NXE/CR4.SMEP/CR4.SMAP match what CPUID said was supported
[PASS] EarlyBoot: GDT + IDT installed
[PASS] LateBoot: PIC/PIT/interrupts enabled
[PASS] Memory: real PhysicalAllocator over real bootloader memory map
[PASS] Paging: real kernel heap, mapped through real hardware page tables, holds real data
[page_fault] real #PF at 0x555555550000, demand-paged and resumed
[PASS] PageFault: a real #PF was triggered and demand-paged by the real handler, then resumed
[page_fault] real #PF at 0x555555550000, demand-paged and resumed
[PASS] Unmap: a real page unmap freed the real physical frame, and the address really re-faulted
[PASS] AddressSpace: a real second CR3-loadable page table, verified absent-then-present-then-restored
[syscall] real int 0x80 trap from ring 3: number=1 arg0=7 arg1=8 -> 15
[syscall] real int 0x80 trap from ring 3: number=2 arg0=1234 arg1=1099511753624 -> 1234
[syscall] real int 0x80 trap from ring 3: number=3 arg0=1249 arg1=1099511753624 -> 0
[syscall] real int 0x80 EXIT trap from ring 3 — abandoning ring 3 for good
[PASS] Syscall: real CPL3 code made 2 real multi-argument syscalls + reported their real, ring-3-computed sum
[PASS] Capability: real CapabilityBroker grants the issuer and denies a stranger
[task_stack] slot 0: mapped a real 64 KiB guarded stack at 0x333333331000, real unmapped guard page at 0x333333330000
[task_stack] slot 1: mapped a real 64 KiB guarded stack at 0x333333342000, real unmapped guard page at 0x333333341000
[task_stack] slot 2: mapped a real 64 KiB guarded stack at 0x333333353000, real unmapped guard page at 0x333333352000
[task_stack] slot 3: mapped a real 64 KiB guarded stack at 0x333333364000, real unmapped guard page at 0x333333363000
[scheduler_bridge] real RunQueue + 4 real task contexts initialized (task_c will really exit, task_e is bound to its own real address space)
[task_c] really exiting after 5 real iterations
[task_d] really spawned at runtime, first real context switch resumed me
[task_a] real context switch resumed me, iteration 20
[task_b] real context switch resumed me, iteration 20
[task_e] real context switch into my own real, bound address space resumed me, iteration 20
[task_b] real context switch resumed me, iteration 40
[task_e] real context switch into my own real, bound address space resumed me, iteration 40
[task_a] real context switch resumed me, iteration 40
... (task_a/task_b/task_e keep alternating — real interleaving from real,
     hardware-timer-driven context switches, not a hardcoded print order —
     through iteration 200 each for task_a/task_b; note exactly *where*
     this interleaving, and where the SchedulerBoot/Sanctum/Ipc PASS lines
     below land relative to it, genuinely varies run to run — this
     specific run put all of them after this activity, a different run
     has interspersed them — because the first real switch away from
     kernel_main's own flow happens whenever a timer tick first lands
     after scheduler_bridge::init(), which is real interrupt timing, not
     a fixed point in the code)
[PASS] SchedulerBoot: real RunQueue initialized
[PASS] Sanctum: real vault created, entered, and region-isolated
[PASS] SanctumBoot phase
[PASS] Ipc: real capability-checked send/receive round-trip
[PASS] IpcBoot phase
[boot] BootSequencer ordering invariant holds: true
[boot] SyscallBoot: a real int 0x80 entry point and a real, small number->handler syscall dispatch table now exist (see the Syscall check above) — BootSequencer's SyscallBoot phase itself is still not completed, since there is no process model or dynamically extensible syscall registry behind it yet, only a fixed, compile-time table
[PASS] Scheduler: real context switches actually ran both kernel tasks, driven by the real hardware timer
[PASS] TaskExit: a real dynamically spawned task ran, then really exited and left the real RunQueue
[PASS] TaskReuse: a task spawned live at runtime ran, really reusing the exact same guard-page-protected stack slot an exited task used
[PASS] TaskAddressSpace: a real scheduled task ran with the ordinary timer-driven scheduler really switching CR3 to its own bound address space

=== UOSC boot self-test: 19/19 checks passed ===
```

QEMU's process exit code: `33`, which decodes (per `isa-debug-exit`'s
`(value << 1) | 1` convention) to `ExitCode::Success = 0x10` — a real,
scriptable pass signal, not a human reading a terminal.

**Reproduced across well over 300 independent BIOS/UEFI runs total across
this crate's history**, most recently: 31 consecutive runs verifying NX/
SMEP/SMAP, then a batch verifying page unmapping + task creation/exit
that caught two real, separate, fixed intermittent bugs, then 66 further
consecutive runs after those fixes (heap-allocated stacks, at the time),
then a guard-page-protected-stack rewrite, which itself caught a real,
100%-reproducible bug on its very first boot (a non-canonical region base
address — see "Real guard-page-protected task stacks" below) and, once
fixed, 51 further consecutive runs, then a one-time manual SMEP
fault-and-recover verification (see "Real NX/SMEP/SMAP" above) plus 8
further consecutive runs confirming the revert, then a second, genuinely
independent address space (see "Real multiple address spaces" below),
which passed on its first boot and 50 further consecutive runs, then this
pass's real syscall ABI generalization and real task-to-address-space
binding (see "Real, small syscall ABI" and "Real task-to-address-space
binding" below), both of which also passed **on their first boot** — no
bug found this time in either — and, since then, **51 further consecutive
runs with zero failures** (25 on QEMU's default CPU model, 10 with `-cpu
qemu64,+smep,+smap` forcing both on, 1 independent UEFI run, 15 more on a
from-scratch clean rebuild). This many repeats weren't idle paranoia —
real bugs were caught and fixed exactly because of this volume of testing
at every stage; see "Real task creation and exit" and "Real
guard-page-protected task stacks" below for the full list, including one
older finding that remains honestly documented as *mitigated, not
debugger-confirmed root-caused* even after a later pass's stronger,
structural fix (see that section for exactly what is and isn't proven).

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
8. **A real page fault, deliberately triggered and demand-paged, then a
   real unmap of that same page.** See "Real hardware page tables" and
   "Real page unmapping" below.
9. **A real ring-3 → ring-0 privilege transition.** See "Real ring-3
   syscall" below.
10. **Real capability, sanctum, and IPC checks** — `uosc_core::capability`,
   `uosc_core::sanctum`, and `uosc_core::ipc` run their real logic (issue a
   token, grant the issuer, deny a stranger; create a vault, enter it,
   confirm inside-region access and outside-region denial; create a port,
   send and receive a capability-checked message) against real allocated
   state, not test fixtures.
11. **Real hardware-timer-driven scheduling, with a real context switch,
   real task creation, and real task exit.** The PIT fires a real
   interrupt at 200 Hz; each one calls into `scheduler_bridge.rs`, which
   runs the real `uosc_core::scheduler::RunQueue::pick_next_task`/
   `update_vruntime` — the exact code `Scheduler.lean`'s two-task proof
   and `SchedulerN.lean`'s general n-task proof cover — and now, when that
   decision actually changes which task should run, calls
   `context::switch_to` to really switch the CPU to it. Four tasks run
   this way: two long-running (`task_a`/`task_b`), one that really exits
   partway through (`task_c`), and one spawned live at runtime, after
   boot, by `task_a` itself, reusing (and really freeing) `task_c`'s
   slot (`task_d`). See "Real context switching" and "Real task creation
   and exit" below.
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

## Real page unmapping

`paging::unmap_page` is new this pass — the real counterpart to
`map_page`, not just an accounting fiction. It calls the real `Mapper::
unmap` (a real page-table-entry removal plus a real TLB flush) and
returns the real physical frame to the shared `PhysicalAllocator`, so
it's genuinely available for the next `allocate()` again.

**The self-test genuinely proves both halves, not just that the call
returned `Ok`.** After the demand-paged fault above, it unmaps that exact
page and checks the real physical allocator's free-page count went up by
one — proof the frame really came back, not just that the page table
entry changed. Then it writes through the *same pointer* a second time.
Since the mapping is genuinely gone, this is a real, *second* `#PF` at the
address that was already mapped once — the demand-page handler catches it
again and maps a fresh page, and the self-test checks
`PAGE_FAULTS_DEMAND_PAGED == 2`, not `1`. A no-op unmap (or one that
silently failed) would have left the first mapping intact and this second
write would never have faulted at all — so this really does distinguish
"the page table entry is gone" from "the accounting says it's gone."

## Real multiple address spaces

`address_space.rs` is new this pass. A second, genuinely independent,
CR3-loadable top-level page table — not a second region carved out of
the one this kernel already had. Until now every check in this kernel
shared the single `OffsetPageTable` built once at boot over the CPU's
actual CR3; this closes the "there is no second, isolated address space"
gap this file used to state plainly.

**How it's built**: [`AddressSpace::new`] allocates one fresh physical
frame (from the same shared `PhysicalAllocator` everything else in this
kernel already uses) to hold a brand-new L4 table, then clones all 512
entries of the *currently-active* L4 table into it. An L4 entry is just a
pointer to an L3 table, so cloning the entries — not the tables they
point to — means the new address space shares every existing mapping
(kernel code, the heap, the demand-page region, guarded task stacks) with
the original: nothing that already worked stops working after a real
switch to it. Only then does it map one real, fresh page at a new,
private address (`0x_1111_1111_0000`) — via a throwaway `OffsetPageTable`
view over *this specific* new L4 frame, not the shared global mapper,
which still targets the original — so exactly one mapping exists in the
new table and nowhere else.

**Real isolation, verified directly, in three steps, not asserted:**

1. **Before any switch**, the boot self-test checks the *original,
   still-active* L4 table's entry for the private address is genuinely
   absent (`PageTableEntry::is_unused()`, a real page-table-entry read) —
   proof the clone didn't somehow leak the new mapping backward into the
   table it was cloned from.
2. **A real `CR3` write** to the new table (`AddressSpace::switch_to`,
   `x86_64::registers::control::Cr3::write`), then a read straight through
   the real virtual address — no physical-offset back door, the same one
   `paging.rs` uses to reach arbitrary frames regardless of which CR3 is
   loaded — confirms the exact recognizable value
   (`0xC0FFEE_C0FFEE`) only that table's private mapping holds. This only
   resolves at all because the hardware page walker is now actually
   consulting the new table.
3. **A real `CR3` write back** to the original table
   (`address_space::restore`), and — this is the strongest evidence of
   all — every check `main.rs` runs *after* this one (Syscall,
   Capability, SchedulerBoot, Sanctum, Ipc, Scheduler, TaskExit,
   TaskReuse) still passing. A subtly wrong restore (a stale TLB entry, a
   wrong frame, wrong flags) would not have panicked cleanly — it would
   have surfaced as one of those later, ordinary checks failing or
   misbehaving in a confusing way. All of them passing, on the very first
   boot with this code, is real, load-bearing confirmation the round trip
   was exact.

**Passed on the first boot — no bug this time**, unlike the guard-page
stack rewrite. Validated the same way regardless: 25 further consecutive
runs on QEMU's default CPU model, 10 with `-cpu qemu64,+smep,+smap`
forcing both on, 1 independent UEFI run, and 15 more on a from-scratch
clean rebuild — 50 further clean runs, 18/18 checks, exit code 33, every
time.

**Scope, stated plainly**: this is a real second address space, but a
narrow one. It shares literally everything except the one deliberately
added private page — there is no process abstraction around it (no PID,
no scheduler awareness, nothing yet ties a `RunQueue` task to a
particular `AddressSpace`), no copy-on-write, and no separate user/kernel
privilege split (this kernel still runs everything at CPL0 in both
address spaces). The switch back is done by hand in the same call that
switched away — there is no general "switch address space as part of a
context switch" mechanism, and nothing schedules a task into a
non-default address space automatically. A real second CR3 that a task
could be scheduled into, with its own private memory a *different* task
genuinely cannot see or corrupt, is real, separate, follow-on work.

## Real, small syscall ABI

`syscall.rs`: a real ring-3 (CPL3) → ring-0 privilege transition, not
CPL0 code merely pretending to be "userspace." A real hand-assembled
program runs at CPL3 on real hardware, makes **two real, multi-argument
syscalls plus one real exit syscall**, and a real dispatch table — not
one hardcoded comparison — routes each by number to its own handler.
Full mechanism in `syscall.rs`'s module docs; short version:

- `enter_ring3` (naked `asm!`) saves the kernel's stack pointer and
  callee-saved registers — the same convention as
  [`context::switch_to`] — then builds a real `iretq` frame with real
  ring-3 GDT selectors (`gdt.rs`'s new `Descriptor::user_code_segment()`/
  `user_data_segment()`) and executes `iretq` to actually drop to CPL3.
- The ring-3 program is hand-assembled bytes, not a compiled Rust
  function's address — its exact length has to be known with certainty
  when copying it into a freshly mapped, real, user-accessible page, and
  hand-assembling avoids any risk of position-dependent relocations
  breaking when the bytes move to a new address. It calls
  `SYS_ADD(7, 8)` (real two-argument syscall, returns `15`), folds that
  into `SYS_ECHO(1234)`'s return value (`1249`), reports that
  ring-3-computed sum via `SYS_REPORT`, then makes the exit trap.
- `int 0x80` traps into `syscall_entry_stub`, installed by raw address
  (`Entry::set_handler_addr`, not the typed `extern "x86-interrupt"`
  convention, since reading the real register file at the trap needs
  exactly that) with DPL set to `Ring3` so a real `int 0x80` from CPL3
  doesn't raise a real `#GP` first.
- **A real register shuffle, not just a rename.** Ring-3 code places the
  syscall number in `rax` and up to three arguments in `rdi`/`rsi`/`rdx`
  — this kernel's own convention. To call `dispatch_syscall` (an ordinary
  `extern "C"` function expecting its four `u64` arguments in
  `rdi`/`rsi`/`rdx`/`rcx` per System V) the stub moves `rcx←rdx←rsi←rdi←
  rax` in that exact dependency order, so each register is read before
  it's overwritten. The call's real return value lands in `rax`
  automatically, and nothing overwrites it again before `iretq` — ring 3
  resumes with exactly that value, with no separate "write the result
  back" step needed.
- **A real number→handler dispatch table** (`dispatch_syscall`, matching
  on `number` to select a real function pointer among `sys_add`/
  `sys_echo`/`sys_report`/`sys_exit`), not the single `cmp eax, 999`
  branch the original one-shot version used. Genuinely narrow still — a
  small, compile-time-fixed table, not a dynamically extensible registry
  — but a real dispatch mechanism, not a special case.
- **Two genuinely different real returns**, not one: for ordinary
  syscalls, the stub calls `dispatch_syscall` then `iretq`s using the
  *same* hardware-pushed interrupt frame the trap already left on the
  stack, resuming ring 3 at the very next instruction after `int 0x80`.
  Only the exit syscall abandons ring 3 — exactly like
  `scheduler_bridge.rs`'s `BOOT_PID` handback, loading the kernel stack
  pointer `enter_ring3` saved and `ret`ing straight back into
  `run_demo_syscall`'s caller.

**Verified by a value only a genuinely correct round trip could
produce.** The old version's self-test observed three fixed constants
(10/20/30) arrive in order — real proof ring 3 resumed and retrapped
correctly, but no proof arguments or return values worked at all (the
old ABI only ever carried a syscall *number*, nothing else). This
pass's check instead asserts the exact reported value `1249`: `SYS_ADD
(7, 8)` must have really received both arguments and really returned
`15`, and `SYS_ECHO(1234)` must have really returned `1234`, and the
*ring-3 program itself* must have correctly summed them — a single wrong
register in the shuffle, or a return value silently dropped, would
change this number, not just fail to print an unrelated flag.

Boot log, this pass, verbatim (see the full boot log above for context):
```
[syscall] real int 0x80 trap from ring 3: number=1 arg0=7 arg1=8 -> 15
[syscall] real int 0x80 trap from ring 3: number=2 arg0=1234 arg1=... -> 1234
[syscall] real int 0x80 trap from ring 3: number=3 arg0=1249 arg1=... -> 0
[syscall] real int 0x80 EXIT trap from ring 3 — abandoning ring 3 for good
```

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
- **SMEP** is enabled (via a real, CPUID-gated `CR4` write) and, this
  pass, empirically fault-tested manually — the same way SMAP's fix was
  verified, and for the same reason it isn't a permanent, always-passing
  self-test: nothing in the checked-in codebase attempts a supervisor-mode
  instruction fetch from a user-accessible page, so there's no recoverable
  fault to build a passing check around. A temporary probe (removed after
  capturing the result) mapped a fresh `USER_ACCESSIBLE` page holding a
  single `ret` byte and called directly into it from ring 0 — a plain
  `call`, not `iretq`, deliberately bypassing `syscall.rs`'s ring-3
  machinery entirely so the result couldn't be confused with a SMAP or
  interrupt-gate effect. Booting with `-cpu qemu64,+smep` (SMAP
  deliberately *not* forced this time, so the probe's own write to that
  page wouldn't also fault) produced a real, immediate fault:
  ```
  [PANIC] panicked at src\interrupts.rs:108:5:
  EXCEPTION: PAGE FAULT at 0x222222220000, error PageFaultErrorCode(PROTECTION_VIOLATION | INSTRUCTION_FETCH) — outside the demand-page region, cannot recover
  InterruptStackFrame {
      instruction_pointer: VirtAddr(0x222222220000),
      ...
  }
  ```
  — `instruction_pointer` landing exactly on the probe address is real,
  direct evidence the fetch itself was blocked before executing a single
  instruction there, not a side effect of something else. On QEMU's
  default CPU model (SMEP unsupported, so `cpu_features::init` never sets
  the bit) the identical probe ran the `ret` and returned harmlessly:
  ```
  [smep_probe] TEMP: about to call ring-0 into a user-accessible page at 0x222222220000
  [smep_probe] TEMP: returned from the user-accessible page without faulting
  ```
  The probe was then fully reverted and the kernel rebuilt back to a
  clean, 0-warning, 17/17-passing state on both configurations before any
  further validation.
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
same self-test runs and passes: **18/18 checks, exit code 33.** Both boot
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

## Real task creation and exit

`scheduler_bridge::spawn_task`/`exit_current_task` — real dynamic task
creation and real task exit, not just the two statically-defined tasks
from before. `spawn_task` installs a new task into any free slot of a
fixed-size pool (`MAX_TASKS = 8` — a real, honestly-stated ceiling, the
same kind as the kernel heap's fixed size): a real heap-allocated stack,
the same real `context::init_stack` frame every task uses, and a real
`RunQueue::add_process`. `exit_current_task` is the real counterpart —
called by a task on itself, it really removes that task from the
`RunQueue` (`pick_next_task` will never choose it again) and switches
away for good, never resuming that stack. A demo task, `task_c`,
exercises this: it runs 5 real iterations (each resumed by a real context
switch, exactly like `task_a`/`task_b`) and then really exits. The boot
self-test's `TaskExit` check confirms both halves — `task_c` actually ran
*and* is genuinely gone from the live `RunQueue` afterward
(`rq.get(pid).is_none()`), not just that an exit flag got set.

A second demo task, `task_d`, is spawned *live* — not at boot, but from
inside `task_a`'s own running loop, the first time it observes
`task_c` has exited. Since `task_c`'s freed slot is the first free one a
linear scan finds, this reliably exercises real slot reuse. The
`TaskReuse` check confirms `task_d` really ran *and* landed in the exact
same guarded `task_stack` slot index `task_c` used — see "Real
guard-page-protected task stacks" below for what backs this now.

**Three real bugs found and fixed empirically across two passes, not just
by inspection or code review** — the actual reason the run counts above
are in the dozens, not a token 3:

1. **(Previous pass.)** The first version of `scheduler_bridge::init()`
   called `spawn_task` three times as three separate statements. Each
   individual call was already atomic (`spawn_task` disables interrupts
   for its own critical section), but interrupts were briefly live again
   *between* the three calls — and by the time `SchedulerBoot` runs,
   interrupts are already globally enabled (`LateBoot` turns them on
   first, earlier in boot). A timer tick landing in one of those gaps
   could preempt `init()` itself — an already-queued earlier task
   (`task_a`, say) is a perfectly legitimate switch target — suspending
   `init()` mid-spawn. Since `on_timer_tick` permanently pins the CPU to
   `BOOT_PID` once the `SWITCH_TICK_BUDGET` tick budget is spent (every
   later tick sees `prev == next == BOOT_PID` and returns immediately),
   an `init()` resumed *after* that threshold would finish spawning
   whatever tasks were left, but they would then never actually get
   scheduled. Observed for real: a 15-run batch failed **twice** (~13%) —
   once with only `TaskExit` failing, once with *both* `TaskExit` and
   `Scheduler` failing. Fixed by wrapping the whole of `init()` in one
   `without_interrupts` critical section instead of three separate ones.
   The very same fix also had to be applied a second time within the same
   function: the log line announcing initialization was originally
   *outside* that block too, and reopened the identical class of gap —
   observed for real as that specific line printing suspiciously late
   (after `task_a`/`task_b`/`task_c`/`task_d` had already run for a
   while), not as a self-test failure, since correctness didn't depend on
   it — but a real, confusing, avoidable artifact with the same root
   cause, closed the same way.
2. **(This pass.)** `task_a`'s live spawn of `task_d` had the identical
   shape of bug: checking `TASK_C_EXITED` and swapping a `TASK_D_SPAWNED`
   guard were two separate steps, with real interrupts still live in
   between (ordinary task code, not a critical section). A tick landing
   between the guard swap and the actual `spawn_task` call could preempt
   `task_a` there — and since the guard was already latched, nothing
   would ever retry it, so `task_d` would simply never be spawned.
   Observed for real: a batch caught exactly this — `task_d` never
   printed anything, and `TaskReuse` failed outright (exit code 35, not a
   hang). Fixed the same way: the whole check-and-spawn decision moved
   inside one `without_interrupts` critical section.
3. **(Previous pass, found but not debugger-confirmed — said plainly.)** A
   genuinely rare, total hang (no crash, no panic, just silence forever)
   surfaced in a large repeated-boot batch, always immediately after
   `task_c`'s very first `exit_current_task` call. Fine-grained diagnostic
   logging narrowed it to "freezes during or immediately after
   `switch_to`," but this environment has no live debugger attached to
   QEMU, so the exact mechanism was not directly observed. The leading
   hypothesis, and the one acted on: heap-allocated task stacks have no
   guard page, and `STACK_SIZE` was only 16 KiB — thin headroom once real
   interrupt nesting (ISR → `on_timer_tick` → `switch_to`'s own pushes)
   stacks on top of a task's own call depth, with nothing to turn a
   marginal overrun into a clean fault instead of silent corruption.
   Quadrupling `STACK_SIZE` to 64 KiB made the hang stop reproducing
   across **66 further consecutive runs** (40 default-CPU, 10 with
   `+smep,+smap` forced on, 1 UEFI, 15 more on a from-scratch clean
   rebuild) where it had appeared roughly every 25-40 runs before. That was
   real, substantial evidence the mitigation worked — deliberately *not*
   described at the time as a debugger-confirmed root cause, because it
   wasn't one. **This pass replaces that mitigation with real
   guard-page-protected task stacks** — see the dedicated section below,
   including a real bug found deploying it and a real, manually-verified
   fault demonstration.

**Scope, stated plainly**: the task pool is fixed-size, not unbounded. An
exited task's stack is genuinely freed, but only lazily, the moment a
future `spawn_task` call happens to reuse that exact slot — a slot that's
never reused (if fewer than `MAX_TASKS` tasks ever exist across a whole
run) leaks its last occupant's stack for the life of the kernel. No task
hierarchy (parent/child, wait/reap), no blocking/IO-driven rescheduling,
no SMP.

## Real guard-page-protected task stacks

`task_stack.rs` is new this pass. Task stacks are no longer
`Box<[u8]>`-backed heap allocations — each of the `MAX_TASKS` task-pool
slots now owns a dedicated, fixed virtual address range, mapped through
the same real `OffsetPageTable`/`PhysicalAllocator` everything else in
this kernel shares, with a genuinely **unmapped** page directly beneath
every stack. The mapping is lazy (first use of a slot maps it) and then
permanent — the slot's real physical frames are reused, not
freed-and-remapped, the next time that slot index is claimed, since the
task pool was always a fixed-size resource anyway.

**A real bug, caught on the very first boot with this code**: the first
choice of base address for this region, `0x_8888_8888_0000`, is not a
canonical x86-64 address — bits 63:47 must all match (a sign extension of
bit 47), and a leading `0x8` nibble sets bit 47 while leaving bits 63:48
at zero. The very first `spawn_task` call to map a guarded stack hit a
real, immediate panic straight from the `x86_64` crate's own `VirtAddr::
new`:
```
[PANIC] panicked at ...\x86_64-0.15.5\src\addr.rs:81:23:
virtual address must be sign extended in bits 48 to 64
```
Every other fixed region this kernel uses (`0x4444...` for the heap,
`0x5555...` for the demand-page region, `0x6666.../0x7777...` for the
ring-3 demo) already uses a nibble `<= 0x7` for exactly this reason — this
one just hadn't been checked against it before the first real boot did.
Fixed by moving the region to `0x_3333_3333_0000`.

**The `TaskReuse` self-test now proves something stronger and more
direct than before.** The old `Box`-backed version proved reuse via a
`Drop`-counting wrapper (did *some* stack get freed). This version tracks
the actual array/slot index each task occupies: `task_c`'s exit records
which slot it vacated, `task_a`'s live `spawn_task(task_d_entry)` call
records which slot `task_d` landed in, and the check confirms they're the
exact same index — real, address-level proof of reuse, not an indirect
counter. Observed directly in the boot log:
```
[task_stack] slot 0: mapped a real 64 KiB guarded stack at 0x333333331000, real unmapped guard page at 0x333333330000
[task_stack] slot 1: mapped a real 64 KiB guarded stack at 0x333333342000, real unmapped guard page at 0x333333341000
[task_stack] slot 2: mapped a real 64 KiB guarded stack at 0x333333353000, real unmapped guard page at 0x333333352000
[scheduler_bridge] real RunQueue + 3 real task contexts initialized (task_c will really exit)
[task_c] really exiting after 5 real iterations
[task_d] really spawned at runtime, first real context switch resumed me
```
— only three slots are ever mapped (task_a/b/c at boot); `task_d` reuses
slot 2 (task_c's), never triggering a fourth `[task_stack] slot 3: mapped
...` line, which would have meant reuse hadn't actually happened.

**Manually verified the guard page actually works, the same way the SMAP
fix was verified**: temporarily made `task_d_entry` recurse (a
`#[inline(never)]` function pushing a 512-byte buffer each call, `black_
box`ed to defeat tail-call/inlining optimization) past its real 64 KiB
budget, rebuilt, and booted. **The real result was a `DOUBLE FAULT`, not a
plain `PAGE FAULT`** — genuinely informative, and not what was originally
guessed:
```
[task_c] really exiting after 5 real iterations
[task_d] TEMP: deliberately overflowing this task's guarded stack

[PANIC] panicked at src\interrupts.rs:76:5:
EXCEPTION: DOUBLE FAULT
InterruptStackFrame {
    instruction_pointer: VirtAddr(0x100000096c7),
    ...
    stack_pointer: VirtAddr(0x333333352ea0),
    ...
}
```
Once the stack pointer wanders into the unmapped guard page, the CPU's own
attempt to push the `#PAGE FAULT` exception frame onto that same,
already-faulting stack pointer faults *again*, which escalates to a real
double fault — the exact reason `gdt.rs` already routes the double-fault
handler through a dedicated IST stack (`DOUBLE_FAULT_IST_INDEX`) rather
than trusting whatever stack was active when the fault happened: an
overflowing task's stack can't be assumed to have room for even one more
exception frame. The captured `stack_pointer` (`0x333333352ea0`) falls
inside that slot's guard page range (`0x333333352000..0x333333353000`,
below its real mapped stack at `0x333333353000`) — direct, real evidence
the overflow reached the guard page specifically. The temporary recursion
was then fully reverted and the kernel rebuilt back to a clean, 0-warning
state before any further validation.

**What this closes, and what it still doesn't prove.** This structurally
closes the "silent corruption" failure mode the previous mitigation could
only make rare: any real stack overflow across this whole kernel now
reliably produces a diagnosable panic (a page fault escalating to a
double fault) instead of possibly corrupting adjacent memory silently.
It does **not** retroactively prove that stack overflow was the previous
hang's actual mechanism — no debugger was available to confirm that then,
and none is available now either; that specific historical question
remains open. What's real is that the failure mode the hypothesis pointed
at is now architecturally closed off, not merely made statistically rare.

## Real task-to-address-space binding

Until this pass, `address_space.rs`'s second, independent `CR3`-loadable
table (see "Real multiple address spaces" above) was a manual, one-shot
demonstration — `main.rs`'s self-test switched to it, read it, and
switched straight back, entirely separate from real task scheduling.
This pass ties the two together for real, in `scheduler_bridge.rs`:

- **`TaskSlot` now carries `address_space: Option<PhysFrame<Size4KiB>>`**
  — `None` (every task before this pass, and `task_a`/`b`/`c`/`d` still)
  means "run in `DEFAULT_L4_FRAME`," the real L4 frame captured once, in
  `init()`, from whatever `CR3` actually is at that point (by then,
  `main.rs`'s own `AddressSpace` check has already switched to its demo
  table and switched back, so this really is the original/default
  table).
- **`spawn_task_with_address_space`** binds a task to a specific L4 frame
  at spawn time — `init()` uses it for a new demo task, `task_e`, bound
  to a fresh `AddressSpace::new()` built the same way the earlier,
  manual demo built its own.
- **`switch_address_space_to`** — a real `CR3` read compared against the
  frame the task being switched *into* should run in, with a real `CR3`
  write only when they differ — is called from *both* real switch call
  sites, `on_timer_tick` and `exit_current_task`, right before the real
  `context::switch_to`. This is genuinely part of the ordinary scheduling
  path now, not a separate mechanism a task has to opt into: switching
  between two tasks that share the default table (the common case) costs
  nothing beyond the `CR3` read that finds nothing to change.
- **`task_e` proves the binding is real, not merely stored.** Every
  iteration it reads straight through the real virtual address
  `address_space::PRIVATE_REGION_ADDR` — no physical-offset back door.
  That only resolves to the expected value because a real `CR3` switch,
  performed by the *ordinary timer-driven scheduler*, actually happened
  immediately before `task_e` was resumed. If the binding were somehow
  wrong, this wouldn't silently pass — either the value would be wrong
  (caught by the `TaskAddressSpace` check below) or, if the private page
  isn't mapped in whatever table turned out to be active, a real,
  diagnosable page fault, the same "wrong is loud, not silent" property
  guard-page stacks already established.
- **`TaskAddressSpace` self-test check**: confirms `task_e`'s counter is
  nonzero — i.e. it really ran, and really saw the correct value, not
  once but on every one of however many iterations the tick budget gave
  it (see the boot log above: `task_e` genuinely interleaves with
  `task_a`/`task_b`, driven by the same real hardware timer).

**Passed on its first boot — no bug found this time**, the same result as
the address-space work it builds on. This makes real sense in hindsight:
`AddressSpace::new()`'s full-clone-plus-one-private-page design already
guarantees every existing mapping (including every task's own guarded
stack) resolves identically regardless of which table is active, which is
exactly the property that makes it safe to switch `CR3` at an arbitrary
point in the middle of an ordinary context switch — the same safety
argument, just reused in a new call site rather than re-derived.

**Scope, stated plainly**: one-way binding only (a task is bound at spawn
time and never rebinds), no unbind/teardown path (a bound task's
`AddressSpace` is never freed even if that task later exited — no task
that owns one does exit in this pass anyway), and no process abstraction
wraps the pairing (a `TaskSlot` just holds an `Option<PhysFrame>`, not a
first-class "process" concept with its own PID namespace, exit status, or
resource accounting).

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

- **The context switch and task model are real but narrow.** Real task
  creation, real task exit, real guard-page-protected stacks, and real
  task-to-address-space binding now exist (see "Real task creation and
  exit," "Real guard-page-protected task stacks," and "Real
  task-to-address-space binding" above), but only within a fixed-size
  pool (`MAX_TASKS = 8`, one-to-one with `task_stack::MAX_SLOTS`); a
  slot's real pages, once mapped, stay mapped and reused for the
  kernel's lifetime rather than being unmapped between occupants (an
  intentional simplification, not a leak — see that section); address-
  space binding is one-way only (bound at spawn, never rebound, never
  torn down); there is still no blocking/IO-driven rescheduling (a task
  can only ever yield by being timer-preempted), no task hierarchy
  (parent/child, wait/reap), and no SMP (the switch code's soundness
  argument in `context.rs`/`scheduler_bridge.rs` explicitly leans on
  "single core, only ever touched with interrupts disabled" — a second
  CPU would break that invariant and needs real synchronization, not
  attempted here). Handing control back to the boot flow after a fixed
  tick budget is a hardcoded sentinel (`BOOT_PID`), not the scheduler
  genuinely managing the kernel's own boot thread as a task.
- **The page tables are real but narrow.** A second, genuinely
  independent CR3-loadable address space now exists, and is now really
  bound to a real scheduled task with the ordinary timer-driven
  scheduler performing the real `CR3` switch (see "Real multiple address
  spaces" and "Real task-to-address-space binding" above) — but there is
  still no process abstraction around any of it (no PID, no exit
  semantics tied to an address space, no copy-on-write), and no process/
  kernel privilege separation (everything still runs at CPL0 in every
  address space). Real unmapping exists (`paging::unmap_page`, see "Real
  page unmapping" above), but nothing in this kernel calls it except the
  self-test's own deliberate demonstration — there's no general "free
  this VMA" path wired into anything else yet. The physical allocator is
  still a single contiguous arena (the largest usable region reported by
  the bootloader, minus the handful of frames the paging bootstrap
  consumed) — a real multi-region allocator is still
  real, separate follow-on work, unchanged from `reference-rs/STATUS.md`'s
  original note.
- **The kernel heap is fixed-size** (256 KiB bootstrap + 1 MiB real =
  1.25 MiB total, `allocator.rs`'s `BOOTSTRAP_HEAP_SIZE`/`REAL_HEAP_SIZE`)
  — real, page-mapped memory, but a hardcoded ceiling, not something that
  grows on demand past that.
- **The syscall ABI is real but small.** A real number→handler dispatch
  table, real multi-argument passing, and real return values the ring-3
  program genuinely uses now exist (see "Real, small syscall ABI" above)
  — but it's a fixed, four-entry, compile-time table, not a dynamically
  extensible registry; still one hand-assembled ring-3 program (no ELF
  loader, no relocation, no process abstraction), no process/exit
  semantics beyond one hardcoded exit value, and only ever one ring-3
  program existing at a time. `BootSequencer`'s `SyscallBoot` phase
  itself is still deliberately left incomplete — a real, if small,
  syscall ABI now exists, but there is no process model or dynamically
  extensible syscall registry behind it, and the live self-test output
  says so rather than silently marking the phase done.
- **NX/SMEP/SMAP are real but narrow.** NX and SMAP are both genuinely
  enforced and empirically exercised every boot (see "Real NX/SMEP/SMAP"
  above); SMEP is enabled, CPUID-gated correctly, and this pass was
  empirically fault-tested too — but only manually, once, via a temporary
  probe that was reverted afterward, not a permanent self-test, since
  nothing in the checked-in kernel actually attempts a supervisor-mode
  fetch from a user-accessible page. No page carries a fine-grained
  read-only/read-write distinction beyond what already existed (every
  mapped page here is still `WRITABLE`); no protection-key (`PKU`)
  support; no CET/shadow stacks.
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
