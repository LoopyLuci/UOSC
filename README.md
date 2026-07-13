# UOSC: Universal Operating System Kernel (Layer 1)

**Microkernel foundation for the Omnisystem three-layer architecture.**

**Status, honestly**: this directory contains two different things, and
mixing them up is exactly the problem this README used to have.

1. **`kernel/*.ti`, `drivers/*.ti`, `proofs/*.ax`** — a specification
   written in the Titan language and a hand-written "Axiom" proof
   language. Titan has no compiler backend that produces a running
   binary, and the `.ax` proof files are prose that nothing checks. This
   is a design document, not working software, however complete it reads.
2. **`reference-rs/`, `proofs-lean4/`, `kernel-x86_64/`** — a real,
   independently-verified implementation and proof of a *subset* of that
   specification: working Rust, a real Lean 4 toolchain checking real
   proofs, and a real bootable kernel that has actually run in QEMU. This
   is where the actual verified status of UOSC lives.

An earlier version of this file claimed "✅ PRODUCTION READY," "zero
unsafe code," "10 formally proven theorems," and multi-hypervisor support,
none of which was true of anything that could actually be built or run —
there was no `Cargo.toml` at this path, no compiler for `.ti`, and no tool
checking the `.ax` proofs. That's the exact failure mode the `reference-rs`
effort exists to replace with something checkable. This file now describes
what's real.

---

## What's actually verified, and how to check it yourself

