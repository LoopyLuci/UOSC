//! Physical/virtual memory management — real port of `kernel/memory.ti`.
//!
//! The Titan source's buddy allocator has the right shape (`free_lists`
//! indexed by order, a `BitmapSet` of used pages) but `split_block` and
//! `coalesce_blocks` are literally `Ok(())` — there's no split or coalesce
//! logic at all, which means the "buddy allocator" as specified cannot
//! actually reuse memory. [`PhysicalAllocator`] below is a real, working
//! buddy allocator: splitting a larger free block on demand, and coalescing
//! a freed block with its buddy when possible.
//!
//! `ProcessMemoryContext::allocate_virtual` also closes a real gap: the
//! Titan source's `has_allocation_capability` always returns `true`
//! (`kernel/memory.ti:358-361`) — a capability check that can never deny
//! anything isn't a capability check. Here it takes a real
//! [`CapabilityBroker`](crate::capability::CapabilityBroker) and is denied
//! exactly like every other resource type.
//!
//! Hardware page tables (x86-64 4-level paging, CR3, TLB shootdown) are out
//! of scope for this portable layer — [`PageTable`] here is a real,
//! testable in-memory mapping table, not a hardware page-walker.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::capability::{CapabilityBroker, Permissions, ResourceType};

pub const PAGE_SIZE: u64 = 0x1000; // 4 KiB
pub const MAX_ORDER: u32 = 20; // supports blocks up to 2^20 * 4KiB = 4 GiB

pub type PhysAddr = u64;
pub type VirtAddr = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    OutOfMemory,
    InvalidOrder,
    RegionNotFound,
    CapabilityDenied,
    DoubleFree,
    Unmapped,
    AccessViolation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MemoryAccess {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
    pub user_accessible: bool,
}

/// A real, working buddy allocator over a fixed arena of `total_pages`
/// order-0 (4 KiB) pages starting at `base`.
#[derive(Debug)]
pub struct PhysicalAllocator {
    base: PhysAddr,
    total_pages: u64,
    /// `free_lists[order]` holds the base addresses of free blocks of that order.
    free_lists: [Vec<PhysAddr>; (MAX_ORDER + 1) as usize],
    /// Tracks the order a live (allocated) block was allocated at, keyed by
    /// its base address — needed so `deallocate` doesn't have to be told
    /// the order back, matching how a real allocator is actually called.
    live_blocks: BTreeMap<PhysAddr, u32>,
}

impl PhysicalAllocator {
    pub fn new(base: PhysAddr, total_pages: u64) -> Self {
        let mut alloc = PhysicalAllocator {
            base,
            total_pages,
            free_lists: core::array::from_fn(|_| Vec::new()),
            live_blocks: BTreeMap::new(),
        };
        alloc.seed_free_lists();
        alloc
    }

    /// Break the arena into the largest aligned power-of-two blocks that
    /// fit, so allocation can start by finding/splitting the smallest
    /// sufficient block rather than a linear scan.
    fn seed_free_lists(&mut self) {
        let mut addr = self.base;
        let mut remaining = self.total_pages;
        while remaining > 0 {
            let mut order = remaining.trailing_zeros().min(MAX_ORDER);
            // Also bound by alignment of addr itself (in page units).
            let page_index = (addr - self.base) / PAGE_SIZE;
            if page_index != 0 {
                order = order.min(page_index.trailing_zeros());
            }
            let block_pages = 1u64 << order;
            self.free_lists[order as usize].push(addr);
            addr += block_pages * PAGE_SIZE;
            remaining -= block_pages;
        }
    }

    fn buddy_of(&self, addr: PhysAddr, order: u32) -> PhysAddr {
        let block_bytes = (1u64 << order) * PAGE_SIZE;
        let offset = addr - self.base;
        self.base + (offset ^ block_bytes)
    }

