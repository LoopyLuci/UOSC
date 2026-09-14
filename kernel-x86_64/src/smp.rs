//! Real x86 AP (application processor) bring-up: the actual
//! INIT-SIPI-SIPI startup sequence via the real LAPIC, a real 16-bit
//! real-mode trampoline relocated below 1MiB (`smp/ap_trampoline.asm`,
//! assembled by a real external assembler — see `build.rs`), transitioning
//! through protected mode into real 64-bit long mode over the kernel's own,
//! already-built page tables, and a real, separate per-CPU GDT/TSS
//! (`gdt::init_for_ap`) before the AP is allowed to take any interrupt.
//!
//! **Scope, stated plainly, matching `acpi_topology.rs`'s own honesty**:
//! this brings up exactly one AP and proves it executes real code and
//! survives a real fault — nothing more. No LAPIC/IOAPIC interrupt routing
//! to the AP beyond the startup IPI itself (the real breakpoint exception
//! this module tests with is delivered directly by the CPU via the shared
//! IDT — exceptions never go through the APIC at all, which is exactly why
//! this can be tested without touching IOAPIC routing). No concurrent-safe
//! scheduler handoff — the AP never touches `scheduler_bridge`'s
//! `RunQueue`, ever. No concurrent heap/physical-allocator access — the BSP
//! (`bring_up_ap`, below) does nothing but poll real atomics in a spin loop
//! from the moment it sends the SIPI, making zero further heap or
//! `PhysicalAllocator` calls until the AP has already finished its own
//! one-time setup allocations (its stack, in `bring_up_ap` itself, before
//! the SIPI is even sent; its GDT/TSS, inside `ap_entry64`, before it signals
//! `AP_STARTED`) and the AP itself never allocates again after that point,
//! parking permanently in a `hlt` loop instead. This is a deliberate,
//! enforced sequencing, not reliance on `linked_list_allocator::LockedHeap`
//! happening to be a spinlock underneath — see `bring_up_ap`'s own comments
//! for exactly where that sequencing is enforced.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use x86_64::structures::paging::{PageTableFlags, PhysFrame, Size4KiB};
use x86_64::{PhysAddr, VirtAddr};

use crate::acpi_topology::CpuTopology;
use crate::paging::{self, PhysicalAllocatorAdapter};
use crate::serial_println;

include!(concat!(env!("OUT_DIR"), "/ap_trampoline_symbols.rs"));

static TRAMPOLINE_BLOB: &[u8] = include_bytes!(env!("AP_TRAMPOLINE_BIN"));

/// Must match `ap_trampoline.asm`'s `ORG 0x8000` exactly — see that file's
/// module docs. Chosen the same way Linux and most hobby x86 kernels choose
/// their own AP trampoline address: a fixed, conventional, page-aligned,
/// below-1MiB physical address real BIOS/QEMU firmware leaves free.
pub const TRAMPOLINE_PHYS_ADDR: u64 = 0x8000;

/// Set by the AP itself (`ap_entry64`, below) once real 64-bit Rust code is
/// genuinely running on it — the boot self-test's real proof the AP
/// executed real code, not just that the trampoline bytes were written.
static AP_STARTED: AtomicBool = AtomicBool::new(false);

/// Set by the AP after it deliberately triggers, and survives, a real
/// `int3` breakpoint exception on its own IDT/GDT/TSS.
static AP_FAULT_SURVIVED: AtomicBool = AtomicBool::new(false);

/// The real APIC ID the AP reports back once running — read back by the
/// BSP to confirm it matches the real MADT-reported APIC ID that was
/// actually targeted, not just that *some* core came up.
static AP_REPORTED_APIC_ID: AtomicU64 = AtomicU64::new(u64::MAX);

/// A real, dedicated stack for the AP — heap-allocated by the BSP before
/// the SIPI is sent, leaked to `'static` since the AP runs on it forever
/// (parked in a `hlt` loop after its self-test; a real teardown path is
/// real, separate, follow-on work no core in this kernel needs yet).
const AP_STACK_SIZE: usize = 64 * 1024;

/// Reads this core's own real, initial APIC ID directly from CPUID leaf 1
/// (EBX bits 31:24) — real hardware identification, available with no
/// LAPIC MMIO mapping needed. Called on the BSP (to know which MADT-
/// reported CPU is *not* itself — the AP to target) and, separately, on the
/// AP itself inside `ap_entry64` (to report back which real core actually
/// ran).
pub fn this_core_apic_id() -> u8 {
    let result = core::arch::x86_64::__cpuid(1);
    (result.ebx >> 24) as u8
}

