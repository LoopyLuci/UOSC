//! Kernel heap — a real `#[global_allocator]`, required because
//! `uosc_core`'s capability/memory/ipc/scheduler modules all use
//! `alloc::collections::BTreeMap`/`Vec` internally.
//!
//! **Two phases, because of a real circular dependency**: the real,
//! persistent `uosc_core::memory::PhysicalAllocator` (`paging::
//! store_globals`) allocates on the heap to build its own free-list
//! bookkeeping (`Vec`-backed) — so the heap has to exist *before* that
//! allocator does. Two real bugs surfaced getting this right:
//!
//! 1. First attempt called `PhysicalAllocator::new()` with no heap set up
//!    at all yet: a real `memory allocation of 32 bytes failed` panic.
//! 2. Second attempt used a small *static* bootstrap array and then tried
//!    to [`linked_list_allocator::Heap::extend`] into whatever virtual
//!    memory happened to sit right after it — which is wherever the
//!    linker placed that array within the kernel's own already-mapped
//!    image, not free virtual address space. Real result: `map_to` failed
//!    with a real `AccessViolation`, because that address was already
//!    spoken for by something the bootloader had mapped.
//!
//! The actual fix: phase 1 maps a handful of real pages at a **fixed,
//! deliberately unmapped** virtual address (`HEAP_BASE`), using a
//! [`crate::paging::BumpFrameAllocator`] that needs no heap at all (just a
//! counter — no `PhysicalAllocator`, no `Vec`). Phase 2, once the real
//! `PhysicalAllocator` exists, extends that same live heap with more real
//! pages placed exactly at its own `top()` — which is now guaranteed to
//! be free virtual space too, since `HEAP_BASE`'s region is ours alone.

use x86_64::VirtAddr;
use x86_64::structures::paging::{OffsetPageTable, PageTableFlags};
use linked_list_allocator::LockedHeap;

/// Chosen the same way the standard `blog_os`-lineage tutorials do: far
/// from both the kernel's own load address and the bootloader's dynamic
/// physical-memory mapping, so it's very unlikely to collide with either.
pub const HEAP_BASE: u64 = 0x_4444_4444_0000;

/// Phase 1: mapped via a heap-free [`crate::paging::BumpFrameAllocator`],
/// just large enough to get the real `PhysicalAllocator` running.
pub const BOOTSTRAP_HEAP_SIZE: u64 = 64 * 4096; // 256 KiB

/// Phase 2: mapped via the real `PhysicalAllocator`, once it exists.
pub const REAL_HEAP_SIZE: u64 = 256 * 4096; // 1 MiB

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// Phase 1: map `BOOTSTRAP_HEAP_SIZE` bytes at the fixed `HEAP_BASE`
/// using `frame_allocator` directly (no global `PhysicalAllocator`
/// involved — it doesn't exist yet), then point the global allocator at
/// that real, mapped range. Must run before anything, including
/// `paging::store_globals`, allocates on the heap.
pub fn init_bootstrap(mapper: &mut OffsetPageTable<'static>, frame_allocator: &mut crate::paging::BumpFrameAllocator) {
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
    let page_count = BOOTSTRAP_HEAP_SIZE / uosc_core::memory::PAGE_SIZE;
    for i in 0..page_count {
        let vaddr = VirtAddr::new(HEAP_BASE + i * uosc_core::memory::PAGE_SIZE);
        crate::paging::map_page_with(mapper, frame_allocator, vaddr, flags)
            .expect("failed to map a real physical page to bootstrap the kernel heap");
    }
    unsafe {
        ALLOCATOR.lock().init(HEAP_BASE as *mut u8, BOOTSTRAP_HEAP_SIZE as usize);
    }
}

/// Phase 2: extend the same live heap with `REAL_HEAP_SIZE` bytes of
/// real, individually mapped pages via the now-real `PhysicalAllocator`,
/// starting exactly at the heap's current top (still within the
/// `HEAP_BASE` region this module owns, so guaranteed free). Must run
/// after `paging::store_globals`.
pub fn extend_with_real_pages() {
    let extend_start = ALLOCATOR.lock().top() as u64;
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
    let page_count = REAL_HEAP_SIZE / uosc_core::memory::PAGE_SIZE;
    for i in 0..page_count {
        let vaddr = VirtAddr::new(extend_start + i * uosc_core::memory::PAGE_SIZE);
        crate::paging::with_mapper_and_phys(|mapper, phys| crate::paging::map_page(mapper, phys, vaddr, flags))
            .expect("paging::store_globals must run before allocator::extend_with_real_pages")
            .expect("failed to map a real physical page to extend the kernel heap");
    }
    unsafe {
        ALLOCATOR.lock().extend(REAL_HEAP_SIZE as usize);
    }
}
