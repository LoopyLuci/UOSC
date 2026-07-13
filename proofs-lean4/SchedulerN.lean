/-
General n-task extension of `Scheduler.lean`'s two-task
`scheduler_no_starvation` proof (Property 5) — the argument that file's
header wrote out but deliberately left unformalized. Proved here for
*any* number of competing tasks, *any* starting vruntimes, *any* positive
tick increment, and *any* adversarial tie-breaking policy among tied
tasks (not just the "first tied index wins" convention `scheduler.rs`
happens to use — this covers every possible tie-break rule at once).
Lean 4 core only, no Mathlib.

**Model.** A distinguished target task `t`'s vruntime, `targetVal`, is
fixed for as long as `t` isn't picked — that's exactly the event being
proved to eventually occur, so `t` never needs its own position in a
list. Every *other* task's vruntime lives in `others : List Nat`; only
`others` evolves. At each step, an adversarial `choice : Nat → Option Nat`
sequence says what happens: `none` means "`t` was picked this step" (what
we're proving happens eventually), `some i` means "other task `i` was
picked" — required by `IsMinimalOther` to actually be a legitimate
argmin pick, not an arbitrary one.

**The monovariant.** `phi` sums how far below `targetVal` every other task
still is (0 once a task has caught up or passed). `tieCount` counts how
many other tasks are *exactly* tied with `targetVal` right now. Combined
as `measureM := phi * (length + 1) + tieCount`, this strictly decreases on
every step that doesn't pick `t`:
- if the picked task wasn't exactly tied, `phi` strictly drops (by
  `min inc headroom ≥ 1`), which dominates any change in `tieCount`
  (`measureM_decreases_on_step`'s "strict case");
- if it *was* exactly tied, `phi` doesn't move, but the tie is permanently
  broken (that task can never return to `targetVal`, only rise past it),
  so `tieCount` strictly drops by exactly 1 (the "tie case").

Either way `measureM` strictly decreases, so it can't decrease forever —
`t` must eventually be picked. That's `eventually_none` below, proved by
strong induction on `measureM`.
-/

/-- Truncated headroom: how far below `targetVal` a value still is. -/
def headroom (targetVal v : Nat) : Nat := targetVal - v

/-- Sum of headroom over every other task. -/
def phi (targetVal : Nat) : List Nat → Nat
  | [] => 0
  | v :: rest => headroom targetVal v + phi targetVal rest

/-- How many other tasks are *exactly* tied with `targetVal` right now. -/
def tieCount (targetVal : Nat) : List Nat → Nat
  | [] => 0
  | v :: rest => (if v = targetVal then 1 else 0) + tieCount targetVal rest

/-- One CFS tick applied to task `i` among the others. -/
def stepOthers (inc i : Nat) (others : List Nat) : List Nat :=
  others.set i (others.getD i 0 + inc)

/-- `i` is a valid pick among the others: it achieves the minimum vruntime
among all other tasks, and — since it must also beat `t` itself to be a
legitimate global argmin — its value is at most `targetVal`. -/
def IsMinimalOther (targetVal : Nat) (others : List Nat) (i : Nat) : Prop :=
  i < others.length ∧ (∀ k, k < others.length → others.getD i 0 ≤ others.getD k 0) ∧
    others.getD i 0 ≤ targetVal

/-- **L1**: stepping a task whose headroom is `d` decreases `phi` by
exactly `min inc d` — stated as an addition identity to sidestep
truncated-subtraction case analysis. -/
theorem phi_step (targetVal inc i : Nat) (others : List Nat) (hi : i < others.length) :
    phi targetVal others = phi targetVal (stepOthers inc i others) + min inc (headroom targetVal (others.getD i 0)) := by
  induction others generalizing i with
  | nil => simp at hi
  | cons v rest ih =>
    cases i with
    | zero =>
      simp only [stepOthers, List.set, phi, List.getD_cons_zero, headroom]
      omega
    | succ j =>
      simp only [List.length_cons] at hi
      have hj : j < rest.length := by omega
      have hrec := ih j hj
      simp only [stepOthers, List.getD_cons_succ] at hrec ⊢
      simp only [List.set, phi]
      omega

/-- **L2**: stepping a task that's exactly tied with `targetVal` strictly
decreases `tieCount` by 1 — that task can never tie again (its value only
rises from here). -/
theorem tieCount_step_tied (targetVal inc i : Nat) (others : List Nat)
    (hi : i < others.length) (htied : others.getD i 0 = targetVal) (hinc : 0 < inc) :
    tieCount targetVal others = tieCount targetVal (stepOthers inc i others) + 1 := by
  induction others generalizing i with
  | nil => simp at hi
  | cons v rest ih =>
    cases i with
    | zero =>
      simp only [List.getD_cons_zero] at htied
      have hval : stepOthers inc 0 (v :: rest) = (v + inc) :: rest := by
        simp [stepOthers, List.set]
      rw [hval]
      unfold tieCount
      have h1 : (if v = targetVal then (1 : Nat) else 0) = 1 := by simp [htied]
      have h2 : (if v + inc = targetVal then (1 : Nat) else 0) = 0 := by
        have hne : v + inc ≠ targetVal := by omega
        simp [hne]
      rw [h1, h2]
      omega
    | succ j =>
      simp only [List.length_cons] at hi
      have hj : j < rest.length := by omega
      simp only [List.getD_cons_succ] at htied
      have hrec := ih j hj htied
      simp only [stepOthers, List.getD_cons_succ] at hrec ⊢
      simp only [List.set, tieCount]
      omega

theorem tieCount_le_length (targetVal : Nat) (others : List Nat) :
    tieCount targetVal others ≤ others.length := by
  induction others with
  | nil => simp [tieCount]
  | cons v rest ih =>
    simp only [tieCount, List.length_cons]
    split <;> omega

/-- The combined, always-strictly-decreasing measure. -/
def measureM (targetVal : Nat) (others : List Nat) : Nat :=
  phi targetVal others * (others.length + 1) + tieCount targetVal others

/-- The heart of the argument: every valid non-`t` pick strictly decreases
`measureM`, in *either* of the two cases L1/L2 cover. -/
theorem measureM_decreases_on_step
    (targetVal inc i : Nat) (others : List Nat) (hinc : 0 < inc) (hmin : IsMinimalOther targetVal others i) :
    measureM targetVal (stepOthers inc i others) < measureM targetVal others := by
  obtain ⟨hi, _hall, hle⟩ := hmin
  have hlen : (stepOthers inc i others).length = others.length := by simp [stepOthers]
  by_cases htie : others.getD i 0 = targetVal
  · -- tie case: phi unchanged, tieCount strictly decreases
    have hphi_eq : phi targetVal (stepOthers inc i others) = phi targetVal others := by
      have hstep := phi_step targetVal inc i others hi
      have hzero : headroom targetVal (others.getD i 0) = 0 := by
        unfold headroom
        omega
      omega
    have htc := tieCount_step_tied targetVal inc i others hi htie hinc
    unfold measureM
    rw [hphi_eq, hlen]
    omega
  · -- strict case: phi strictly decreases by min inc headroom ≥ 1, dominating any tieCount change
    have hstep := phi_step targetVal inc i others hi
    have hhr : 0 < headroom targetVal (others.getD i 0) := by
      simp only [headroom]
      omega
    have hdecr : 0 < min inc (headroom targetVal (others.getD i 0)) := by omega
    have htc_le : tieCount targetVal (stepOthers inc i others) ≤ (stepOthers inc i others).length :=
      tieCount_le_length targetVal (stepOthers inc i others)
    unfold measureM
    rw [hlen] at htc_le ⊢
    have hmul : phi targetVal others * (others.length + 1)
        = phi targetVal (stepOthers inc i others) * (others.length + 1)
          + min inc (headroom targetVal (others.getD i 0)) * (others.length + 1) := by
      rw [hstep, Nat.add_mul]
    have hpos : others.length + 1 ≤ min inc (headroom targetVal (others.getD i 0)) * (others.length + 1) := by
      have h1 : 1 ≤ min inc (headroom targetVal (others.getD i 0)) := hdecr
      have h2 := Nat.mul_le_mul_right (others.length + 1) h1
      omega
    rw [hmul]
    omega

/-- Whether `t` (`none`) or some other task (`some i`) is a legitimate pick
against the current `others` state. -/
def NextValid (targetVal : Nat) (others : List Nat) (choice : Nat → Option Nat) : Prop :=
  match choice 0 with
  | none => ∀ j, j < others.length → targetVal ≤ others.getD j 0
  | some i => IsMinimalOther targetVal others i

/-- The state of `others` after `k` steps under an adversarial `choice`
sequence, reshifting `choice` by one position at each recursive step so
`choice 0` always means "the next pick from here." -/
def runOthers (inc : Nat) : List Nat → (Nat → Option Nat) → Nat → List Nat
  | others, _, 0 => others
  | others, choice, k + 1 =>
      match choice 0 with
      | none => others
      | some i => runOthers inc (stepOthers inc i others) (fun j => choice (j + 1)) k

/-- **Property 5, general n-task case**: for *any* number of other tasks,
*any* starting vruntimes, and *any* adversarial (but always argmin-legal)
choice of who runs at each tick, `t` is eventually picked. `hvalid`
requires every step of the run to be a legitimate pick — the same
"scheduler actually implements argmin" assumption `scheduler.rs`'s
`RunQueue::pick_next_task` is required to satisfy, just stated for an
arbitrary adversarial tie-break instead of the concrete first-occurrence
one that type actually uses (concrete first-occurrence tie-breaking is one
particular instance of a legal `choice` sequence, so this theorem covers
it as a special case). -/
theorem eventually_none (targetVal inc : Nat) (hinc : 0 < inc) :
    ∀ others : List Nat, ∀ choice : Nat → Option Nat,
    (∀ n : Nat, NextValid targetVal (runOthers inc others choice n) (fun j => choice (n + j))) →
    ∃ n, choice n = none := by
  have key : ∀ m : Nat, ∀ others : List Nat, measureM targetVal others = m → ∀ choice : Nat → Option Nat,
      (∀ n : Nat, NextValid targetVal (runOthers inc others choice n) (fun j => choice (n + j))) →
      ∃ n, choice n = none := by
    intro m
    induction m using Nat.strongRecOn with
    | ind m ih =>
      intro others hm choice hvalid
      have hv0 := hvalid 0
      simp only [runOthers, Nat.zero_add, NextValid] at hv0
      cases hchoice0 : choice 0 with
      | none => exact ⟨0, hchoice0⟩
      | some i =>
        rw [hchoice0] at hv0
        have hdecr := measureM_decreases_on_step targetVal inc i others hinc hv0
        rw [hm] at hdecr
        have hrun : ∀ n, runOthers inc (stepOthers inc i others) (fun j => choice (j + 1)) n
            = runOthers inc others choice (n + 1) := by
          intro n
          simp only [runOthers, hchoice0]
        have hvalid' : ∀ n, NextValid targetVal (runOthers inc (stepOthers inc i others) (fun j => choice (j + 1)) n)
            (fun j => (fun j => choice (j + 1)) (n + j)) := by
          intro n
          rw [hrun n]
          have := hvalid (n + 1)
          have heq : (fun j => choice ((n + 1) + j)) = (fun j => (fun j => choice (j + 1)) (n + j)) := by
            funext j
            congr 1
            omega
          rwa [heq] at this
        obtain ⟨n', hn'⟩ := ih (measureM targetVal (stepOthers inc i others)) hdecr
          (stepOthers inc i others) rfl (fun j => choice (j + 1)) hvalid'
        exact ⟨n' + 1, hn'⟩
  intro others choice hvalid
  exact key (measureM targetVal others) others rfl choice hvalid
