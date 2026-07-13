//! Real guard-page-protected task stacks — closes a gap `scheduler_bridge.rs`'s
//! module docs deliberately left open: the earlier `Box<[u8]>`-backed task
//! stacks had **no guard page**, and thin heap headroom under real
//! interrupt nesting was the leading (never debugger-confirmed) suspect for
//! a rare, real total hang. This module gives each task-pool slot a fixed,
//! dedicated virtual address range with a genuinely **unmapped** page
//! directly below it, so a real stack overflow now produces a real,
//! diagnosable `#PF` panic via `interrupts.rs`'s existing
//! `page_fault_handler` (anything outside the demand-page region is a real,
//! unrecoverable fault there, by design) instead of silently corrupting
//! whichever heap allocation happened to sit next to the old `Box`.
//!
//! **Reused, not reclaimed.** The task pool is already a fixed-size
//! resource (`MAX_SLOTS`), so each slot's real pages are mapped once, the
//! first time that slot is ever used, and left mapped for the life of the
//! kernel — a future task claiming that same slot just reuses the same real
//! physical frames, the same way it always reused the same `TaskSlot` array
//! entry. The old stack's bytes are never zeroed on reuse, which is fine:
//! nothing can read a predecessor's stale bytes without first writing its
//! own (identical reasoning to reusing any freed heap allocation without
//! zeroing it). This does mean the previous `TaskStack`/`Drop`-counter proof
//! ("a freed slot's stack was really deallocated") no longer applies — see
//! `scheduler_bridge::task_reused_a_guarded_slot` for what replaces it: a
//! direct check that task_d really landed in the exact same guarded slot
//! task_c's stack used, not merely *some* slot.
//!
//! **Not exercised by an actual overflow in the checked-in boot self-test,
//! unlike the demand-page region's fault-and-recover check.** There is no
//! recovery path for a fault outside the demand-page region — that's the
//! whole point of a guard page — so deliberately overflowing into one here
//! would correctly end the boot with a real panic, not a passing check.
//! This was verified manually instead, the same way the SMAP fix was: by
//! temporarily making `task_d_entry` recurse past its real 64 KiB budget
//! and observing the result, then reverting.
//!
//! **The real result was a `DOUBLE FAULT`, not a plain `PAGE FAULT`** — a
//! genuinely useful, non-obvious finding, not what was originally
//! expected. Once the stack pointer wanders into the unmapped guard page,
//! the CPU's attempt to push the `#PAGE FAULT` exception frame onto that
//! same, already-faulting stack pointer itself faults again, which
//! escalates to a double fault. That's real and expected — the same
//! reason real kernels (this one included, see `gdt.rs`'s
//! `DOUBLE_FAULT_IST_INDEX`) always route the double-fault handler through
//! a dedicated IST stack rather than the faulting task's own: the
//! overflowing task's stack can't be trusted to have room for *any*
//! further exception frame, and a double fault is exactly the CPU's own
//! escalation path for "the fault handler's attempt to handle the first
//! fault itself faulted." The captured panic's `stack_pointer` field
//! (`0x333333352ea0`, for a slot whose guard page starts at
//! `0x333333352000` and whose real stack starts at `0x333333353000`) is
//! itself real, direct evidence the overflow reached into the guard page
//! specifically, not merely somewhere else. See `STATUS.md` for the exact
//! captured output.

use spin::Mutex;
use uosc_core::memory::PAGE_SIZE;
use x86_64::VirtAddr;
use x86_64::structures::paging::PageTableFlags;

/// The fixed number of guarded stack slots — one per task-pool slot.
/// `scheduler_bridge::MAX_TASKS` is defined in terms of this, not the other
/// way around: the guarded virtual region is what actually bounds how many
/// concurrently-alive tasks can exist now.
pub const MAX_SLOTS: usize = 8;

/// Real per-task stack size — the same real budget the heap-allocated
/// stacks used after the earlier mitigation, just backed by real page
/// tables instead of the heap now.
pub const STACK_SIZE: u64 = 64 * 1024;

const STACK_PAGES: u64 = STACK_SIZE / PAGE_SIZE;

/// One real, deliberately-unmapped guard page below every stack.
const GUARD_PAGES: u64 = 1;

const SLOT_STRIDE: u64 = (GUARD_PAGES + STACK_PAGES) * PAGE_SIZE;

/// Chosen distinct from every other fixed region this kernel already uses:
/// `allocator::HEAP_BASE` (`0x4444...`), `paging::DEMAND_PAGE_REGION_START`
/// (`0x5555...`), and the ring-3 demo's `USER_CODE_ADDR`/`USER_STACK_ADDR`
/// (`0x6666.../0x7777...`, see `syscall.rs`) — a collision would silently
/// corrupt something else's page table entries instead of failing loudly.
///
/// **A real bug, caught on the very first boot after this module was
/// written**: an earlier version of this constant used `0x_8888_8888_0000`,
/// which is not a canonical x86-64 address — bits 63:47 must all be equal
/// (a sign-extension of bit 47), and a leading `0x8` nibble sets bit 47 to
/// 1 while bits 63:48 stay 0, tripping a real, immediate panic from the
/// `x86_64` crate's own `VirtAddr::new` the first time `spawn_task` tried
/// to map a guarded stack: `"virtual address must be sign extended in bits
/// 48 to 64"`. Every other fixed region in this file uses a nibble `<=
/// 0x7` for exactly this reason; this one now does too.
const REGION_BASE: u64 = 0x_3333_3333_0000;

fn slot_guard_page_addr(slot: usize) -> u64 {
    REGION_BASE + slot as u64 * SLOT_STRIDE
}

fn slot_stack_base(slot: usize) -> u64 {
    slot_guard_page_addr(slot) + PAGE_SIZE
}

/// Which slots have had their real pages mapped at least once — mapping is
/// idempotent per slot, deferred to first use for simplicity (by the time
/// any slot is actually needed, `paging::store_globals` has already run).
static MAPPED: Mutex<[bool; MAX_SLOTS]> = Mutex::new([false; MAX_SLOTS]);

/// Returns the fixed top-of-stack address for `slot`, mapping its real
/// pages the first time this slot is used. The page immediately below
/// (`slot_guard_page_addr`) is deliberately never mapped by this function —
/// that omission *is* the guard page.
pub fn ensure_slot_mapped(slot: usize) -> u64 {
    let stack_base = slot_stack_base(slot);
    let mut mapped = MAPPED.lock();
    if !mapped[slot] {
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | crate::cpu_features::nx_flag();
        for i in 0..STACK_PAGES {
            let vaddr = VirtAddr::new(stack_base + i * PAGE_SIZE);
            crate::paging::with_mapper_and_phys(|mapper, phys| crate::paging::map_page(mapper, phys, vaddr, flags))
                .expect("paging::store_globals must run before any task_stack::ensure_slot_mapped")
                .expect("failed to map a real physical page for a guarded task stack");
        }
        mapped[slot] = true;
        crate::serial_println!(
            "[task_stack] slot {slot}: mapped a real {} KiB guarded stack at {:#x}, real unmapped guard page at {:#x}",
            STACK_SIZE / 1024,
            stack_base,
            slot_guard_page_addr(slot)
        );
    }
    stack_base + STACK_SIZE
}
