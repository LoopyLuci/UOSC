//! Kernel heap — a real `#[global_allocator]`, required because
//! `uosc_core`'s capability/memory/ipc/scheduler modules all use
//! `alloc::collections::BTreeMap`/`Vec` internally.
//!
//! **Scope note**: the heap arena is a static array in the kernel binary's
//! own BSS, not a dynamically-mapped virtual memory region — this binary
//! doesn't set up hardware page tables at all (see `main.rs`'s top-level
//! docs), so there's no virtual memory system yet to carve a heap out of.
//! A real kernel heap backed by pages obtained through
//! `uosc_core::memory::PhysicalAllocator` and mapped via a real page table
//! is real, separate follow-on work.

use linked_list_allocator::LockedHeap;

const HEAP_SIZE: usize = 1024 * 1024; // 1 MiB

#[repr(align(16))]
struct HeapArena([u8; HEAP_SIZE]);

static mut HEAP_ARENA: HeapArena = HeapArena([0; HEAP_SIZE]);

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init() {
    unsafe {
        #[allow(static_mut_refs)]
        let start = HEAP_ARENA.0.as_mut_ptr();
        ALLOCATOR.lock().init(start, HEAP_SIZE);
    }
}
