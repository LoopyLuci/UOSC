# A federation protocol for multiple UOSC instances

**Status: design analysis only, and unusually, this doc can't fully do
even that yet — "federation" has never been given a stated goal anywhere
in this codebase's history. What follows is the requirements-gathering
that would need to happen *before* protocol design is even meaningful,
plus what this kernel would need first regardless of the answers.**

## Why this is the least-scoped item in this directory

Every other doc in `future-work/` describes a reasonably well-understood
engineering problem (port an OS to a new ISA, sign a boot image, patch
running code) where the *destination* is clear even though the path is
long. "Federation" has no equivalent clarity here — the word doesn't
appear anywhere in `kernel-x86_64/` or `reference-rs`'s design beyond the
root README's exclusion list. Before any protocol design, real questions
need real answers:

- **Federation of what, between what?** Candidate meanings, each implying
  a completely different protocol:
  - Multiple UOSC *kernel instances* (separate physical/virtual machines)
    coordinating scheduling or resource decisions — closer to a
    distributed-systems consensus problem.
  - Multiple *processes/address spaces* within future UOSC instances
    migrating or sharing state across a network — closer to a distributed
    IPC/RPC problem, and depends on the syscall-ABI/process work tracked
    separately from this directory actually existing first.
  - UOSC nodes forming a trust federation for capability delegation
    (`uosc_core::capability`'s `CapabilityBroker` already models
    single-node issuance/revocation with expiry — a *federated* capability
    system would need to answer how a token issued on node A is verified
    and honored on node B without node B trusting node A unconditionally).
- **What's the trust model between nodes?** Mutually trusting (a cluster
  under one administrative domain) vs. mutually suspicious (genuinely
  federated, each node protecting itself from the others) leads to
  completely different designs — the former can share simple, fast
  primitives; the latter needs real authentication, likely tying into
  [`POST_QUANTUM_CRYPTO.md`](POST_QUANTUM_CRYPTO.md)'s open questions
  about what cryptography this kernel eventually has at all.
- **What's the consistency requirement?** Strong consistency (real
  consensus — Raft/Paxos-family, with real leader election and log
  replication) vs. eventual consistency (gossip-style state
  reconciliation) vs. no shared state at all (federation only for
  discovery/routing, not shared truth) are different protocols with
  different real failure modes, and nothing about UOSC's actual intended
  deployment shape (how many nodes, what network reliability, what
  failure model) has been specified anywhere to decide between them.
- **Is there even a network stack to run a protocol over?** No — this
  kernel has no network driver, no packet processing, nothing. A
  federation protocol's wire format is meaningless to design in detail
  before there's a real transport to carry it, the same dependency
  problem [`LIVE_PATCHING.md`](LIVE_PATCHING.md) has on a dynamic loader
  that doesn't exist yet.

## What would have to exist first, regardless of which federation design
is eventually chosen

1. **A real network stack** (or, at minimum, a real driver for one NIC and
   enough of a packet path to send/receive framed messages) — currently
   entirely absent, and itself a substantial, separate project (driver
   development against real or emulated hardware, a real protocol stack
   layer, real error handling for a genuinely unreliable transport in a
   way this kernel's current all-local-memory IPC never has to consider).
2. **A real multi-node test harness.** This session's entire verification
   discipline has been "boot one QEMU instance, read its exit code."
   Testing *any* federation protocol for real needs multiple cooperating
   instances, a way to inject real network partitions/delays/reordering
   between them, and a way to assert on cross-node outcomes — none of
   which this repository's tooling does today.
3. **A decided identity/naming scheme** for nodes — even the simplest
   federation design needs a real answer to "how does node A refer to
   node B," and whether that's a flat namespace, a hierarchical one, or
   tied to the cryptographic identity work in
   [`SECURE_BOOT.md`](SECURE_BOOT.md)/[`POST_QUANTUM_CRYPTO.md`](POST_QUANTUM_CRYPTO.md).

## A real, phased plan (not attempted here) — contingent on answering the
open questions above first

1. **Write an actual requirements document** naming the specific use case
   this is meant to serve (not "federation" in the abstract) — this is
   real prerequisite work, not a formality, since every later design
   decision depends on it.
2. **Build the network stack prerequisite** (a real, separate, large
   effort) before any federation-specific code, the same layering
   [`LIVE_PATCHING.md`](LIVE_PATCHING.md) requires a dynamic loader first.
3. **Prototype the simplest version of the chosen use case** between
   exactly two nodes before generalizing to N, with a real multi-instance
   test setup built specifically for this (extending this repo's existing
   single-instance QEMU batch-testing discipline to coordinate multiple
   QEMU processes with a real virtual network between them — QEMU's
   user-mode or tap networking could plausibly provide this, but hasn't
   been evaluated for it).
4. **Only then**, generalize toward whatever consistency/trust model the
   requirements document called for.

## Open questions nobody has answered yet

- All of the "federation of what, between what" questions above — this
  is the actual blocking question, not an implementation detail.
- Is this meant to compose with the multiple-address-space work that
  *does* now exist for real in `kernel-x86_64` (see `STATUS.md`'s "Real
  multiple address spaces" section) — e.g. a federated capability
  crossing into a locally-isolated address space — or is federation
  conceived at the whole-node level only?
- What's the actual deployment scale this needs to handle (two nodes on a
  LAN vs. a large, WAN-distributed set) — this changes essentially every
  answer above.
