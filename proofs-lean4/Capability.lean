/-
Real, mechanically-checked Lean 4 port of a subset of `../proofs/kernel_security.ax`,
scoped to match what `../reference-rs/src/capability.rs` actually implements.

This uses Lean 4's core library only (no Mathlib), so `lean Capability.lean`
type-checks with no network access and no external build. A green check here
means Lean's kernel accepted every proof term in this file — nothing here is
a comment asserting correctness, it's a claim the compiler verified.
-/

/-- Mirrors `Permissions` in `capability.rs`. -/
structure Permissions where
  read     : Bool
  write    : Bool
  execute  : Bool
  delegate : Bool
deriving DecidableEq, Repr

/-- Mirrors `Permissions::covers`: `a.covers b` means `a` grants everything `b` grants. -/
def Permissions.covers (a b : Permissions) : Bool :=
  (!b.read || a.read) && (!b.write || a.write) &&
  (!b.execute || a.execute) && (!b.delegate || a.delegate)

/-- Mirrors `Permissions::intersect`. -/
def Permissions.intersect (a b : Permissions) : Permissions :=
  { read := a.read && b.read, write := a.write && b.write
    execute := a.execute && b.execute, delegate := a.delegate && b.delegate }

/-- **Property 8 core, `capability_delegation_authentic`'s permission-bound
half**: a delegated capability can never exceed what its source held. This is
the invariant `CapabilityBroker::delegate` in `capability.rs` relies on — it
builds the delegate's permission set with `source.perms.intersect(requested)`,
and this theorem is why that's safe no matter what `requested` asks for. -/
theorem delegate_never_exceeds_source (source requested : Permissions) :
    source.covers (source.intersect requested) = true := by
  unfold Permissions.intersect Permissions.covers
  cases source.read <;> cases source.write <;> cases source.execute <;> cases source.delegate <;>
    cases requested.read <;> cases requested.write <;> cases requested.execute <;> cases requested.delegate <;>
    decide

/-- A capability record, mirroring `Capability` in `capability.rs` (minus the
fields — expiry, path — that don't change this theorem's shape). -/
structure Capability where
  owner    : Nat
  resource : Nat
  perms    : Permissions
  revoked  : Bool
deriving DecidableEq, Repr

abbrev CapStore := List Capability

/-- Mirrors `CapabilityBroker::check_access`: access is granted exactly when
a live (non-revoked) capability for that exact (owner, resource) pair exists
in the store. -/
def CapStore.checkAccess (s : CapStore) (owner resource : Nat) : Bool :=
  s.any (fun c => c.owner == owner && c.resource == resource && !c.revoked)

/-- **Property 1, `process_capability_confinement`**: if no live capability
for (owner, resource) exists anywhere in the store, access is denied. Proved
by induction on the store rather than by unfolding a separate "has capability"
predicate — `checkAccess` is directly the thing being reasoned about, the
same function `check_access` in the Rust code actually calls. -/
theorem process_capability_confinement (s : CapStore) (owner resource : Nat) :
    (∀ c ∈ s, ¬ (c.owner = owner ∧ c.resource = resource ∧ c.revoked = false)) →
    s.checkAccess owner resource = false := by
  induction s with
  | nil => intro _; rfl
  | cons c rest ih =>
    intro hnone
    simp only [CapStore.checkAccess, List.any_cons]
    have hc : ¬ (c.owner = owner ∧ c.resource = resource ∧ c.revoked = false) :=
      hnone c List.mem_cons_self
    have hc' : (c.owner == owner && c.resource == resource && !c.revoked) = false := by
      cases h1 : c.owner == owner with
      | false => simp
      | true =>
        cases h2 : c.resource == resource with
        | false => simp
        | true =>
          cases h3 : c.revoked with
          | false => exact absurd ⟨beq_iff_eq.mp h1, beq_iff_eq.mp h2, h3⟩ hc
          | true => simp
    rw [hc']
    simp only [Bool.false_or]
    exact ih (fun c' hin => hnone c' (List.mem_cons_of_mem c hin))

/-- **Property 3, `capability_revocation_effective`**: once a capability's
`revoked` flag is set, no store containing only that (now-revoked) record for
a given (owner, resource) grants access through it. Modeled directly against
`checkAccess`'s definition: a revoked record can never satisfy the
`!c.revoked` conjunct. -/
theorem revoked_capability_denies_access (owner resource : Nat) (perms : Permissions) :
    (CapStore.checkAccess [{ owner := owner, resource := resource, perms := perms, revoked := true }] owner resource) = false := by
  simp [CapStore.checkAccess]

/-- The documented gap this whole effort insists on stating rather than
hiding (see `capability.rs`'s
`revoking_source_does_not_touch_an_independently_issued_delegate_token`
test and `../reference-rs/STATUS.md`): revoking one capability record does
not touch a second, independently-issued record for the same resource. This
is *not* a bug — cascading revocation was never claimed — but it means
Property 3 only holds per-capability, not per-(owner, resource) pair, and
this theorem makes that boundary explicit and checked rather than assumed. -/
theorem revocation_is_per_capability_not_per_resource
    (owner resource : Nat) (perms : Permissions) :
    ∃ (s : CapStore),
      (∃ c ∈ s, c.owner = owner ∧ c.resource = resource ∧ c.revoked = true) ∧
      s.checkAccess owner resource = true := by
  refine ⟨[{ owner := owner, resource := resource, perms := perms, revoked := true },
           { owner := owner, resource := resource, perms := perms, revoked := false }], ?_, ?_⟩
  · exact ⟨_, List.mem_cons_self, rfl, rfl, rfl⟩
  · simp [CapStore.checkAccess]
