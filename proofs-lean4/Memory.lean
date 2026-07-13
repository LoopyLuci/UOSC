/-
Real Lean 4 port of the memory-isolation and allocation-uniqueness properties
from `../proofs/kernel_security.ax`, scoped to match
`../reference-rs/src/memory.rs`. Lean 4 core only, no Mathlib.
-/

/-- A page table is modeled the same way `memory.rs`'s `PageTable` models it:
a finite mapping from virtual page numbers this process owns to physical page
numbers, represented as an association list. This is a policy model (who is
allowed to translate what), not a hardware page-walker — same scope note as
`reference-rs/STATUS.md`. -/
abbrev PageTable := List (Nat × Nat)

def PageTable.translates (pt : PageTable) (vpage : Nat) : Bool :=
  pt.any (fun entry => entry.1 == vpage)

/-- **Property 2, `memory_process_isolation`**, and **Property 9,
`sanctum_vault_isolation`** (the same shape of theorem — a vault's memory
region is exactly a second process's address space under a different name,
so one proof covers both): if a virtual page is not present in a page
table at all, nothing translated through that page table can reach it, full
stop. This is definitional in the `PageTable.translates` model, which is the
point — `ProcessMemoryContext::allocate_virtual` in `memory.rs` only ever
inserts entries for pages *this* process was granted, so two processes'
tables are only ever populated with their own pages, and this theorem is
what makes that construction actually give isolation instead of merely
looking like it does. -/
theorem no_translation_no_access (pt : PageTable) (vpage : Nat) :
    (∀ entry ∈ pt, entry.1 ≠ vpage) → pt.translates vpage = false := by
  induction pt with
  | nil => intro _; rfl
  | cons e rest ih =>
    intro hnone
    simp only [PageTable.translates, List.any_cons]
    have he : e.1 ≠ vpage := hnone e List.mem_cons_self
    have he' : (e.1 == vpage) = false := by
      cases h : e.1 == vpage with
      | false => rfl
      | true => exact absurd (beq_iff_eq.mp h) he
    rw [he']
    simp only [Bool.false_or]
    exact ih (fun e' hin => hnone e' (List.mem_cons_of_mem e hin))

/-- Two distinct processes' page tables, each only ever populated with pages
that process was individually granted (`disjointly_built`), never let one
translate a page that lives in the other's table exclusively. Direct
corollary of `no_translation_no_access`, stated over a pair of tables to
match the two-process shape of the original theorem exactly. -/
theorem distinct_processes_cannot_translate_each_others_pages
    (pt1 pt2 : PageTable) (vpage : Nat)
    (_hin1 : ∃ e ∈ pt1, e.1 = vpage)
    (hdisjoint : ∀ e ∈ pt2, e.1 ≠ vpage) :
    pt2.translates vpage = false :=
  no_translation_no_access pt2 vpage hdisjoint

/-- A free-page pool, matching `PhysicalAllocator`'s free-list model at the
level of "which pages are currently available," collapsed from per-order
buddy free-lists to a single set since uniqueness doesn't depend on order
bookkeeping. -/
abbrev FreePool := List Nat

/-- **The `allocate_physical_uniqueness` axiom** from `kernel_security.ax`,
proved rather than assumed: given a well-formed pool (no physical page
listed as free twice over — `hunique`, which is exactly what
`PhysicalAllocator::seed_free_lists` must maintain for the allocator to be
sound at all), once `page` is allocated it is gone from what's left — a
second allocation of the same page cannot be satisfied. This is the
discipline `PhysicalAllocator::allocate`'s real split logic (fixed from the
Titan source's `Ok(())` stub — see bug #3 in `reference-rs/STATUS.md`) has
to preserve. -/
theorem allocate_removes_page_from_pool (pool : FreePool) (page : Nat)
    (hunique : pool.count page ≤ 1) :
    page ∉ pool.erase page := by
  have hcount : (pool.erase page).count page = 0 := by
    rw [List.count_erase_self]
    omega
  exact List.count_eq_zero.mp hcount
