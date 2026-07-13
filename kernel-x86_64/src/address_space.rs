//! Real multiple address spaces — a second, genuinely independent,
//! CR3-loadable top-level page table, not a second region carved out of
//! the same one. Until now every check in this kernel (`paging.rs`'s
//! `map_page`/`unmap_page`, the demand-paged region, guarded task stacks)
//! shared the single `OffsetPageTable` built once at boot over the CPU's
//! actual CR3. This module builds a second one, switches the CPU to it
//! for real (a real `CR3` write, not a simulation), and proves it's
//! genuinely independent rather than an alias of the first.
//!
//! **The construction**: [`AddressSpace::new`] allocates one fresh
//! physical frame (from the same shared `PhysicalAllocator` everything
//! else in this kernel uses) to hold a brand-new level-4 table, then
//! clones every one of the *current* active L4 table's 512 entries into
//! it. Since an L4 entry is just a pointer to an L3 table, cloning the
//! entries — not the tables they point to — means the new address space
//! shares every existing mapping (kernel code, the heap, the demand-page
//! region, guarded task stacks) with the original: nothing that already
//! worked stops working after a real switch to this new table. Only then
//! does [`AddressSpace::new`] map one real, fresh page at
//! [`PRIVATE_REGION_ADDR`] — via a throwaway `OffsetPageTable` view over
//! *this specific* new L4 frame, not the shared global mapper, which
//! still targets the original — so that one mapping exists in the new
//! address space and nowhere else.
//!
//! **Real isolation, verified directly, not asserted.** The boot self-test
//! ([`main.rs`]) checks, before ever switching: the original/active L4
//! table's entry for `PRIVATE_REGION_ADDR` is genuinely absent (`
//! is_unused()`) — proof the clone didn't somehow leak the new mapping
//! backward. Then it performs a real `CR3` write to the new table, reads
//! straight through the real virtual address (no physical-offset back
//! door — this only resolves at all because the hardware page walker is
//! now actually using the new table), confirms the expected value, and
//! writes a real `CR3` write back to restore the original table exactly.
//! Every check `main.rs` runs *after* this one (Scheduler, TaskExit,
//! TaskReuse, Sanctum, Ipc) still passing is itself real, load-bearing
//! evidence the restore was exact — a subtly wrong restore (a stale TLB
//! entry, a wrong frame, wrong flags) would have surfaced as later
//! mappings silently misbehaving, not a clean panic.
//!
//! **Scope, stated plainly.** This is a real second address space, but a
//! narrow one: it shares literally everything except one deliberately
//! added private page — there is no process abstraction around it (no
//! PID, no scheduler awareness, nothing yet ties a `RunQueue` task to a
//! particular `AddressSpace`), no copy-on-write, no separate user/kernel
//! split (this kernel still runs everything at CPL0 in both address
//! spaces), and the switch back is done by hand in the same function that
//! switched away, not by any general "switch address space on context
//! switch" mechanism. A real second CR3 that a task could be scheduled
//! into automatically is real, separate, follow-on work.

use spin::Mutex;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB};
use x86_64::{PhysAddr, VirtAddr};

use crate::paging::{self, PhysicalAllocatorAdapter};

/// A distinct, canonical (nibble `<= 0x7`, matching every other fixed
/// region in this kernel — see `task_stack.rs`'s docs for why that matters)
/// virtual address that this pass adds a real mapping for *only* in a new
/// address space, never in the original/boot one — the one thing that
/// differs between the two, and therefore the one thing that proves
/// they're genuinely independent rather than the same table twice.
pub const PRIVATE_REGION_ADDR: u64 = 0x_1111_1111_0000;

/// A recognizable value with no plausible accidental origin (not a byte
/// pattern the demand-page/unmap checks or a stray zeroed page could ever
/// produce), written into the private page so a successful read-back
/// really proves the new mapping resolved, not that a coincidentally
/// zeroed/reused page just happened to satisfy a weaker check.
pub const PRIVATE_VALUE: u64 = 0xC0FFEE_C0FFEE_u64;

static ACTIVE_SWITCH_LOCK: Mutex<()> = Mutex::new(());

pub struct AddressSpace {
    l4_frame: PhysFrame<Size4KiB>,
}

