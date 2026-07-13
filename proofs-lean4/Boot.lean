/-
Real Lean 4 port of **Property 10, `boot_sequence_integrity`**, from
`../proofs/kernel_security.ax`, now that `../reference-rs/src/boot.rs`
gives this theorem a real code referent to formalize against (bug #7 in
`../reference-rs/STATUS.md` — the Titan source never built the `BootPhase`
machinery the theorem's own statement assumes exists). Lean 4 core only, no
Mathlib.

This mirrors `boot.rs`'s `BootSequencer` exactly: six ordered phases, a
`complete` operation that's only supposed to be called once every earlier
phase is already done, and the invariant that a complete phase implies
every earlier phase is complete too.
-/

inductive BootPhase where
  | earlyBoot
  | lateBoot
  | schedulerBoot
  | sanctumBoot
  | ipcBoot
  | syscallBoot
deriving DecidableEq, Repr

/-- Fixed position in the boot order — same six phases, same order, as
`BootPhase::ORDER` in `boot.rs`. -/
def BootPhase.idx : BootPhase → Nat
  | .earlyBoot => 0
  | .lateBoot => 1
  | .schedulerBoot => 2
  | .sanctumBoot => 3
  | .ipcBoot => 4
  | .syscallBoot => 5

/-- `q` is ordered strictly before `p`. -/
def earlierThan (q p : BootPhase) : Bool :=
  q.idx < p.idx

/-- A boot sequencer's state: which phases have been marked complete.
Represented as a function rather than `boot.rs`'s fixed-size array —
equivalent content, avoids index-out-of-bounds bookkeeping Lean doesn't
need since `BootPhase` is already the index type. -/
abbrev BootSequencer := BootPhase → Bool

def BootSequencer.empty : BootSequencer := fun _ => false

/-- Unconditional update — mirrors what `boot.rs`'s `complete` does to
its `completed` array *after* its guard passes. The guard itself is
`hprereq` in the theorems below, kept as a hypothesis rather than baked
into a `Result`-returning function, since Lean doesn't need the runtime
error value to state the property. -/
def BootSequencer.complete (s : BootSequencer) (p : BootPhase) : BootSequencer :=
  fun q => if q = p then true else s q

/-- Exactly `kernel_security.ax`'s `theorem boot_sequence_integrity`:
every phase ordered before a complete phase is itself complete. -/
def BootSequencer.satisfiesInvariant (s : BootSequencer) : Prop :=
  ∀ p q : BootPhase, s p = true → earlierThan q p = true → s q = true

theorem empty_satisfies_invariant : BootSequencer.empty.satisfiesInvariant := by
  intro p q hp _
  simp [BootSequencer.empty] at hp

/-- **The real enforcement theorem**: `boot.rs`'s `complete` only ever
calls the unconditional update after checking every earlier phase is
already done (`hprereq`) — this is the proof that doing so is exactly
what's needed to keep the invariant true, matching what `complete`'s guard
is actually for. -/
theorem complete_preserves_invariant (s : BootSequencer) (p : BootPhase)
    (hinv : s.satisfiesInvariant)
    (hprereq : ∀ q, earlierThan q p = true → s q = true) :
    (s.complete p).satisfiesInvariant := by
  intro p' q' hp' hq'
  simp only [BootSequencer.complete] at hp' ⊢
  split at hp'
  · -- p' = p: q' is either p itself (trivially now true) or genuinely
    -- earlier than p, covered by hprereq.
    rename_i heq
    subst heq
    by_cases hqp : q' = p'
    · simp [hqp]
    · simp only [hqp, if_false]
      exact hprereq q' hq'
  · -- p' ≠ p: p' was already complete before this update, so the
    -- original invariant already guarantees q' is complete; the update
    -- can only add `p` to the completed set, never remove anything.
    by_cases hqp : q' = p
    · simp [hqp]
    · simp only [hqp, if_false]
      exact hinv p' q' hp' hq'

/-- Sanity instance: completing all six phases in the real boot order, each
guarded by "every earlier phase already done" exactly like `boot.rs`'s
`phases_complete_in_order_succeed` test exercises, produces a fully-complete,
invariant-satisfying sequencer. Built by six concrete, fully unfolded
applications of `complete_preserves_invariant` rather than a general
induction over arbitrary prefixes — the concrete six-phase case is all
`boot.rs` actually needs, and staying concrete keeps every `hprereq`
side-condition a closed decidable proposition. -/
theorem completing_in_order_reaches_full_completion :
    ∃ s : BootSequencer, s.satisfiesInvariant ∧ ∀ p, s p = true := by
  have h0 := empty_satisfies_invariant
  have h1 := complete_preserves_invariant BootSequencer.empty .earlyBoot h0
    (by intro q hq; cases q <;> simp_all [earlierThan, BootPhase.idx])
  have h2 := complete_preserves_invariant _ .lateBoot h1
    (by intro q hq; cases q <;> simp_all [earlierThan, BootPhase.idx, BootSequencer.complete])
  have h3 := complete_preserves_invariant _ .schedulerBoot h2
    (by intro q hq; cases q <;> simp_all [earlierThan, BootPhase.idx, BootSequencer.complete])
  have h4 := complete_preserves_invariant _ .sanctumBoot h3
    (by intro q hq; cases q <;> simp_all [earlierThan, BootPhase.idx, BootSequencer.complete])
  have h5 := complete_preserves_invariant _ .ipcBoot h4
    (by intro q hq; cases q <;> simp_all [earlierThan, BootPhase.idx, BootSequencer.complete])
  have h6 := complete_preserves_invariant _ .syscallBoot h5
    (by intro q hq; cases q <;> simp_all [earlierThan, BootPhase.idx, BootSequencer.complete])
  refine ⟨_, h6, ?_⟩
  intro p
  cases p <;> simp_all [BootSequencer.complete]