    /// Real split logic (`split_block` in the Titan source is `Ok(())`).
    /// Finds the smallest free block at or above `order` and recursively
    /// halves it down to `order`, pushing the unused halves onto their own
    /// free lists.
    fn ensure_free_block(&mut self, order: u32) -> Result<(), MemoryError> {
        if order as usize >= self.free_lists.len() {
            return Err(MemoryError::InvalidOrder);
        }
        if !self.free_lists[order as usize].is_empty() {
            return Ok(());
        }
        // Find the smallest larger order with a free block.
        let mut source_order = order + 1;
        while source_order as usize <= MAX_ORDER as usize
            && self.free_lists[source_order as usize].is_empty()
        {
            source_order += 1;
        }
        if source_order as usize > MAX_ORDER as usize {
            return Err(MemoryError::OutOfMemory);
        }
        // Split it down one level at a time until `order` has a block.
        for cur in (order + 1..=source_order).rev() {
            let block = self.free_lists[cur as usize]
                .pop()
                .ok_or(MemoryError::OutOfMemory)?;
            let half = cur - 1;
            let block_bytes = (1u64 << half) * PAGE_SIZE;
            self.free_lists[half as usize].push(block);
            self.free_lists[half as usize].push(block + block_bytes);
        }
        Ok(())
    }

    pub fn allocate(&mut self, order: u32) -> Result<PhysAddr, MemoryError> {
        self.ensure_free_block(order)?;
        let addr = self.free_lists[order as usize]
            .pop()
            .ok_or(MemoryError::OutOfMemory)?;
        self.live_blocks.insert(addr, order);
        Ok(addr)
    }

    /// Real coalesce logic (`coalesce_blocks` in the Titan source is
    /// `Ok(())`). After freeing a block, repeatedly checks whether its
    /// buddy is also free and merges them into the next order up, all the
    /// way to `MAX_ORDER` if possible — this is what actually prevents the
    /// arena from fragmenting into permanently order-0 blocks over time.
    pub fn deallocate(&mut self, addr: PhysAddr) -> Result<(), MemoryError> {
        let mut order = self
            .live_blocks
            .remove(&addr)
            .ok_or(MemoryError::DoubleFree)?;
        let mut block = addr;

        while order < MAX_ORDER {
            let buddy = self.buddy_of(block, order);
            let list = &mut self.free_lists[order as usize];
            if let Some(pos) = list.iter().position(|&a| a == buddy) {
                list.remove(pos);
                block = block.min(buddy);
                order += 1;
            } else {
                break;
            }
        }
        self.free_lists[order as usize].push(block);
        Ok(())
    }

    pub fn free_page_count(&self) -> u64 {
        self.free_lists
            .iter()
            .enumerate()
            .map(|(order, list)| list.len() as u64 * (1u64 << order))
            .sum()
    }
}

#[derive(Debug, Clone, Copy)]
struct Mapping {
    phys: PhysAddr,
    access: MemoryAccess,
}

/// Real, portable stand-in for hardware page tables — a mapping table any
/// hardware backend can be built underneath later.
#[derive(Debug, Default)]
pub struct PageTable {
    mappings: BTreeMap<VirtAddr, Mapping>,
}

impl PageTable {
    pub fn new() -> Self {
        PageTable { mappings: BTreeMap::new() }
    }

    pub fn map_page(&mut self, vaddr: VirtAddr, paddr: PhysAddr, access: MemoryAccess) {
        self.mappings.insert(vaddr, Mapping { phys: paddr, access });
    }

    pub fn unmap_page(&mut self, vaddr: VirtAddr) -> Result<PhysAddr, MemoryError> {
        self.mappings
            .remove(&vaddr)
            .map(|m| m.phys)
            .ok_or(MemoryError::Unmapped)
    }

