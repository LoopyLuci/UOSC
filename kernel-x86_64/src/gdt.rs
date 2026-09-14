//! Real GDT + a dedicated double-fault stack (IST), using the `x86_64`
//! crate's safe wrappers over the real descriptor-table hardware
//! interface. This is the x86-64 equivalent of `kernel/boot.ti`'s
//! `init_gdt()` — a genuinely hardware-specific step this crate's
//! `no_std`, `#![cfg_attr(not(test), no_std)]`-only `reference-rs` library
//! deliberately never attempted, and which real x86-64 boot requires.

use alloc::boxed::Box;
use lazy_static::lazy_static;
use x86_64::VirtAddr;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable};
use x86_64::structures::tss::TaskStateSegment;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            const STACK_SIZE: usize = 4096 * 5;
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
            #[allow(static_mut_refs)]
            let stack_start = VirtAddr::from_ptr(&raw const STACK);
            stack_start + STACK_SIZE as u64
        };
        // RSP0: the kernel stack the CPU loads automatically on any trap
        // that raises the privilege level to ring 0 with no IST override —
        // exactly what happens the moment `syscall.rs`'s ring-3 demo
        // executes `int 0x80`. Without this set, that trap would run on
        // whatever garbage (zeroed) stack pointer the TSS starts with.
        tss.privilege_stack_table[0] = {
            const STACK_SIZE: usize = 4096 * 5;
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
            #[allow(static_mut_refs)]
            let stack_start = VirtAddr::from_ptr(&raw const STACK);
            stack_start + STACK_SIZE as u64
        };
        tss
    };
}

pub struct Selectors {
    code_selector: x86_64::structures::gdt::SegmentSelector,
    tss_selector: x86_64::structures::gdt::SegmentSelector,
    pub user_code_selector: x86_64::structures::gdt::SegmentSelector,
    pub user_data_selector: x86_64::structures::gdt::SegmentSelector,
}

lazy_static! {
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.append(Descriptor::kernel_code_segment());
        let tss_selector = gdt.append(Descriptor::tss_segment(&TSS));
        // Real ring-3 descriptors, used by `syscall.rs` to actually drop
        // to CPL3 — `GlobalDescriptorTable::append` bakes each
        // descriptor's own DPL into the returned selector's RPL bits
        // automatically, so these are already ring-3-ready selectors, no
        // manual `| 3` needed.
        let user_data_selector = gdt.append(Descriptor::user_data_segment());
        let user_code_selector = gdt.append(Descriptor::user_code_segment());
        (gdt, Selectors { code_selector, tss_selector, user_code_selector, user_data_selector })
    };
}

pub fn selectors() -> &'static Selectors {
    &GDT.1
}

pub fn init() {
    use x86_64::instructions::segmentation::{CS, DS, ES, FS, GS, SS, Segment};
    use x86_64::instructions::tables::load_tss;
    use x86_64::structures::gdt::SegmentSelector;
    use x86_64::PrivilegeLevel;

    GDT.0.load();
    unsafe {
        CS::set_reg(GDT.1.code_selector);
        load_tss(GDT.1.tss_selector);
        // The bootloader's own GDT is gone the moment `GDT.0.load()` runs
        // above — but DS/ES/FS/GS/SS still hold *selectors* (indices) into
        // that now-replaced table. In 64-bit mode a null data-segment
        // selector is valid and exactly what every other minimal-kernel
        // reference does here; without this, those registers keep
        // pointing at descriptor-table slots that no longer exist, which
        // doesn't fault immediately but does fault the next time hardware
        // implicitly consults one of them — which is exactly what an
        // interrupt entry/IRETQ does.
        let null_selector = SegmentSelector::new(0, PrivilegeLevel::Ring0);
        DS::set_reg(null_selector);
        ES::set_reg(null_selector);
        FS::set_reg(null_selector);
        GS::set_reg(null_selector);
        SS::set_reg(null_selector);
    }
}

/// A real, genuinely separate GDT + TSS for the AP — required, not
/// optional, before the AP is allowed to take any interrupt or exception.
/// If the AP ran with the BSP's shared [`TSS`] (in particular its
/// [`DOUBLE_FAULT_IST_INDEX`] stack pointer), a fault taken on the AP would
/// push its exception frame onto the *same* IST stack the BSP's own
/// double-fault handler uses — a real race/corruption hazard, not a
/// hypothetical one, the moment both cores ever fault around the same time.
///
/// This kernel only ever brings up one AP in this milestone (see
/// `smp.rs`), so one heap-allocated, `'static`-leaked instance — built
/// fresh each time this is called, on whichever core calls it — is the
/// honest, minimal version of "per-CPU": a real second, independent
/// instance, not an array pre-sized for a core count this kernel has no
/// present use for. A real multi-core system would index an array of these
/// by APIC ID instead; that's real, separate follow-on work.
///
/// **Must run on the AP itself** — `CS::set_reg`/`load_tss` write core-local
/// state (there is no shared "current GDT/TSS" — each logical CPU has its
/// own GDTR/TR registers) — and must run before the AP's IDT is loaded via
/// [`crate::interrupts::init_idt`], since the double-fault IDT entry's IST
/// index only resolves to a real stack once *this* core's TSS is the one
/// loaded.
pub fn init_for_ap() {
    use x86_64::instructions::segmentation::{CS, DS, ES, FS, GS, SS, Segment};
    use x86_64::instructions::tables::load_tss;
    use x86_64::structures::gdt::SegmentSelector;
    use x86_64::PrivilegeLevel;

    const STACK_SIZE: usize = 4096 * 5;

    let mut tss = TaskStateSegment::new();
    let double_fault_stack = Box::leak(alloc::vec![0u8; STACK_SIZE].into_boxed_slice());
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
        VirtAddr::from_ptr(double_fault_stack.as_ptr()) + STACK_SIZE as u64;
    // RSP0, same role as the BSP's: the stack the CPU loads automatically
    // on any ring0-raising trap with no IST override.
    let priv_stack = Box::leak(alloc::vec![0u8; STACK_SIZE].into_boxed_slice());
    tss.privilege_stack_table[0] = VirtAddr::from_ptr(priv_stack.as_ptr()) + STACK_SIZE as u64;
    let tss: &'static TaskStateSegment = Box::leak(Box::new(tss));

    let mut gdt = GlobalDescriptorTable::new();
    let code_selector = gdt.append(Descriptor::kernel_code_segment());
    let tss_selector = gdt.append(Descriptor::tss_segment(tss));
    let gdt: &'static GlobalDescriptorTable = Box::leak(Box::new(gdt));

    gdt.load();
    unsafe {
        CS::set_reg(code_selector);
        load_tss(tss_selector);
        let null_selector = SegmentSelector::new(0, PrivilegeLevel::Ring0);
        DS::set_reg(null_selector);
        ES::set_reg(null_selector);
        FS::set_reg(null_selector);
        GS::set_reg(null_selector);
        SS::set_reg(null_selector);
    }
}