| Directory | What it is | Real, run-it-yourself evidence |
|---|---|---|
| [`reference-rs/`](reference-rs/) | Rust port of the portable logic in 8 of the ~11 `.ti` files (capability, memory, ipc, scheduler, sanctum, boot, timer, console) | `cargo test` → **64 passed, 0 failed**; `cargo clippy` → 0 warnings. See [`reference-rs/STATUS.md`](reference-rs/STATUS.md) for the 10 real bugs found in the Titan spec while porting it. |
| [`proofs-lean4/`](proofs-lean4/) | Real Lean 4 re-hosting of `proofs/kernel_security.ax`'s 10 theorems | `lean <file>.lean` on all six `.lean` files → all exit 0. **8 of 10** theorems have a real machine-checked result (one of them — Property 7 — turned out to be *false as originally stated*; see [`proofs-lean4/README.md`](proofs-lean4/README.md)). |
| [`kernel-x86_64/`](kernel-x86_64/) | A real bootable kernel binary built on `reference-rs`, using `bootloader_api` + a real GDT/IDT/PIC/PIT, real hardware page tables, a real context switch between two independently-running kernel tasks, and a real ring-3 ↔ ring-0 privilege transition with a real return path | Boots in QEMU (BIOS **and** UEFI, real bundled EDK2 firmware) and runs a 13-check self-test against real hardware interrupts, a real physical memory map, real page-table-mapped heap memory, a real deliberately-triggered and demand-paged page fault, a real hardware-timer-driven context switch, and real CPL3 code making 3 real round-trip syscalls plus 1 real exit trap via `int 0x80`. **13/13 checks pass**, reproduced across multiple independent runs on both boot paths. See [`kernel-x86_64/STATUS.md`](kernel-x86_64/STATUS.md) for exact commands, verbatim boot output, and the real bugs found and fixed along the way (a GDT segment-register bug, a circular dependency between the kernel heap and the physical allocator — which turned out to also expose a real, previously-invisible bug in `reference-rs`'s allocator, see its own `STATUS.md` — a real link failure from a missing RIP-relative reference, and a real silent-hang bug from interrupts staying disabled after a hand-rolled privilege transition). |

Nothing above is described as "complete" in the sense the old table below
used the word — each status file says explicitly what is and isn't
covered, and why.

## What's still specification, not implementation

- `kernel/hypercall.ti` and `drivers/{block,input,network,graphics}.ti` —
  not ported at all. They're close to 100% direct hardware/hypervisor
  dispatch (`asm!("vmcall")`, raw MMIO) with no portable logic to get
  right or wrong in a `no_std` library; see `reference-rs/STATUS.md` for
  the reasoning.
- Properties 4 (`ipc_message_atomicity`) and 6 (`interrupt_handler_safety`)
  in `kernel_security.ax` — need a real operational semantics of
  hardware/concurrency that doesn't exist anywhere in this codebase.
- SMP, a general syscall ABI, a process model, a filesystem, a network
  stack, RISC-V/AArch64 ports, post-quantum crypto, secure boot, live
  patching, a federation protocol — none of this exists yet in any form,
  real or aspirational-but-labeled-as-such. Each is real, substantial,
  separate engineering, not a checkbox.
  (Context switching between two independently-scheduled kernel tasks,
  real hardware page tables backing a real kernel heap and a real
  demand-paged page fault, and a real ring-3 ↔ ring-0 privilege
  transition — with a real return to ring 3 between traps, not just a
  one-shot abandon — via real `int 0x80` traps, *do* now exist for real —
  see the table above — but narrowly: exactly two static tasks, one
  address space, no process isolation, no unmapping, a fixed-size heap,
  one hand-assembled ring-3 program with a fixed, hardcoded sequence of
  traps rather than a general syscall ABI. General task creation/exit,
  multiple address spaces, a real syscall ABI, and SMP are still on this
  list.)

## Directory structure

```
UOSC/
├── README.md                    # This file
├── kernel/                      # Titan-language kernel specification (unverified spec)
│   ├── boot.ti / memory.ti / scheduler.ti / ipc.ti / sanctum.ti / hypercall.ti
├── drivers/                     # Titan-language driver specification (unverified spec)
│   ├── console.ti / timer.ti / block_drivers.ti / input_drivers.ti / network_drivers.ti / graphics_drivers.hlx
├── proofs/                      # Hand-written "Axiom" proof text (unchecked by any tool)
│   ├── kernel_security.ax / UNIVERSAL_OS_CORE.ax / additional/
├── reference-rs/                # REAL: Rust port + 64 passing tests — start here for working code
│   └── STATUS.md
├── proofs-lean4/                # REAL: Lean 4 proofs, 8/10 theorems, real toolchain
│   └── README.md
├── kernel-x86_64/                # REAL: bootable kernel binary, boots in QEMU (BIOS+UEFI), 13/13 self-test
│   └── STATUS.md
├── kernel-x86_64-builder/        # Host-side tool that packages the kernel into a bootable disk image
├── docs/                        # Original design documentation (describes the specification, not verified status)
└── CONTRIBUTING.md
```

## Quick start (the real, working path)

```bash
# Run the real Rust test suite
cd reference-rs && cargo test

# Check the real Lean 4 proofs
cd proofs-lean4 && lean Capability.lean && lean Memory.lean && lean Scheduler.lean \
  && lean SchedulerN.lean && lean Boot.lean && lean PageFault.lean

# Build and boot the real kernel in QEMU
cd kernel-x86_64
cargo +nightly build --release --target x86_64-unknown-none \
  -Zbuild-std=core,alloc,compiler_builtins -Zbuild-std-features=compiler-builtins-mem
cd ../kernel-x86_64-builder
cargo +nightly run -- ../kernel-x86_64/target/x86_64-unknown-none/release/uosc-kernel-x86_64 ../kernel-x86_64/target/images
qemu-system-x86_64 -drive format=raw,file=../kernel-x86_64/target/images/uosc-bios.img \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 -serial stdio -display none -no-reboot
```

## Relationship to Omnisystem

- UOSC = Layer 1 (this directory) — the only layer with a real, booting
  binary right now.
- Omnisystem = Layer 2 (OS services) — see [`../README.md`](../README.md).
- BonsaiEcosystem = Layer 3 (applications) — see
  [`../modules/BonsaiEcosystem/README.md`](../modules/BonsaiEcosystem/README.md).

Layer 2/3 documentation describing integration with UOSC predates this
pass and should be read with the same caveat this file used to need: check
`reference-rs/`, `proofs-lean4/`, and `kernel-x86_64/`'s own status files
for what's actually real before trusting a status claim elsewhere in the
tree.
