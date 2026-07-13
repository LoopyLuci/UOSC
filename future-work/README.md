# Future work — design documents, not implementations

Every file in this directory is a **design document**: real technical
analysis, grounded in this repository's actual code (`kernel-x86_64/`,
`reference-rs/`), of what a real implementation would require. None of
them is, or claims to be, working software. This directory exists
specifically so that distinction never gets blurry the way the original
`docs/` tree's aspirational claims did (see the root
[`README.md`](../README.md)'s opening section) — a design document is
useful and honest; a design document mislabeled as a shipped feature is
not.

Each doc follows the same shape: what problem it solves, why it's
genuinely hard (not just time-consuming), what in *this* codebase would
have to change, a phased real plan, and the open questions nobody has
answered yet. None of them include a "this pass" boot log, a run count,
or a PASS/FAIL line, because none of them have been run.

| Doc | What it scopes |
|---|---|
| [`RISCV_AARCH64_PORTS.md`](RISCV_AARCH64_PORTS.md) | Porting `kernel-x86_64` to RISC-V and/or AArch64 targets |
| [`REAL_HARDWARE_BRINGUP.md`](REAL_HARDWARE_BRINGUP.md) | Moving from QEMU-only to real, physical x86-64 hardware |
| [`POST_QUANTUM_CRYPTO.md`](POST_QUANTUM_CRYPTO.md) | Adding post-quantum cryptography (there is currently *no* cryptography anywhere in this kernel) |
| [`SECURE_BOOT.md`](SECURE_BOOT.md) | Real UEFI Secure Boot / measured boot |
| [`LIVE_PATCHING.md`](LIVE_PATCHING.md) | Runtime kernel code patching without a reboot |
| [`FEDERATION_PROTOCOL.md`](FEDERATION_PROTOCOL.md) | A protocol for multiple UOSC instances to coordinate — currently not even a stated goal, let alone a design |

## Why these six specifically

These were named, in an earlier pass of this same effort, as explicitly
out of scope for honest, in-session implementation — not because they're
uninteresting, but because each one fails a different way if attempted
quickly:

- **RISC-V/AArch64 ports** and **real hardware bring-up** fail by needing
  things this environment doesn't have (real target toolchains and, for
  hardware, physical boards) rather than more time at a keyboard.
- **Post-quantum crypto** and **secure boot** fail by being exactly the
  kind of security-critical work where a fast, unreviewed implementation
  is worse than no implementation — a subtly wrong constant-time
  comparison or a badly managed trust anchor is a real vulnerability, not
  a bug to fix later.
- **Live patching** fails by depending on infrastructure (a real dynamic
  loader/relocator) that doesn't exist yet in this kernel at all.
- **A federation protocol** fails by not yet having a stated goal to
  design *against* — "add federation" isn't a spec, it's a placeholder
  for one.

Each doc below tries to turn "not achievable quickly" into "here is
exactly what achieving it for real would take."
