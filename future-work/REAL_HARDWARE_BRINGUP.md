# Moving from QEMU-only to real, physical x86-64 hardware

**Status: design analysis only. This kernel has never run on anything
but QEMU (TCG/emulated), and this environment has no physical machine to
boot it on — this document cannot itself close that gap, only describe
what closing it would require.**

## Why QEMU passing isn't the same claim as hardware passing

`kernel-x86_64/STATUS.md` is careful to say "real (emulated) hardware"
throughout, specifically because QEMU's TCG mode is a real, high-fidelity
emulator, not a rubber stamp — but every real bring-up bug this session
found (the GDT segment-register bug, the SMAP/SMEP enforcement gaps, the
guard-page overflow producing a double fault) was still found *inside
QEMU's model* of x86-64, which is necessarily a simplification. Concrete
places where real hardware would diverge:

- **CPU discovery**: QEMU's default CPU model and its `+smep`/`+smap`
  flags are convenient knobs; real hardware has a fixed, non-negotiable
  feature set discovered once via real `CPUID`, and a real machine might
  lack NX/SMEP/SMAP entirely (old hardware) or have features this kernel
  has never seen (e.g. 5-level paging/LA57, which would silently change
  what a "canonical address" even means — see the guard-page pass's
  non-canonical-address bug for a preview of the failure mode).
- **Interrupt controllers**: this kernel programs the legacy 8259 PIC and
  PIT directly. Real modern x86-64 hardware boots with those present for
  backward compatibility, but a real OS is expected to move to the
  **APIC**/**I/O APIC** (and `HPET` or the invariant TSC for timing) —
  the 8259/PIT path this kernel uses today works on real hardware, but
  is legacy-mode operation, not what a real OS would ship long-term.
- **Firmware quirks**: QEMU's bundled EDK2/OVMF is a real, standards-
  compliant UEFI implementation, but real vendor firmware has real,
  documented quirks (ACPI table bugs, non-standard memory map holes,
  differing behavior around `ExitBootServices`) that only show up on
  physical hardware from a specific vendor.
- **No `isa-debug-exit` device.** This kernel's entire self-test
  reporting mechanism — a scriptable process exit code — depends on a
  QEMU-only debug device. Real hardware has no equivalent; a real
  bring-up needs a different signal (a fixed serial handshake sequence a
  host script watches for, a GPIO toggle, or just requiring a human to
  read the serial console and judge pass/fail).
- **No clean shutdown.** `-no-reboot` plus `isa-debug-exit` gives a clean
  process exit in QEMU. Real hardware has no equivalent to "exit the
  emulator" — a real bring-up needs to define what happens after the
  self-test completes (halt forever via `hlt` loop, which this kernel
  already does as a fallback path in places, or a real ACPI shutdown via
  `\_S5` — not implemented anywhere here).
- **Real DMA and cache coherency.** Nothing in this kernel currently
  performs DMA or reasons about cache coherency at all (there are no
  disk/network drivers yet) — real hardware bring-up is usually where
  these first become load-bearing, and QEMU's emulated devices don't
  exercise the same real bus/cache-snooping behavior real silicon does.
- **Real timing.** `pit::set_frequency` computes a divisor via
  `uosc_core::timer::pit_divisor` and programs real port I/O — this
  genuinely programs real PIT hardware identically whether emulated or
  real, so this specific piece is a low-risk carry-over. Calibrating
  against a real TSC frequency (which varies per physical CPU and isn't
  reliably discoverable without either `CPUID` leaf 0x15 support or a
  real calibration loop against a known-good timer) is new, real work
  with no QEMU equivalent to have already caught bugs in.

## What a real, phased bring-up would look like

1. **Pick a target board/VM-adjacent reference platform** and obtain real
   documentation for its specific chipset/firmware (real hardware bring-up
   is never generic — it's always "this specific board").
2. **Serial-only bring-up**: get `serial.rs`'s UART driver working against
   the board's real COM port wiring (baud/IRQ routing can differ from the
   16550-compatible default this kernel assumes), and get one string out
   before anything else is trusted.
3. **A real pass/fail signal design**, since `isa-debug-exit` doesn't
   exist — likely a fixed serial sentinel string plus a defined "then halt
   forever" contract a host-side script or a human watches for.
4. **Real memory map validation**: dump and manually cross-check the
   firmware-reported memory map against the board's actual documented RAM
   layout before trusting `PhysicalAllocator::new`'s inputs.
5. **Re-run every existing self-test check, one at a time, on hardware**,
   expecting some of them to behave differently or not at all (SMEP/SMAP/
   NX support is a real unknown until `CPUID` is actually read on that
   silicon) — and expect *new* real bugs QEMU's model never had a reason
   to produce.
6. **Only after that**: real APIC/HPET migration, real ACPI parsing, real
   shutdown path — each a substantial, separate piece of work in its own
   right, not a checkbox after "it boots."

## Open questions nobody has answered yet

- What's the actual target board? "Real hardware" isn't a single
  destination — a bring-up plan for a specific mini-PC/dev board looks
  very different from one for a specific server platform.
- What's the acceptable-risk story for bricking/hanging real hardware
  during bring-up (serial-console-only recovery vs. needing physical
  reflashing access) — this matters for how cautious each experimental
  step needs to be, in a way that simply doesn't apply to a QEMU process
  that can be killed and relaunched for free.
- Does this kernel need a serial bootloader/JTAG story for iterating
  quickly, or is the plan to reflash and reboot the physical board for
  every single change during early bring-up (much slower than this
  session's QEMU batches, which could reproduce a ~1-in-30 intermittent
  bug across dozens of runs in minutes)?