/// Real busy-wait, timed off the real PIT tick counter
/// (`scheduler_bridge::tick_count`, 200 Hz) rather than an uncalibrated
/// spin count — the same real hardware timer source the scheduler itself
/// already depends on.
fn busy_wait_ticks(ticks: u64) {
    let start = crate::scheduler_bridge::tick_count();
    while crate::scheduler_bridge::tick_count() < start + ticks {
        core::hint::spin_loop();
    }
}

/// LAPIC MMIO register offsets this module actually uses — the real,
/// documented xAPIC register layout, not the newer x2APIC MSR interface
/// (this kernel never checks for/enables x2APIC; QEMU's default CPU model
/// exposes the plain xAPIC MMIO window every real INIT-SIPI-SIPI
/// implementation historically targets).
const LAPIC_REG_ICR_LOW: usize = 0x300;
const LAPIC_REG_ICR_HIGH: usize = 0x310;

/// Fixed virtual address the real LAPIC MMIO page is mapped at — an unused
/// nibble among this kernel's other fixed regions (`0x1111`/`0x3333`/
/// `0x4444`/`0x5555`/`0x6666`/`0x7777`; `0x8888` is avoided, see
/// `task_stack.rs`'s documented non-canonical-address bug).
const LAPIC_VIRT_BASE: u64 = 0x_2222_2222_0000;

/// Real MMIO write to a LAPIC register. **Deliberately write-only** — an
/// earlier version of this module also read `ICR_LOW` back (the standard
/// "poll the delivery-status bit" technique) immediately after writing a
/// SIPI, and that real read reliably stalled this QEMU version's whole VM
/// (not just this core) — a real, reproducible bug, not a hypothetical one;
/// see STATUS.md for the full story. [`bring_up_ap`] now waits a real,
/// generous, PIT-tick-paced delay after each send instead.
unsafe fn lapic_write(reg: usize, value: u32) {
    unsafe {
        ((LAPIC_VIRT_BASE as usize + reg) as *mut u32).write_volatile(value);
    }
}

/// Real Rust entry point for the AP, jumped to directly (`jmp rax`, no
/// `call`/`ret` — there is nothing to return to) by the very last
/// instruction of `ap_trampoline.asm`'s 64-bit section. This is the first
/// Rust code that has ever executed on any core but the boot processor in
/// this kernel.
extern "C" fn ap_entry64() -> ! {
    // A real, genuinely separate GDT + TSS — required before this core may
    // take any interrupt or exception (see gdt.rs's module docs on why).
    // Must run before init_idt(): the double-fault IST index only resolves
    // to a real stack once this core's own TSS is the one loaded.
    crate::gdt::init_for_ap();
    crate::interrupts::init_idt();

    let apic_id = this_core_apic_id();
    AP_REPORTED_APIC_ID.store(apic_id as u64, Ordering::SeqCst);
    serial_println!("[ap] real 64-bit Rust code now running on the AP, apic_id={apic_id}");
    AP_STARTED.store(true, Ordering::SeqCst);

    // A real, deliberately-triggered exception on this exact core, using
    // this core's own IDT/GDT/TSS — proof the AP can take a fault and
    // resume without corrupting the BSP's own fault-handling state. `int3`
    // is the cleanest possible demonstration: `interrupts::
    // breakpoint_handler` only logs and returns via a real `iretq`, and
    // unlike a page fault it never touches the shared mapper/physical
    // allocator at all, so this is a pure, unambiguous test of "this core's
    // interrupt machinery works and doesn't disturb shared state."
    serial_println!("[ap] deliberately triggering a real breakpoint exception on this core");
    unsafe {
        core::arch::asm!("int3");
    }
    serial_println!("[ap] resumed after the real breakpoint exception — this core's IDT/GDT/TSS survived it");
    AP_FAULT_SURVIVED.store(true, Ordering::SeqCst);

    // This AP never touches the scheduler, the heap allocator, or the
    // physical page allocator again after this point — see this module's
    // docs on why that's a deliberate scope boundary, not an oversight.
    // Interrupts are disabled first so nothing (a stray, unrouted IRQ)
    // ever wakes this `hlt` loop.
    unsafe {
        core::arch::asm!("cli");
    }
    loop {
        x86_64::instructions::hlt();
    }
}