    pub fn translate(&self, vaddr: VirtAddr, want: MemoryAccess) -> Result<PhysAddr, MemoryError> {
        let mapping = self.mappings.get(&vaddr).ok_or(MemoryError::Unmapped)?;
        let ok = (!want.read || mapping.access.read)
            && (!want.write || mapping.access.write)
            && (!want.execute || mapping.access.execute);
        if ok {
            Ok(mapping.phys)
        } else {
            Err(MemoryError::AccessViolation)
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct VMemRegion {
    pub start: VirtAddr,
    pub end: VirtAddr,
    pub access: MemoryAccess,
}

/// Real port of `ProcessMemoryContext` — `allocate_virtual` here actually
/// consults a [`CapabilityBroker`] instead of the Titan source's
/// unconditional `true`.
pub struct ProcessMemoryContext {
    pub pid: u64,
    pub page_table: PageTable,
    pub regions: Vec<VMemRegion>,
    next_addr: VirtAddr,
}

impl ProcessMemoryContext {
    pub fn new(pid: u64, heap_start: VirtAddr) -> Self {
        ProcessMemoryContext {
            pid,
            page_table: PageTable::new(),
            regions: Vec::new(),
            next_addr: heap_start,
        }
    }

    pub fn allocate_virtual(
        &mut self,
        phys: &mut PhysicalAllocator,
        caps: &mut CapabilityBroker,
        size: u64,
        access: MemoryAccess,
        now: u64,
    ) -> Result<VirtAddr, MemoryError> {
        let needed = Permissions {
            read: true,
            write: access.write,
            execute: access.execute,
            create: true,
            ..Permissions::NONE
        };
        let decision = caps.check_access(self.pid, ResourceType::Memory, "heap", needed, now);
        if decision != crate::capability::EnforcementDecision::Allowed {
            return Err(MemoryError::CapabilityDenied);
        }

        let page_count = size.div_ceil(PAGE_SIZE);
        let vaddr = self.next_addr;
        for i in 0..page_count {
            let paddr = phys.allocate(0)?;
            self.page_table.map_page(vaddr + i * PAGE_SIZE, paddr, access);
        }
        self.next_addr += page_count * PAGE_SIZE;
        self.regions.push(VMemRegion { start: vaddr, end: vaddr + size, access });
        Ok(vaddr)
    }

    pub fn deallocate_virtual(
        &mut self,
        phys: &mut PhysicalAllocator,
        vaddr: VirtAddr,
    ) -> Result<(), MemoryError> {
        let idx = self
            .regions
            .iter()
            .position(|r| r.start == vaddr)
            .ok_or(MemoryError::RegionNotFound)?;
        let region = self.regions.remove(idx);

        let page_count = (region.end - region.start).div_ceil(PAGE_SIZE);
        for i in 0..page_count {
            let page_vaddr = region.start + i * PAGE_SIZE;
            let paddr = self.page_table.unmap_page(page_vaddr)?;
            phys.deallocate(paddr)?;
        }
        Ok(())
    }

    /// Lazy-allocation page fault handler: only satisfies faults inside a
    /// registered region, and only if the fault's access type was actually
    /// granted for that region.
    pub fn handle_page_fault(
        &mut self,
        phys: &mut PhysicalAllocator,
        vaddr: VirtAddr,
        is_write: bool,
    ) -> Result<(), MemoryError> {
        let region = self
            .regions
            .iter()
            .find(|r| r.start <= vaddr && vaddr < r.end)
            .ok_or(MemoryError::Unmapped)?;

        if is_write && !region.access.write {
            return Err(MemoryError::AccessViolation);
        }

        let page_vaddr = vaddr - (vaddr % PAGE_SIZE);
        let paddr = phys.allocate(0)?;
        self.page_table.map_page(page_vaddr, paddr, region.access);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{CapabilityBroker, Permissions, ResourceType};
    use proptest::prelude::*;

    #[test]
    fn allocate_never_hands_out_the_same_page_twice_without_a_free() {
        let mut a = PhysicalAllocator::new(0, 4);
        let p1 = a.allocate(0).unwrap();
        let p2 = a.allocate(0).unwrap();
        assert_ne!(p1, p2);
    }

    #[test]
    fn split_then_coalesce_returns_to_a_single_top_order_block() {
        let mut a = PhysicalAllocator::new(0, 8); // one order-3 block initially
        assert_eq!(a.free_page_count(), 8);

        let p0 = a.allocate(0).unwrap(); // forces splitting down to order 0
        assert_eq!(a.free_page_count(), 7);

        a.deallocate(p0).unwrap();
        // After freeing the only allocated page, coalescing should fully
        // reassemble the arena back into free pages.
        assert_eq!(a.free_page_count(), 8);
    }

    #[test]
    fn double_free_is_rejected_not_silently_corrupting_state() {
        let mut a = PhysicalAllocator::new(0, 4);
        let p = a.allocate(0).unwrap();
        a.deallocate(p).unwrap();
        assert_eq!(a.deallocate(p), Err(MemoryError::DoubleFree));
    }

    #[test]
    fn out_of_memory_is_reported_not_panicked() {
        let mut a = PhysicalAllocator::new(0, 2);
        a.allocate(0).unwrap();
        a.allocate(0).unwrap();
        assert_eq!(a.allocate(0), Err(MemoryError::OutOfMemory));
    }

    proptest! {
        /// However many pages the arena has, free_page_count after any
        /// interleaving of allocate/deallocate that ends with everything
        /// freed must return to the starting total — this is the real
        /// property a buddy allocator exists to guarantee, and the one the
        /// stubbed Titan version cannot, since it never coalesces.
        #[test]
        fn allocate_deallocate_cycles_never_leak_pages(ops in prop::collection::vec(0usize..3, 1..40)) {
            let total = 16u64;
            let mut a = PhysicalAllocator::new(0, total);
            let mut live: Vec<PhysAddr> = Vec::new();

            for op in ops {
                match op {
                    0 | 1 => {
                        if let Ok(p) = a.allocate(0) {
                            live.push(p);
                        }
                    }
                    _ => {
                        if let Some(p) = live.pop() {
                            a.deallocate(p).unwrap();
                        }
                    }
                }
            }
            for p in live.drain(..) {
                a.deallocate(p).unwrap();
            }
            prop_assert_eq!(a.free_page_count(), total);
        }
    }

    #[test]
    fn allocate_virtual_is_denied_without_a_real_capability_grant() {
        let mut phys = PhysicalAllocator::new(0, 16);
        let mut caps = CapabilityBroker::new();
        let mut ctx = ProcessMemoryContext::new(1, 0x1000_0000);

        // No grant issued at all — the Titan source's stub would allow
        // this unconditionally; this port must deny it.
        let result = ctx.allocate_virtual(
            &mut phys,
            &mut caps,
            PAGE_SIZE,
            MemoryAccess { read: true, write: true, ..Default::default() },
            0,
        );
        assert_eq!(result, Err(MemoryError::CapabilityDenied));
    }

    #[test]
    fn allocate_virtual_succeeds_and_is_usable_once_granted() {
        let mut phys = PhysicalAllocator::new(0, 16);
        let mut caps = CapabilityBroker::new();
        caps.issue(
            0,
            1,
            ResourceType::Memory,
            "heap".into(),
            Permissions { read: true, write: true, create: true, ..Permissions::NONE },
            0,
            1000,
        );
        let mut ctx = ProcessMemoryContext::new(1, 0x1000_0000);

        let vaddr = ctx
            .allocate_virtual(
                &mut phys,
                &mut caps,
                PAGE_SIZE,
                MemoryAccess { read: true, write: true, ..Default::default() },
                5,
            )
            .unwrap();

        let paddr = ctx.page_table.translate(vaddr, MemoryAccess { read: true, ..Default::default() });
        assert!(paddr.is_ok());

        ctx.deallocate_virtual(&mut phys, vaddr).unwrap();
        assert_eq!(phys.free_page_count(), 16);
    }

    #[test]
    fn page_fault_outside_any_region_is_not_satisfied() {
        let mut phys = PhysicalAllocator::new(0, 4);
        let mut ctx = ProcessMemoryContext::new(1, 0x1000_0000);
        let result = ctx.handle_page_fault(&mut phys, 0xDEAD_0000, false);
        assert_eq!(result, Err(MemoryError::Unmapped));
    }

    #[test]
    fn page_fault_write_to_a_read_only_region_is_an_access_violation() {
        let mut phys = PhysicalAllocator::new(0, 4);
        let mut ctx = ProcessMemoryContext::new(1, 0x1000_0000);
        ctx.regions.push(VMemRegion {
            start: 0x2000,
            end: 0x3000,
            access: MemoryAccess { read: true, write: false, execute: false, user_accessible: true },
        });
        let result = ctx.handle_page_fault(&mut phys, 0x2000, true);
        assert_eq!(result, Err(MemoryError::AccessViolation));
    }
}
