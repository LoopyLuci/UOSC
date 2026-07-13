# Adding post-quantum cryptography

**Status: design analysis only. There is currently no cryptography of any
kind — classical or post-quantum — anywhere in `kernel-x86_64` or
`reference-rs`.**

## The honest starting point: there's nothing to upgrade yet

"Add post-quantum crypto" implicitly assumes there's a classical
cryptographic scheme in place that needs replacing. There isn't one. This
kernel has:

- No signature verification of anything (boot images, patches, IPC
  messages).
- No encryption of anything (memory contents, IPC payloads, the
  eventual-someday network stack).
- No key management, no RNG beyond whatever `x86_64`/`core` provide
  incidentally, no TLS-equivalent, nothing.

So the real first question isn't "which post-quantum algorithm," it's
**what would cryptography in this kernel actually protect, and against
what threat model?** A few candidate uses, each with a genuinely
different design:

1. **Boot image integrity** (pairs directly with
   [`SECURE_BOOT.md`](SECURE_BOOT.md)) — verifying the kernel ELF/disk
   image wasn't tampered with before `kernel_main` runs. A signature
   scheme (Dilithium/ML-DSA) verified by the bootloader or firmware.
2. **IPC confidentiality/integrity** between two address spaces or two
   federated nodes (pairs with
   [`FEDERATION_PROTOCOL.md`](FEDERATION_PROTOCOL.md)) — needs a key
   exchange (Kyber/ML-KEM) plus a symmetric AEAD for the actual payload
   (post-quantum key exchange is typically paired with a *classical*
   symmetric cipher like AES-GCM/ChaCha20-Poly1305 for the bulk data —
   "post-quantum crypto" for a channel almost never means "everything is
   a novel PQ primitive," it means the key-exchange step is
   quantum-resistant).
3. **At-rest protection**, once a real filesystem exists (see the
   filesystem/network work item, tracked separately from this
   directory since it's judged achievable in-session) — a symmetric
   scheme, not really a "post-quantum" concern at all (symmetric crypto
   with a large enough key is already believed quantum-resistant; PQ
   specifically matters for public-key schemes, which quantum computers
   threaten via Shor's algorithm).

## Why "implement it from scratch this session" is the wrong move

Even setting aside the "which algorithm" question, hand-writing NIST PQC
candidates (Kyber/ML-KEM, Dilithium/ML-DSA, or the hash-based signature
schemes SPHINCS+/ML-DSA's stateful cousins like LMS/XMSS) from a spec, in
a `no_std` environment, without a security review, is precisely the kind
of work where a fast implementation is actively worse than none:

- **Constant-time discipline is the entire point** and the easiest thing
  to get subtly wrong — a single data-dependent branch or memory access
  pattern in the NTT (number-theoretic transform) core of a lattice
  scheme, or in a hash-based signature's tree traversal, can leak the
  private key to a timing or cache side-channel attacker, and this class
  of bug does not show up in ordinary functional tests. This kernel's
  own boot self-test discipline (real QEMU runs, real PASS/FAIL) has no
  way to catch a timing side-channel at all.
- **The reference implementations already exist and are reviewed.**
  `liboqs` (Open Quantum Safe) and the individual NIST submission
  reference/optimized implementations have had real cryptographic review
  that a from-scratch reimplementation here would not get. The honest
  path is *integrating* a vetted implementation, not writing a new one.
- **`no_std` FFI bindings to a C library are themselves real,
  non-trivial work** — `liboqs` is a C library; binding it into a
  `#![no_std]` `alloc`-only kernel means either (a) vendoring/porting a
  pure-Rust PQC crate (the `pqcrypto` family wraps PQClean, which is
  `no_std`-friendlier but still needs a real audit of what allocator/
  panic assumptions it makes) or (b) building a real C toolchain
  integration for this kernel's `x86_64-unknown-none` target and linking
  `liboqs` statically, verifying it doesn't pull in anything requiring an
  OS (file I/O, threads) this kernel doesn't provide.

## A real, phased plan (not attempted here)

1. **Write the actual threat model first** — what's being protected, from
   whom, and what's explicitly out of scope (e.g. "protects boot image
   integrity against an attacker who can modify the disk image, does not
   protect against a compromised build machine").
2. **Pick one concrete use case** (boot image signature verification is
   the most self-contained — no networking prerequisite) rather than
   "add crypto" generally.
3. **Evaluate `no_std`-compatible crate options for real** — build a
   throwaway test binary linking candidate crates against this exact
   target triple and toolchain, and confirm it actually compiles and
   produces correct known-answer-test (KAT) vectors from the NIST
   submission's own test vectors, before writing any kernel integration
   code.
4. **Integrate signature verification into the boot path**, gated behind
   a real, documented key-provisioning story (where does the public key
   live — baked into the bootloader, read from a fixed flash region —
   and how is it protected from the same attacker being defended
   against).
5. **Get real, independent review** before trusting this for anything —
   this is the one item in this whole future-work set where "we tested it
   and it passed" is *not* sufficient evidence of correctness, unlike
   everything else in this repository's verification discipline.

## Open questions nobody has answered yet

- Which specific NIST-standardized algorithm (ML-KEM/ML-DSA are the 2024
  FIPS-standardized names for Kyber/Dilithium) and parameter set (security
  level 1/3/5) — this is a real tradeoff between key/signature size and
  security margin that depends on the concrete use case chosen above.
- Does this kernel's `no_std`+`alloc` environment provide enough of a
  standard library surface for existing pure-Rust PQC crates to compile
  at all against `x86_64-unknown-none` — this needs to be checked
  empirically before any of the rest of this plan is meaningful, the same
  way this session empirically checked crate compatibility for
  `x86_64`/`linked_list_allocator`/etc.
- Is hybrid classical+post-quantum (e.g. X25519 + ML-KEM together, so a
  break in either scheme alone doesn't compromise the channel) the right
  default, matching current industry practice (e.g. TLS 1.3 hybrid key
  exchange), rather than PQ-only?
