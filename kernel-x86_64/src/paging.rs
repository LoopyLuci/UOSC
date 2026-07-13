//! Real x86-64 hardware page tables — CR3, a real 4-level page walk, real
//! `map_to`/TLB-flush calls, not the in-memory `uosc_core::memory::
//! PageTable` policy model standing in for one.
//!
//! `PhysicalAllocator` (from `reference-rs`, already exercised by this
//! kernel's boot self-test) supplies physical frames; this module is what
//! turns "here is a free physical frame" into "here is a virtual address
//! the CPU will actually resolve to it," by writing real page table
//! entries and invalidating the TLB. The bootloader's own physical memory
//! mapping (`BootInfo::physical_memory_offset`, requested via
//! `Mapping::Dynamic` in `main.rs`'s `BOOTLOADER_CONFIG`) is what makes a
//! freshly allocated physical frame (e.g. a new page table's own backing
//! page) reachable at all before anything has mapped it explicitly.

use spin::Mutex;
use uosc_core::memory::{MemoryError, PhysicalAllocator};
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};

use crate::serial_println;

/// The only virtual address range the real page fault handler
/// (`interrupts.rs`) is willing to demand-page. Real demand paging is
/// always scoped to a known region (a VMA, in real-OS terms) — exactly
/// the same principle `uosc_core::memory::ProcessMemoryContext::
/// handle_page_fault` already applies to the portable in-memory model.
/// Anything outside this range still panics: a wild pointer dereference
/// or a real stack overflow must stay a diagnosable fault, not get
/// silently papered over with a freshly mapped page of zeros.
pub const DEMAND_PAGE_REGION_START: u64 = 0x_5555_5555_0000;
pub const DEMAND_PAGE_REGION_END: u64 = DEMAND_PAGE_REGION_START + 4096;

/// The one real page table mapper and the one real physical allocator for
/// this kernel — everything that needs to map a page or hand out a
/// physical frame (the kernel heap, the page fault handler's demand
/// paging, the boot self-test's own allocator check) goes through these
/// same two instances, not a separate scratch copy each.
static MAPPER: Mutex<Option<OffsetPageTable<'static>>> = Mutex::new(None);
static PHYS: Mutex<Option<PhysicalAllocator>> = Mutex::new(None);

/// Reads CR3 and returns a mutable reference to the active level-4 page
/// table, reached through the bootloader's physical-memory mapping. Real
/// hardware state, not a copy — every entry written through this
/// reference is a change the CPU's own page walker will see.
///
/// # Safety
/// Caller must ensure `physical_memory_offset` is the real offset the
/// bootloader mapped all physical memory at, and must not alias this
/// reference (only ever call once and hold the single resulting
/// `OffsetPageTable`).
unsafe fn active_level_4_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    let (level_4_frame, _) = Cr3::read();
    let phys = level_4_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();
    unsafe { &mut *page_table_ptr }
}

/// Builds a real `OffsetPageTable` over the CPU's actual active page
/// table. This is the real hardware equivalent of what `uosc_core::memory
/// ::PageTable` only modeled in memory. Needs no heap allocation, which is
/// exactly why `main.rs` calls this before the kernel heap exists at all.
///
/// # Safety
/// Caller must ensure `physical_memory_offset` is the real offset the
/// bootloader mapped physical memory at, and this must run exactly once.
pub unsafe fn init(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    let level_4_table = unsafe { active_level_4_table(physical_memory_offset) };
    unsafe { OffsetPageTable::new(level_4_table, physical_memory_offset) }
}

/// Stores the real mapper (already built via [`init`]) and constructs the
/// one real, persistent `PhysicalAllocator` this whole kernel shares —
/// managing `[region_start, region_start + total_pages * PAGE_SIZE)`.
///
/// **Must run after the kernel heap already exists.** `PhysicalAllocator::
/// new` allocates on the heap (its free-list bookkeeping is `Vec`-backed)
/// — this was a real bug the first time this kernel tried real paging: a
/// genuine `memory allocation of 32 bytes failed` panic from calling this
/// before any heap existed. `allocator::init_bootstrap` (which itself only
/// needs [`init`] and a heap-free [`BumpFrameAllocator`]) is what breaks
/// that circularity; see `allocator.rs`'s module docs for the full story.
pub fn store_globals(mapper: OffsetPageTable<'static>, region_start: u64, total_pages: u64) {
    let phys = PhysicalAllocator::new(region_start, total_pages);
    *MAPPER.lock() = Some(mapper);
    *PHYS.lock() = Some(phys);
}

/// Locked access to the real physical allocator alone (used where no
/// mapping is needed, just accounting — e.g. the boot self-test's
/// alloc/dealloc round-trip check).
pub fn with_phys<R>(f: impl FnOnce(&mut PhysicalAllocator) -> R) -> Option<R> {
    PHYS.lock().as_mut().map(f)
}

