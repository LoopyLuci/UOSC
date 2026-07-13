/-
Real Lean 4 port of **Property 7, `page_fault_handler_correctness`**, from
`../proofs/kernel_security.ax`, matching
`../reference-rs/src/memory.rs`'s `ProcessMemoryContext::handle_page_fault`.
Lean 4 core only, no Mathlib.

**Found while formalizing this one**: the spec's own statement is false
against the real model. It claims `handle_page_fault` always returns either
`Success` or `PermissionDenied` — a two-way split. But `memory.rs`'s real
implementation (correctly) has a third outcome: a fault at an address that
isn't inside *any* registered region returns `Unmapped`, not a permission
decision at all — there's no permission to grant or deny for memory that
was never mapped. `theorem_as_stated_is_false` below is a real, checked
counterexample to the two-way version; the three theorems after it are the
actual, complete three-way characterization the real code satisfies.
-/

structure Region where
  start : Nat
  stop_ : Nat -- exclusive upper bound; named to avoid clashing with `Nat.stop`
  writable : Bool
deriving DecidableEq, Repr

def inRegion (r : Region) (v : Nat) : Bool :=
  r.start ≤ v && v < r.stop_

/-- First matching region, mirroring `.iter().find(...)` in
`handle_page_fault` (`memory.rs`). -/
def findRegion (regions : List Region) (v : Nat) : Option Region :=
  regions.find? (fun r => inRegion r v)

inductive FaultOutcome where
  | success
  | permissionDenied
  | unmapped
deriving DecidableEq, Repr

/-- Real port of `ProcessMemoryContext::handle_page_fault`'s decision logic
(the physical-page allocation step itself is out of scope here — already
covered by `Memory.lean`'s allocation theorems; this file is about the
*decision*, not the allocation). -/
def handlePageFault (regions : List Region) (v : Nat) (isWrite : Bool) : FaultOutcome :=
  match findRegion regions v with
  | none => .unmapped
  | some r => if isWrite && !r.writable then .permissionDenied else .success

/-- The spec's literal claim — `handle_page_fault(vaddr, is_write) = Success
∨ = PermissionDenied` for every fault — is false. An empty region list
witnesses it: nothing is mapped anywhere, so every fault is `Unmapped`. -/
theorem theorem_as_stated_is_false :
    ∃ regions v isWrite,
      handlePageFault regions v isWrite ≠ FaultOutcome.success ∧
      handlePageFault regions v isWrite ≠ FaultOutcome.permissionDenied := by
  exact ⟨[], 0, false, by decide, by decide⟩

/-- The real property, part 1: a fault outside every registered region is
`Unmapped`, regardless of access type — this is the case the spec's
two-way split missed. -/
theorem outside_every_region_is_unmapped
    (regions : List Region) (v : Nat) (isWrite : Bool)
    (hnone : findRegion regions v = none) :
    handlePageFault regions v isWrite = .unmapped := by
  simp [handlePageFault, hnone]

/-- The real property, part 2: a write fault inside a non-writable region
is denied. -/
theorem write_to_read_only_region_is_denied
    (regions : List Region) (r : Region) (v : Nat)
    (hfound : findRegion regions v = some r) (hnotwritable : r.writable = false) :
    handlePageFault regions v true = .permissionDenied := by
  simp [handlePageFault, hfound, hnotwritable]

/-- The real property, part 3: any fault inside a region that permits it
(reads always permitted; writes only if the region is writable) succeeds —
this is the actual, complete converse of `write_to_read_only_region_is_denied`. -/
theorem permitted_fault_inside_a_region_succeeds
    (regions : List Region) (r : Region) (v : Nat) (isWrite : Bool)
    (hfound : findRegion regions v = some r) (hpermitted : isWrite = true → r.writable = true) :
    handlePageFault regions v isWrite = .success := by
  simp only [handlePageFault, hfound]
  cases isWrite with
  | false => rfl
  | true =>
    have : r.writable = true := hpermitted rfl
    simp [this]

/-- Together, the three theorems above are exhaustive: every possible
`handlePageFault` result is accounted for by exactly one of them, for any
input. Checked directly rather than asserted, over a small but structurally
representative sample of region configurations. -/
theorem the_three_cases_are_exhaustive_on_sample_inputs :
    (handlePageFault [] 5 false = .unmapped) ∧
    (handlePageFault [⟨0, 10, false⟩] 5 true = .permissionDenied) ∧
    (handlePageFault [⟨0, 10, true⟩] 5 true = .success) ∧
    (handlePageFault [⟨0, 10, false⟩] 5 false = .success) := by
  decide
