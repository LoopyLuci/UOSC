/-
Real Lean 4 port of **Property 5, `scheduler_no_starvation`**, from
`../proofs/kernel_security.ax`, scoped to match the CFS half of
`../reference-rs/src/scheduler.rs`'s `RunQueue` (the pick-smallest-vruntime
mechanism). Lean 4 core only, no Mathlib.

Scope, stated up front: this file proves no-starvation for **two** competing
Normal-class tasks, for *any* starting vruntimes and *any* positive
vruntime increment. That is the real mathematical mechanism behind CFS
fairness — the task that's behind always eventually gets picked, because
picking it is what stops it from falling further behind.

**The general n-task case is now proved too — see `SchedulerN.lean` in this
same directory.** A first pass through this file left it as a written-out,
not-yet-machine-checked argument, on the reasoning that generalizing this
proof's single-opponent comparison naively (pick some other task `j` and
track the `t`-vs-`j` gap) breaks under tie-breaking: if `j` ties with `t`
and loses the tie, `j` can be re-picked without the gap ever closing, and
nothing in a single-opponent argument bounds how long that can recur. The
fix that made it through to a real proof was to stop comparing `t` against
one opponent and instead sum headroom across *all* other tasks at once
(`SchedulerN.lean`'s `phi`), paired with a tie-count that strictly drops
every time a genuinely-tied opponent is retired
(`SchedulerN.lean`'s `tieCount`) — see that file's header for the full
argument and `measureM_decreases_on_step` for where the two cases meet.
`scheduler.rs`'s existing `no_normal_task_starves_for_arbitrary_task_counts`
property test (256 random cases, up to 12 tasks) remains the empirical
cross-check that this formal model matches what the Rust scheduler
actually does.
-/

/-- One CFS scheduling step for two tasks: whichever has the smaller (or
equal) vruntime runs and has `inc` added to its vruntime, exactly
`RunQueue::pick_next_task` + `update_vruntime` for a two-task Normal-only
queue. -/
def stepPair (inc : Nat) (p : Nat × Nat) : Nat × Nat :=
  if p.1 ≤ p.2 then (p.1 + inc, p.2) else (p.1, p.2 + inc)

/-- `n` scheduling ticks from a starting pair. -/
def runPair (inc : Nat) : Nat → Nat × Nat → Nat × Nat
  | 0, p => p
  | n + 1, p => runPair inc n (stepPair inc p)

/-- **`scheduler_no_starvation`, two-task case**: for any starting vruntimes
`a` and `b` and any positive tick increment, there is some finite number of
ticks after which task `b`'s vruntime has changed — i.e., `b` was picked at
least once. Since this holds for *every* starting pair (not just the one
right after `b` was last picked), it holds repeatedly forever: `b` is
picked again and again, never starved. -/
theorem eventually_picked (inc : Nat) (hinc : 0 < inc) :
    ∀ a b : Nat, ∃ n, (runPair inc n (a, b)).2 ≠ b := by
  have key : ∀ gap a b : Nat, b - a ≤ gap → ∃ n, (runPair inc n (a, b)).2 ≠ b := by
    intro gap
    induction gap using Nat.strongRecOn with
    | ind gap ih =>
      intro a b hgap
      by_cases hle : a ≤ b
      · by_cases heq : a = b
        · -- a = b: one step keeps b unchanged (a is picked, tied), the
          -- following step now has a strictly ahead, so b is picked next.
          refine ⟨2, ?_⟩
          show (runPair inc 1 (stepPair inc (a, b))).2 ≠ b
          have hstep1 : stepPair inc (a, b) = (a + inc, b) := by simp [stepPair, hle]
          rw [hstep1]
          show (runPair inc 0 (stepPair inc (a + inc, b))).2 ≠ b
          have hnotle : ¬ a + inc ≤ b := by omega
          have hstep2 : stepPair inc (a + inc, b) = (a + inc, b + inc) := by
            simp [stepPair, hnotle]
          rw [hstep2]
          simp only [runPair]
          omega
        · -- a < b strictly: stepping brings a strictly closer to b, with a
          -- strictly smaller gap to recurse on.
          have halt : a < b := by omega
          have hstep1 : stepPair inc (a, b) = (a + inc, b) := by simp [stepPair, hle]
          have hgap' : b - (a + inc) < gap := by omega
          obtain ⟨n', hn'⟩ := ih (b - (a + inc)) hgap' (a + inc) b ((by omega))
          refine ⟨n' + 1, ?_⟩
          show (runPair inc n' (stepPair inc (a, b))).2 ≠ b
          rw [hstep1]
          exact hn'
      · -- a > b already: one step picks b directly.
        refine ⟨1, ?_⟩
        show (runPair inc 0 (stepPair inc (a, b))).2 ≠ b
        have hstep1 : stepPair inc (a, b) = (a, b + inc) := by simp [stepPair, hle]
        rw [hstep1]
        simp only [runPair]
        omega
  intro a b
  exact key (b - a) a b ((by omega))

/-- By symmetry of `stepPair` (swap the pair, swap the picked branch,
swap back), task `a` is equally never starved. Stated as its own theorem
rather than derived automatically, since `stepPair`'s tie-break (`a` wins
ties) makes the symmetry non-definitional — proved directly the same way. -/
theorem eventually_picked' (inc : Nat) (hinc : 0 < inc) :
    ∀ a b : Nat, ∃ n, (runPair inc n (a, b)).1 ≠ a := by
  have key : ∀ gap a b : Nat, a - b ≤ gap → ∃ n, (runPair inc n (a, b)).1 ≠ a := by
    intro gap
    induction gap using Nat.strongRecOn with
    | ind gap ih =>
      intro a b hgap
      by_cases hle : a ≤ b
      · by_cases heq : a = b
        · refine ⟨2, ?_⟩
          show (runPair inc 1 (stepPair inc (a, b))).1 ≠ a
          have hstep1 : stepPair inc (a, b) = (a + inc, b) := by simp [stepPair, hle]
          rw [hstep1]
          show (runPair inc 0 (stepPair inc (a + inc, b))).1 ≠ a
          have hnotle : ¬ a + inc ≤ b := by omega
          have hstep2 : stepPair inc (a + inc, b) = (a + inc, b + inc) := by
            simp [stepPair, hnotle]
          rw [hstep2]
          simp only [runPair]
          omega
        · refine ⟨1, ?_⟩
          show (runPair inc 0 (stepPair inc (a, b))).1 ≠ a
          have hstep1 : stepPair inc (a, b) = (a + inc, b) := by simp [stepPair, hle]
          rw [hstep1]
          simp only [runPair]
          omega
      · have hgt : b < a := by omega
        have hstep1 : stepPair inc (a, b) = (a, b + inc) := by simp [stepPair, hle]
        have hgap' : a - (b + inc) < gap := by omega
        obtain ⟨n', hn'⟩ := ih (a - (b + inc)) hgap' a (b + inc) ((by omega))
        refine ⟨n' + 1, ?_⟩
        show (runPair inc n' (stepPair inc (a, b))).1 ≠ a
        rw [hstep1]
        exact hn'
  intro a b
  exact key (a - b) a b ((by omega))
