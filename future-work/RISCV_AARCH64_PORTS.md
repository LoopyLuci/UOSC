# Porting `kernel-x86_64` to RISC-V and/or AArch64

**Status: design analysis only. No RISC-V or AArch64 code exists anywhere
in this repository.**

## Why this is a port, not a recompile

`kernel-x86_64`'s name is accurate: every non-trivial module is written
directly against x86-64 hardware, not against a portable abstraction with
an x86-64 backend. Concretely, per module:

| Module | What's x86-64-specific | What a RISC-V/AArch64 equivalent needs |
|---|---|---|
| `context.rs` | `switch_to`/`task_trampoline` are hand-written `naked_asm!` using the System V AMD64 calling convention's callee-saved registers (`rbp`,`rbx`,`r12`-`r15`) and `push`/`pop`/`ret` | RISC-V: save `ra`,`sp`,`s0`-`s11` and use a `jr`-based return; AArch64: save `x19`-`x30`,`sp` and use `ret`. The *design* (a fake initial stack frame that "returns" into a trampoline) carries over; the assembly does not. |
| `paging.rs`/`address_space.rs` | `x86_64` crate's `PageTable`/`PageTableFlags`/`OffsetPageTable`/`Cr3` are x86-64-only types with x86-64 PTE bit layouts | RISC-V Sv39/Sv48 PTEs (`V`,`R`,`W`,`X`,`U`,`G`,`A`,`D` bits, different positions) and `satp` register; AArch64 has a different translation table format entirely (`TTBR0`/`TTBR1`, block/table/page descriptors, `MAIR_EL1` memory attributes instead of PTE flags). No crate in this dependency tree models either. |
| `cpu_features.rs` | `EFER.NXE`/`CR4.SMEP`/`CR4.SMAP`, gated on x86 `CPUID` leaves | RISC-V has no CPUID equivalent at all — feature discovery is via the `misa` CSR and SBI calls, and there is no direct RISC-V analog of SMEP/SMAP (some of this is covered by page-table `U`-bit semantics instead). AArch64 has `ID_AA64*` system registers and its own W^X story (`PXN`/`UXN`/`XN` bits), not a drop-in mapping from this module's API. |
| `interrupts.rs`/`gdt.rs`/`pit.rs` | Real IDT/GDT/PIC/PIT — all literally x86 hardware | RISC-V: CLINT (timer/software interrupts) + PLIC (external interrupts), trap vector via `mtvec`, no descriptor tables at all. AArch64: GICv2/v3 distributor+redistributor, exception vector table, generic timer (`CNTP_*_EL0`). Neither has a GDT-shaped concept; ring 3 vs ring 0 becomes EL0/EL1 (AArch64) or U-mode/S-mode (RISC-V), with different trap/return instructions (`eret` vs `sret`/`mret`). |
| `syscall.rs` | `int 0x80` is an x86 software-interrupt instruction | RISC-V: `ecall`; AArch64: `svc`. Both trap to a different vector than x86, with different register conventions for the "syscall number." |
| `task_stack.rs` | Reuses `x86_64::structures::paging::PageTableFlags` | Same PTE-format problem as `paging.rs` — the *architecture* (one guard page per slot, mapped lazily) ports; the flag bits and page-table walk code do not. |
| `bootloader_api`/`kernel-x86_64-builder` | The `bootloader`/`bootloader_api` crates are x86-64-only; they don't target RISC-V or AArch64 at all | A RISC-V port would boot via OpenSBI + a flat kernel image or a small custom loader reading a device tree blob; an AArch64 port would boot via U-Boot or UEFI (QEMU's `virt` machine can do either) — either way, a different boot protocol, different kernel entry contract (register conventions for what the firmware hands off — e.g. RISC-V SBI hands `hartid` in `a0` and a device-tree pointer in `a1`), and no `BootInfo`/`MemoryRegion` struct from `bootloader_api` to parse memory maps from — that comes from a device tree instead. |

## What genuinely carries over

Everything in `reference-rs` (`uosc_core::capability`/`memory`/`ipc`/
`sanctum`/`scheduler`/`boot`) is portable Rust with no architecture
dependency — this was true before `kernel-x86_64` existed and remains
true. The *shape* of several `kernel-x86_64` mechanisms also carries over
even though the code doesn't: a fake initial stack frame for new tasks, a
demand-paged region with a scoped fault handler, a guard page below every
task stack, cloning a top-level page table to build a new address space.
Those are architectural decisions, not x86-64 tricks — a port re-expresses
each of them in the target ISA rather than inventing a new design.

## A real, phased plan (not attempted here)

1. **Pick one target first** — RISC-V64 (`qemu-system-riscv64 -M virt`)
   is the more approachable of the two: simpler privilege model (M/S/U
   modes vs AArch64's four exception levels with more state), a simpler
   PLIC/CLINT interrupt story than GICv3, and an actively maintained
   `riscv` crate ecosystem (though nothing as complete as the `x86_64`
   crate used here). AArch64 has a larger and more fragmented "which UEFI/
   PSCI/GIC version" surface even under QEMU.
2. **Boot to "hello, serial"** — get a flat kernel image (or a minimal
   ELF) loaded by OpenSBI, output one string over a real UART (QEMU
   `virt`'s NS16550-compatible UART is close enough to this kernel's
   existing `serial.rs` that the driver itself may need only address
   changes), and exit QEMU cleanly. No paging, no interrupts yet — this
   is the RISC-V/AArch64 equivalent of this repo's very first commit to
   `kernel-x86_64`, not a small step.
3. **Real trap handling** — one trap vector, one timer interrupt (CLINT's
   `mtimecmp`/AArch64's generic timer), enough to prove a tick actually
   fires and is acknowledged, mirroring `interrupts.rs`'s current PIT/PIC
   milestone.
4. **Real paging** — Sv39 (RISC-V) or a 4KB-granule AArch64 translation
   table, a real heap mapped through it, mirroring `allocator.rs`'s
   two-phase bootstrap (which itself depends on this step, not the other
   way around).
5. **Real context switching, then the rest of the checklist this kernel
   already has for x86-64** — task creation/exit, guard pages, a second
   address space, a user/kernel privilege transition — each one, again, a
   real re-implementation against the new ISA's actual mechanisms, not a
   recompile of the existing modules.

## Open questions nobody has answered yet

- Does `uosc_core` (the `reference-rs` crate) need any changes at all, or
  does it stay genuinely architecture-neutral through a real second
  `kernel-*` crate consuming it? (Current best guess: no changes needed —
  nothing in `reference-rs` touches a register or a page table directly.)
- Is a shared `kernel-common` crate worth factoring out of `kernel-x86_64`
  now, before a second architecture exists, or would that be premature
  abstraction for a codebase that has exactly one real target today?
- What's the actual QEMU invocation and firmware story for each target in
  *this specific* environment (scoop's bundled `qemu-system-riscv64.exe`/
  `qemu-system-aarch64.exe`, whatever OpenSBI/U-Boot binaries would need
  to be sourced) — this needs to be checked for real before any of the
  above is attempted, the same way this session already had to discover
  QEMU's bundled EDK2 firmware for the existing UEFI boot path.