impl AddressSpace {
    /// Builds a real second address space: a fresh L4 frame, cloned from
    /// the currently-active one (see module docs for why that's safe and
    /// what it shares), with one additional real, private mapping only
    /// this new table has. `None` on any real allocation/mapping failure
    /// (out of physical memory, or `paging::init`/`store_globals` not
    /// having run yet).
    pub fn new() -> Option<AddressSpace> {
        let phys_offset = paging::phys_mem_offset()?;

        // 1. A fresh physical frame from the same shared allocator
        // everything else in this kernel uses, to hold the new L4 table.
        let new_l4_phys = paging::with_phys(|phys| phys.allocate(0))?.ok()?;
        let new_l4_frame = PhysFrame::containing_address(PhysAddr::new(new_l4_phys));
        let new_l4_virt = phys_offset + new_l4_phys;
        // Safety: new_l4_phys was just allocated from the shared
        // PhysicalAllocator (so nothing else owns it), and phys_offset
        // maps every real physical frame, including this one, reachably —
        // the same reasoning `paging::active_level_4_table` already
        // relies on for the boot-time L4 frame.
        let new_l4_table: &mut PageTable = unsafe { &mut *(new_l4_virt.as_mut_ptr()) };
        new_l4_table.zero();

        // 2. Clone every entry (not the tables they point to) from the
        // currently-active L4 table — see module docs for why this keeps
        // every existing mapping working identically in the new table.
        {
            // Safety: only read from the active table here (never
            // written), and the borrow ends before this block does.
            let active = unsafe { paging::active_level_4_table(phys_offset) };
            for i in 0..512 {
                new_l4_table[i] = active[i].clone();
            }
        }

        // 3. Map one real, fresh page at PRIVATE_REGION_ADDR *only* in the
        // new table, via a throwaway OffsetPageTable view over this
        // specific new L4 frame — the shared global MAPPER still targets
        // the original/boot L4 and must not be used here.
        let mut new_mapper = unsafe { OffsetPageTable::new(new_l4_table, phys_offset) };
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | crate::cpu_features::nx_flag();
        let new_frame = paging::with_phys(|phys| {
            let mut allocator = PhysicalAllocatorAdapter { phys };
            paging::map_page_with(&mut new_mapper, &mut allocator, VirtAddr::new(PRIVATE_REGION_ADDR), flags)
        })?
        .ok()?;

        // Write PRIVATE_VALUE through the physical-offset back door —
        // reachable right now regardless of which CR3 is currently
        // loaded, the same trick every frame allocation in this kernel
        // already relies on. This runs *before* any real CR3 switch, so
        // it never depends on the new table actually being active yet.
        let frame_virt = phys_offset + new_frame.start_address().as_u64();
        unsafe {
            (frame_virt.as_mut_ptr::<u64>()).write_volatile(PRIVATE_VALUE);
        }

        Some(AddressSpace { l4_frame: new_l4_frame })
    }

    /// A real `CR3` write to this address space's L4 frame — the CPU's
    /// page walker genuinely starts resolving every subsequent memory
    /// access through this table from this instruction onward. Returns
    /// the frame that was active immediately before, so the caller can
    /// restore it exactly via [`restore`].
    ///
    /// # Safety
    /// Every mapping the caller depends on after this call (kernel code,
    /// the current stack, the IDT/GDT) must resolve identically in this
    /// address space — true here because [`AddressSpace::new`] cloned
    /// every entry from the table active at construction time, but not
    /// true in general for an arbitrary `AddressSpace`.
    pub unsafe fn switch_to(&self) -> PhysFrame<Size4KiB> {
        let (prev_frame, flags) = Cr3::read();
        unsafe {
            Cr3::write(self.l4_frame, flags);
        }
        prev_frame
    }
}

/// A real `CR3` write back to a previously-active frame — the exact
/// counterpart to [`AddressSpace::switch_to`]. Takes whatever
/// `Cr3::read()` reported as the flags at the moment of the call, same as
/// `switch_to` does, since this kernel never varies `Cr3Flags` between
/// address spaces.
///
/// # Safety
/// `frame` must be a real, still-valid L4 frame (in practice, the value
/// [`AddressSpace::switch_to`] returned).
pub unsafe fn restore(frame: PhysFrame<Size4KiB>) {
    let (_, flags) = Cr3::read();
    unsafe {
        Cr3::write(frame, flags);
    }
}

/// Real, direct proof the clone in [`AddressSpace::new`] didn't leak
/// [`PRIVATE_REGION_ADDR`] backward into the table active *before* any
/// new address space was ever created — checks the real L4 entry
/// (`is_unused()`, a genuine page-table-entry read, not a convention) for
/// the currently-active table.
pub fn private_region_absent_from_active() -> bool {
    let Some(phys_offset) = paging::phys_mem_offset() else { return false };
    let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(PRIVATE_REGION_ADDR));
    let index = usize::from(page.p4_index());
    // Safety: read-only inspection of one entry, never written.
    let active = unsafe { paging::active_level_4_table(phys_offset) };
    active[index].is_unused()
}

/// Serializes real `CR3` switches so two callers can never interleave a
/// switch-away/switch-back pair — not needed today (only the boot
/// self-test ever calls this, before real preemption's tick budget would
/// let anything else run concurrently), but a real second caller racing
/// this one-shot demo would be a genuine bug, not a hypothetical one, so
/// the lock exists rather than relying on that timing coincidence.
pub fn with_lock<R>(f: impl FnOnce() -> R) -> R {
    let _guard = ACTIVE_SWITCH_LOCK.lock();
    f()
}