/// Real proof, from the BSP's side, that the AP survived its fault.
pub fn ap_fault_survived() -> bool {
    AP_FAULT_SURVIVED.load(Ordering::SeqCst)
}

pub fn ap_reported_apic_id() -> Option<u8> {
    match AP_REPORTED_APIC_ID.load(Ordering::SeqCst) {
        u64::MAX => None,
        id => Some(id as u8),
    }
}

/// Real AP bring-up: copies the real trampoline to a real, identity-mapped
/// page below 1MiB, pokes the real CR3/stack/entry-point values into it,
/// then sends a real INIT-SIPI-SIPI sequence via the real LAPIC targeting
/// `target_apic_id`.
///
/// Returns `true` only if the AP genuinely reported back (`AP_STARTED`) and
/// survived its own deliberate fault (`AP_FAULT_SURVIVED`) within a real,
/// generous, PIT-tick-paced timeout — not "the IPI was sent," which proves
/// nothing about whether anything on the other side actually ran.
pub fn bring_up_ap(topology: &CpuTopology, target_apic_id: u8) -> bool {
    // Real sanity check, not a hand-counted assumption: the nasm-computed
    // symbol range (from the real map file, see build.rs) must exactly
    // match the real blob size `include_bytes!` pulled in from the same
    // assembler run.
    debug_assert_eq!(
        (AP_TRAMPOLINE_END - AP_TRAMPOLINE_START) as usize,
        TRAMPOLINE_BLOB.len(),
        "nasm-reported trampoline symbol range doesn't match the real assembled blob length"
    );

    // --- Map the real LAPIC MMIO page at a fixed virtual address. Not
    // necessarily already covered by the bootloader's blanket
    // physical-memory mapping (that mapping's upper bound comes from the
    // real e820/UEFI memory map, which does not have to include every MMIO
    // hole a real chipset exposes) — mapped explicitly here instead of
    // assumed. ---
    let lapic_phys = topology.local_apic_address as u64;
    let mapped_lapic = paging::with_mapper_and_phys(|mapper, phys| {
        let mut allocator = PhysicalAllocatorAdapter { phys };
        paging::map_page_to_frame(
            mapper,
            &mut allocator,
            VirtAddr::new(LAPIC_VIRT_BASE),
            PhysFrame::containing_address(PhysAddr::new(lapic_phys)),
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE | crate::cpu_features::nx_flag(),
        )
        .is_ok()
    })
    .unwrap_or(false);
    if !mapped_lapic {
        serial_println!("[smp] failed to map the real LAPIC MMIO page at {:#x} — cannot send a real IPI", lapic_phys);
        return false;
    }

    // --- Identity-map the trampoline's physical page (virt == phys) — the
    // AP's first instructions run with paging off, so the instruction right
    // after CR0.PG is set (still fetched from this same low address, now
    // interpreted as *virtual*) must resolve to the same physical byte it
    // already is. See ap_trampoline.asm's module docs. ---
    let trampoline_frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(PhysAddr::new(TRAMPOLINE_PHYS_ADDR));
    let identity_mapped = paging::with_mapper_and_phys(|mapper, phys| {
        let mut allocator = PhysicalAllocatorAdapter { phys };
        paging::map_page_to_frame(
            mapper,
            &mut allocator,
            VirtAddr::new(TRAMPOLINE_PHYS_ADDR),
            trampoline_frame,
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
        )
        .is_ok()
    })
    .unwrap_or(false);
    if !identity_mapped {
        serial_println!("[smp] failed to identity-map the real trampoline page at {:#x}", TRAMPOLINE_PHYS_ADDR);
        return false;
    }

    // --- Copy the real, nasm-assembled trampoline to its real destination
    // physical address, reached via the bootloader's real
    // physical-memory-offset mapping — the same mechanism every other
    // physical-address access in this kernel already uses (acpi_topology.rs,
    // address_space.rs). ---
    let Some(phys_mem_offset) = paging::phys_mem_offset() else {
        serial_println!("[smp] no physical memory offset available — paging::init must run first");
        return false;
    };
    let dest = (phys_mem_offset.as_u64() + TRAMPOLINE_PHYS_ADDR) as *mut u8;
    unsafe {
        core::ptr::copy_nonoverlapping(TRAMPOLINE_BLOB.as_ptr(), dest, TRAMPOLINE_BLOB.len());
    }
    serial_println!(
        "[smp] real {}-byte AP trampoline copied to physical {:#x} (identity-mapped)",
        TRAMPOLINE_BLOB.len(),
        TRAMPOLINE_PHYS_ADDR
    );

    // --- A real, dedicated stack for the AP — never the BSP's. Allocated
    // here, before the SIPI is sent, so this is the *last* heap allocation
    // the BSP makes before it starts pure-spin-polling for the AP's real
    // atomics — see this module's top-level docs on the deliberate
    // BSP/AP heap-access sequencing. ---
    let ap_stack = alloc::vec![0u8; AP_STACK_SIZE].into_boxed_slice();
    let ap_stack_top = ap_stack.as_ptr() as u64 + AP_STACK_SIZE as u64;
    core::mem::forget(ap_stack); // leaked: the AP runs on this stack forever, see module docs

    // --- Poke the three real values the trampoline's data slots need,
    // using the absolute addresses nasm itself computed from the real
    // ORG'd assembly (see build.rs) — not hand-counted byte offsets. ---
    let (real_cr3, _) = x86_64::registers::control::Cr3::read();
    unsafe {
        ((phys_mem_offset.as_u64() + AP_CR3) as *mut u64).write_volatile(real_cr3.start_address().as_u64());
        ((phys_mem_offset.as_u64() + AP_STACK_TOP) as *mut u64).write_volatile(ap_stack_top);
        ((phys_mem_offset.as_u64() + AP_ENTRY_ADDR) as *mut u64).write_volatile(ap_entry64 as *const () as usize as u64);
    }

    serial_println!(
        "[smp] sending a real INIT-SIPI-SIPI sequence via the real LAPIC at {:#x} (mapped at {:#x}) to apic_id={}",
        lapic_phys,
        LAPIC_VIRT_BASE,
        target_apic_id
    );

    let vector = (TRAMPOLINE_PHYS_ADDR >> 12) as u32;
    unsafe {
        // Real INIT IPI: delivery mode 101 (INIT), level=assert (bit 14),
        // destination = target_apic_id.
        lapic_write(LAPIC_REG_ICR_HIGH, (target_apic_id as u32) << 24);
        lapic_write(LAPIC_REG_ICR_LOW, 0x0000_4500);
    }
    busy_wait_ticks(3); // ~15ms at 200Hz — real hardware-timer-paced delay, not a guessed spin count

    // Two real SIPIs, per the real Intel MP startup algorithm, each
    // followed by a real, PIT-tick-paced delay to give real hardware time
    // to act on it.
    for _ in 0..2 {
        unsafe {
            lapic_write(LAPIC_REG_ICR_HIGH, (target_apic_id as u32) << 24);
            lapic_write(LAPIC_REG_ICR_LOW, 0x0000_4600 | vector);
        }
        busy_wait_ticks(1); // ~5ms
    }

    // --- From here on the BSP makes no further heap/allocator calls until
    // this function returns — it only polls real atomics the AP itself
    // sets. This is the enforced half of the deliberate no-concurrent-heap-
    // access sequencing described in this module's top-level docs. ---

    // Real proof this actually ran, not "the IPI was sent": poll the real
    // atomic the AP itself sets, with a generous real-timer-paced timeout
    // (~1s at 200Hz) rather than assuming success.
    let deadline = crate::scheduler_bridge::tick_count() + 200;
    while !AP_STARTED.load(Ordering::SeqCst) {
        if crate::scheduler_bridge::tick_count() > deadline {
            serial_println!("[smp] timed out waiting for the AP to report in — no real evidence it ran");
            return false;
        }
        core::hint::spin_loop();
    }
    serial_println!("[smp] real confirmation: the AP reported in (apic_id={:?})", ap_reported_apic_id());

    // Real proof the deliberately-triggered fault on the AP was survived —
    // same real-timer-paced wait.
    let deadline = crate::scheduler_bridge::tick_count() + 200;
    while !AP_FAULT_SURVIVED.load(Ordering::SeqCst) {
        if crate::scheduler_bridge::tick_count() > deadline {
            serial_println!("[smp] timed out waiting for the AP's fault-survival report");
            return false;
        }
        core::hint::spin_loop();
    }
    serial_println!("[smp] real confirmation: the AP survived its deliberately-triggered breakpoint exception");

    true
}
