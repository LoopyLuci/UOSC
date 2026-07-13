//! Real GDT + a dedicated double-fault stack (IST), using the `x86_64`
//! crate's safe wrappers over the real descriptor-table hardware
//! interface. This is the x86-64 equivalent of `kernel/boot.ti`'s
//! `init_gdt()` — a genuinely hardware-specific step this crate's
//! `no_std`, `#![cfg_attr(not(test), no_std)]`-only `reference-rs` library
//! deliberately never attempted, and which real x86-64 boot requires.

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
        tss
    };
}

struct Selectors {
    code_selector: x86_64::structures::gdt::SegmentSelector,
    tss_selector: x86_64::structures::gdt::SegmentSelector,
}

lazy_static! {
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.append(Descriptor::kernel_code_segment());
        let tss_selector = gdt.append(Descriptor::tss_segment(&TSS));
        (gdt, Selectors { code_selector, tss_selector })
    };
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