/// Locked access to both the real mapper and the real physical allocator
/// together — needed for anything that actually maps a page, since
/// `map_page` needs both at once.
pub fn with_mapper_and_phys<R>(f: impl FnOnce(&mut OffsetPageTable<'static>, &mut PhysicalAllocator) -> R) -> Option<R> {
    let mut mapper_guard = MAPPER.lock();
    let mut phys_guard = PHYS.lock();
    match (mapper_guard.as_mut(), phys_guard.as_mut()) {
        (Some(m), Some(p)) => Some(f(m, p)),
        _ => None,
    }
}

/// Adapts the real, already-tested `uosc_core::memory::PhysicalAllocator`
/// buddy allocator to the `x86_64` crate's [`FrameAllocator`] trait, so
/// the exact same physical-page bookkeeping this kernel's boot self-test
/// already exercises is what backs real page table entries too — not a
/// second, parallel physical allocator.
pub struct PhysicalAllocatorAdapter<'a> {
    pub phys: &'a mut PhysicalAllocator,
}

unsafe impl FrameAllocator<Size4KiB> for PhysicalAllocatorAdapter<'_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let addr = self.phys.allocate(0).ok()?;
        Some(PhysFrame::containing_address(PhysAddr::new(addr)))
    }
}

/// A trivial, heap-free physical frame source: hands out consecutive real
/// physical frames starting at `start`, with no bookkeeping beyond a
/// counter. Exists for exactly one purpose — mapping the first few pages
/// of the kernel heap *before* the real `PhysicalAllocator` can exist,
/// since constructing that allocator itself needs a working heap (see
/// [`store_globals`]'s docs). Never freed, never reused: the handful of
/// frames this consumes are permanently excluded from the region handed
/// to the real `PhysicalAllocator` afterward, by construction (`main.rs`
/// starts that allocator's region *after* the bytes this consumed).
pub struct BumpFrameAllocator {
    next: u64,
}

impl BumpFrameAllocator {
    pub fn new(start: u64) -> Self {
        BumpFrameAllocator { next: start }
    }
}

unsafe impl FrameAllocator<Size4KiB> for BumpFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let frame = PhysFrame::containing_address(PhysAddr::new(self.next));
        self.next += uosc_core::memory::PAGE_SIZE;
        Some(frame)
    }
}

/// Maps one real 4 KiB page using any real frame source — the shared
/// `PhysicalAllocator` via [`PhysicalAllocatorAdapter`] ([`map_page`]) or,
/// during heap bootstrap, a [`BumpFrameAllocator`] directly.
pub fn map_page_with(
    mapper: &mut OffsetPageTable<'static>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    vaddr: VirtAddr,
    flags: PageTableFlags,
) -> Result<PhysFrame<Size4KiB>, MemoryError> {
    let page: Page<Size4KiB> = Page::containing_address(vaddr);
    let frame = frame_allocator.allocate_frame().ok_or(MemoryError::OutOfMemory)?;
    match unsafe { mapper.map_to(page, frame, flags, frame_allocator) } {
        Ok(flush) => {
            flush.flush();
            Ok(frame)
        }
        Err(e) => {
            serial_println!("[paging] map_to({:#x}) failed: {:?}", vaddr.as_u64(), e);
            Err(MemoryError::AccessViolation)
        }
    }
}

/// [`map_page_with`], specialized to the shared `PhysicalAllocator` — the
/// common case once the heap (and therefore that allocator) exists.
pub fn map_page(
    mapper: &mut OffsetPageTable<'static>,
    phys: &mut PhysicalAllocator,
    vaddr: VirtAddr,
    flags: PageTableFlags,
) -> Result<PhysFrame<Size4KiB>, MemoryError> {
    let mut allocator = PhysicalAllocatorAdapter { phys };
    map_page_with(mapper, &mut allocator, vaddr, flags)
}

/// The real counterpart to [`map_page`]: removes the real page table entry
/// (a real `mapper.unmap` + TLB flush, not just an accounting fiction) and
/// returns the real physical frame it was backed by to the shared
/// `PhysicalAllocator`, so it's genuinely available for the next
/// `allocate()` again — not leaked, not double-owned. Any subsequent
/// access to `vaddr` after this returns really does fault: the hardware
/// page walker has nothing to find there any more.
pub fn unmap_page(
    mapper: &mut OffsetPageTable<'static>,
    phys: &mut PhysicalAllocator,
    vaddr: VirtAddr,
) -> Result<(), MemoryError> {
    let page: Page<Size4KiB> = Page::containing_address(vaddr);
    match mapper.unmap(page) {
        Ok((frame, flush)) => {
            flush.flush();
            phys.deallocate(frame.start_address().as_u64())
        }
        Err(e) => {
            serial_println!("[paging] unmap({:#x}) failed: {:?}", vaddr.as_u64(), e);
            Err(MemoryError::AccessViolation)
        }
    }
}
