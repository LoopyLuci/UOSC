# Real UEFI Secure Boot / measured boot

**Status: design analysis only. This kernel's UEFI boot path currently
verifies nothing about its own image before running — anyone who can
write to the disk image can run arbitrary code as this kernel.**

## What already exists in this environment, worth noting for real

While investigating this kernel's real UEFI boot path earlier this
session, the bundled `qemu` scoop package was found to also ship
`edk2-x86_64-secure-code.fd` / `edk2-i386-secure-code.fd` alongside the
non-secure firmware this kernel currently boots with
(`edk2-x86_64-code.fd`) — a real, Secure-Boot-capable OVMF build already
sitting in this environment, unused. That's a genuine, concrete starting
point for a future pass: swapping the firmware image is real and cheap to
try; everything after that (below) is the actual work.

## What "secure boot" actually requires, layer by layer

1. **A real key hierarchy**: Platform Key (PK) → Key Exchange Key (KEK)
   → `db`/`dbx` (allowed/forbidden signature databases). UEFI Secure Boot
   is fundamentally "the firmware refuses to execute any boot loader/
   kernel image whose signature doesn't chain to a key enrolled in `db`."
   None of this exists for this kernel today — there is no key, no
   certificate, no enrollment step.
2. **A real signing pipeline.** The kernel ELF this session builds via
   `cargo +nightly build` and packages via `kernel-x86_64-builder` would
   need to be signed (e.g. with `sbsign` or equivalent) as a real, distinct
   build step, with the private key handled with real operational care
   (not committed to the repo, not sitting in plaintext on the build
   machine indefinitely).
3. **Either a real bootloader that chains trust, or direct kernel
   signing.** Production Secure Boot deployments typically go through a
   Microsoft-signed `shim` (so the OS's own key doesn't need to be
   pre-enrolled in every OEM's firmware `db`), which then verifies a
   second-stage loader (e.g. GRUB), which then verifies the kernel. This
   kernel's actual boot path (`bootloader_api`, no GRUB/shim) is much
   simpler and could plausibly skip straight to "the firmware verifies
   the kernel image's signature directly" — but that requires the test/
   deployment firmware's `db` to actually have this project's key
   enrolled, which for the QEMU OVMF vars store means writing a real
   enrollment step against a real (writable) vars file rather than the
   throwaway copy this session already makes for ordinary UEFI boots.
4. **A real decision about scope**: Secure Boot alone only proves "the
   firmware ran the image I signed" — it says nothing about what that
   image does *after* it starts running. Pairing it with **measured
   boot** (extending TPM PCRs with a hash of each stage, so a remote
   party can *attest* to what actually ran, not just trust that the local
   firmware enforced a signature check) is a materially different and
   larger scope — this kernel has no TPM driver of any kind today, and
   QEMU's TPM emulation (`swtpm`) is a separate, additional piece of
   environment setup nothing in this repo currently uses.
5. **A real revocation story.** `dbx` (the forbidden-signatures database)
   exists because keys get compromised or images get recalled — a secure
   boot design that never considers "how do we revoke a previously-signed
   kernel image" is incomplete for anything beyond a personal demo.

## Why this wasn't attempted in-session

Every step above involves handling cryptographic trust material (private
signing keys, PK/KEK enrollment) where a rushed, unreviewed
implementation is a real liability, not a shortcut — the same reasoning
[`POST_QUANTUM_CRYPTO.md`](POST_QUANTUM_CRYPTO.md) lays out for algorithm
implementation applies here to key handling and enrollment procedure.
There's also a real dependency: any signature scheme chosen here (RSA/
ECDSA today, or a PQ scheme per that doc) needs the same "vetted
implementation, not hand-rolled" discipline before it's trustworthy.

## A real, phased plan (not attempted here)

1. **Boot with the already-present secure firmware image**
   (`edk2-x86_64-secure-code.fd`) in Setup Mode first, with Secure Boot
   left *disabled*, just to confirm this kernel's existing UEFI path still
   boots unmodified against the secure-capable firmware build (a real,
   low-risk first check that hasn't been done yet).
2. **Generate a real test PK/KEK/db key hierarchy** (self-signed, clearly
   labeled test-only, never used for anything but this project's own QEMU
   vars store) and enroll it into a writable copy of the vars store via
   the firmware's Setup Mode UI or a real enrollment tool, not by hand-
   editing the vars file's binary format.
3. **Sign a real build of the kernel image** and confirm the firmware
   actually refuses an *unsigned* rebuild (the negative test matters as
   much as the positive one — same "prove the mechanism actually rejects
   what it should" discipline as this session's SMAP/SMEP fault
   verification).
4. **Only then** consider measured boot / TPM attestation as a distinct,
   larger follow-on, and a real revocation drill (sign a new key,
   populate `dbx` with the old one, confirm the old signed image is now
   rejected).

## Open questions nobody has answered yet

- Is the goal "boots on real hardware with Secure Boot enabled" (which
  drags in the shim/Microsoft-signing question for anything meant to run
  on unmodified consumer firmware) or "boots under this project's own
  enrolled test keys in QEMU/a controlled lab environment" — these are
  very different scopes with very different real-world trust
  implications.
- What's the actual key custody plan even for a *test* hierarchy — where
  does the private key live between signing operations, and who/what can
  invoke a signing operation?
- Does a future syscall ABI or IPC mechanism (see the achievable-scope
  work tracked outside this directory) end up needing the *same* key
  material for a different purpose (e.g. verifying loaded user programs),
  in which case the key hierarchy design here should be planned jointly
  with that work rather than in isolation?
