# Runtime kernel code patching without a reboot

**Status: design analysis only. This kernel has no dynamic code loading
of any kind — the only code that ever runs is what's linked into the
single ELF image at build time, plus one fixed, hand-assembled ring-3
program (`syscall.rs`) copied byte-for-byte into a pre-mapped page.**

## The real prerequisite this kernel doesn't have yet

Live patching — replacing a running kernel function with a new version
without rebooting — presupposes the ability to load and link *new* code
into a running kernel at all. This kernel cannot do that today for
anything:

- There is no ELF loader. `syscall.rs`'s `USER_PROGRAM` is 30 fixed bytes
  of hand-assembled machine code, chosen specifically so its length is
  known at compile time and copied verbatim — it is not loaded,
  relocated, or linked in any general sense.
- There is no relocation/symbol-resolution machinery — a real patch
  (compiled as a separate object/shared object) would need its internal
  and external references fixed up against wherever it actually landed in
  memory and against the running kernel's actual symbol addresses, and
  nothing in this codebase does either.
- `address_space.rs`'s new address space demonstrates *mapping* a fresh
  page real, but mapping is only step one — turning "here are some bytes
  in memory" into "here is a callable function with correctly resolved
  references" is the part live patching actually needs and this kernel
  has no version of at all.

So the honest scope of "live patching" here is really **two** substantial
projects, not one: a real dynamic loader first, then a live-patching
mechanism built on top of it.

## What real live patching requires, once dynamic loading exists

1. **A trampoline/redirection mechanism.** Two real approaches, both used
   by production systems: (a) overwrite the first few bytes of the old
   function with a jump to the new one (what Linux's `kpatch`/`kGraft`
   and Windows hotpatching do), which needs the target function to be
   large enough to hold a jump instruction and needs the overwrite itself
   to be atomic with respect to any CPU currently executing near that
   address; or (b) redirect through an indirect call/function pointer
   table maintained for every patchable function, decided at *build*
   time — simpler to make safe, but requires every patchable call site to
   have been compiled indirectly from the start, which nothing in this
   kernel is today.
2. **The consistency problem**, which is the actual hard part and where
   most real complexity lives: it is never safe to patch a function while
   some CPU (or, on this single-core kernel, some suspended task's saved
   context) has a return address pointing into the *middle* of that
   function's old code, or is executing it right now. Real systems solve
   this via a "consistency model" — kpatch uses stack-trace inspection
   (walk every sleeping task's saved stack, refuse to patch if any return
   address lands inside a to-be-patched function, retry later), kGraft
   uses a per-task "which version" marker with a lazy migration. This
   kernel's own `scheduler_bridge.rs` already keeps enough state (each
   `TaskSlot`'s saved `rsp`) to *make this checkable* in principle, but no
   code inspects a saved stack for this purpose today, and doing so
   safely means genuinely understanding this kernel's exact stack layout
   at every possible preemption point (`context::switch_to`'s pushed
   registers, whatever the task's own call depth looked like) — real,
   detailed work, not a checkbox.
3. **A real ordering/atomicity story with interrupts.** This kernel
   already has hard-won experience (documented in `scheduler_bridge.rs`'s
   own module docs) with how easily a check-then-act sequence breaks when
   interrupts stay live in between — patching a function's entry bytes
   while a timer tick could preempt into the *middle* of that exact write
   is the same class of bug, at a much higher stakes level (a torn write
   to executable code is not a logic error, it's arbitrary code execution
   on whatever partially-written bytes end up there).
4. **Real testing infrastructure this kernel doesn't have**: a live-patch
   mechanism can't be verified the way this repo's other work has been
   (boot, run self-test, read PASS/FAIL, exit) — it needs a *running*
   kernel to apply a patch *to*, mid-execution, which is a different kind
   of test harness than "boot once and check the exit code."

## A real, phased plan (not attempted here)

1. **Build a real, minimal dynamic loader first** — load a second,
   separately-compiled object into a freshly mapped region (this kernel
   already has the primitive for "map a fresh, private region," from
   `address_space.rs`, though a loader wouldn't necessarily need a new
   address space, just new pages in the existing one), perform real
   relocation processing, and call into it successfully. This alone is
   substantial, real, separate work with its own honest verification
   story, unattempted here.
2. **Only then**, build one narrow, safe patching primitive — likely the
   indirect-call-table approach (safer to reason about than in-place
   trampolining) restricted to a small, deliberately chosen set of
   patchable functions decided at build time.
3. **Build the consistency check** against this kernel's actual saved-
   context representation, and prove it catches the case it's supposed
   to (a task suspended inside a to-be-patched function) the same way
   this session proved the guard page and SMEP fault by deliberately
   constructing the failure case and observing the real result.
4. **Only after that**, consider general trampolining/hot-patching
   arbitrary functions — a materially larger and riskier scope.

## Open questions nobody has answered yet

- Does this kernel's eventual use case actually need live patching at
  all, or would a design that accepts brief downtime (stop scheduling,
  apply the patch, resume — much simpler, no consistency-model problem)
  satisfy the real requirement? "No reboot" and "no pause" are different
  asks with very different real costs.
- What's the trust model for a patch itself — is this meant to pair with
  [`SECURE_BOOT.md`](SECURE_BOOT.md)'s signature verification (a live
  patch is, from a security standpoint, exactly as sensitive as the
  original boot image — arguably more so, since it modifies a kernel
  already trusted and running)?
- Given [`RISCV_AARCH64_PORTS.md`](RISCV_AARCH64_PORTS.md) is also
  unimplemented, should a dynamic loader/patcher be designed
  architecture-neutral from the start, or is that premature abstraction
  for a kernel with exactly one real target today?
