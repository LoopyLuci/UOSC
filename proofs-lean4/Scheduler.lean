/-
Real Lean 4 port of **Property 5, `scheduler_no_starvation`**, from
`../proofs/kernel_security.ax`, scoped to match the CFS half of
`../reference-rs/src/scheduler.rs`'s `RunQueue` (the pick-smallest-vruntime
mechanism). Lean 4 core only, no Mathlib.

Scope, stated up front: this file proves no-starvation for **two** competing
Normal-class tasks, for *any* starting vruntimes and *any* positive
vruntime increment. That is the real mathematical mechanism behind CFS
fairness — the task that's behind always eventually gets picked, because
picking it is what stops it from falling further behind. The n-task case
(arbitrary queue size) is exercised empirically, not proved here, by
`scheduler.rs`'s existing `no_normal_task_starves_for_arbitrary_task_counts`
property test (256 random cases, up to 12 tasks).

**The general n-task argument, worked out but not (yet) machine-checked**:
naively generalizing the two-task proof by picking some other single task
`j` to compare against `t` doesn't work — if `j` ties with `t` and loses
the tie-break, `j` can be re-picked without the `t`-vs-`j` gap closing at
all, and nothing in a single-opponent argument bounds how long that can
recur. The fix is to stop comparing `t` against one opponent and instead
track `restMin(vs, t) := min { vs[i] | i ≠ t }`, the minimum over
*everyone else*. Two facts make this work where the pairwise approach
didn't:

1. `restMin` is non-decreasing over time, full stop — every step increments
   exactly one entry by `inc` and never decreases anything, so the minimum
   of any fixed subset of entries can only rise or hold.
2. `restMin` cannot *hold* for more than `n - 1` consecutive steps that
   don't pick `t`: each such step increments whichever other task is
   currently *at* `restMin`; once incremented, that task can never return
   to `restMin` (monotonicity again), so it's permanently retired from the
   set of tasks still capable of holding `restMin` at its current value.
   There are at most `n - 1` other tasks to retire, so `restMin` must
   strictly increase (by at least `inc`) within `n - 1` steps of not
   picking `t`.

Together: as long as `vs[t] > restMin`, repeat "at most `n-1` steps" rounds,
each provably increasing `restMin` by at least `inc`, until
`vs[t] ≤ restMin` — at which point `t` is (tied-for-)minimal and must be
picked. This gives an explicit bound, `(n - 1) * ⌈(vs[t] - restMin₀) / inc⌉`
steps, generalizing the two-task proof's single-opponent bound exactly the
way you'd hope. It avoids the tie-break trap because `restMin` bundles all
`n - 1` opponents into one monovariant instead of tracking one at a time.

This is a complete, correct argument, not a hand-wave — but turning it into
Lean means representing `restMin` over a `List Nat` with position identity
preserved across `List.set` updates, and the "at most `n-1` retirements"
step needs its own bounded induction over a shrinking finite set of
not-yet-retired indices. That's real, additional proof engineering on top
of what's in this file, with a real chance of hitting the same class of
fiddly index/`List` lemma issues the two-task proof needed several
iterations to resolve — attempting it inside this pass risked leaving
something half-finished or subtly wrong with no compiler run left to catch
it. Recorded here precisely enough that finishing it is a bounded,
well-defined follow-on task, not an open question.
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
