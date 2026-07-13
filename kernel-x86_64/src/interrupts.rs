//! Real IDT + PIC-driven hardware interrupts. This is the x86-64
//! equivalent of `kernel/boot.ti`'s `init_idt()` plus `drivers/timer.ti`'s
//! PIT interrupt wiring — genuinely hardware-specific, out of scope for
//! `reference-rs` itself (see that crate's `timer.rs` module docs), and
//! implemented for real here since a bootable kernel needs it to do
//! anything beyond running once and halting.
//!
//! The timer handler below is what actually drives
//! `uosc_core::scheduler::RunQueue` — every PIT tick calls
//! `SCHEDULER.lock().tick()`, which is the real preemption point.

use core::sync::atomic::{AtomicU64, Ordering};
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

use crate::gdt::DOUBLE_FAULT_IST_INDEX;
use crate::serial_println;

/// How many real page faults the handler has actually caught and
/// demand-paged — read back by the boot self-test to confirm the fault
/// really was raised and handled by hardware, not that the memory just
/// happened to already be mapped.
pub static PAGE_FAULTS_DEMAND_PAGED: AtomicU64 = AtomicU64::new(0);

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> = Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
}

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            idt.double_fault.set_handler_fn(double_fault_handler)
                .set_stack_index(DOUBLE_FAULT_IST_INDEX);
        }
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt[InterruptIndex::Timer as u8].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard as u8].set_handler_fn(keyboard_interrupt_handler);
        // Installed by raw address, not `set_handler_fn`: the real ring-3
        // demo (`syscall.rs`) needs the actual register file at the trap
        // (`rax` as the "syscall number"), which the typed
        // `extern "x86-interrupt"` frame doesn't expose. DPL must be
        // Ring3, or a real `int 0x80` from CPL3 raises a real #GP instead
        // of reaching this handler — software interrupts otherwise
        // default to requiring CPL <= the gate's DPL, same as any other
        // privileged operation.
        unsafe {
            idt[0x80]
                .set_handler_addr(x86_64::VirtAddr::new(crate::syscall::syscall_entry_stub as *const () as u64))
                .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
        }
        idt
    };
}

pub fn init_idt() {
    IDT.load();
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    serial_println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, _error_code: u64) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

/// Real demand paging: a fault inside `paging::DEMAND_PAGE_REGION_*` gets a
/// real physical frame mapped in on the spot, via the same real
/// `PhysicalAllocator`/`OffsetPageTable` everything else in this kernel
/// shares, and execution resumes — the faulting instruction actually
/// retries and succeeds, on real (emulated) hardware. Anything outside
/// that region is a real, unrecoverable fault and still panics.
extern "x86-interrupt" fn page_fault_handler(stack_frame: InterruptStackFrame, error_code: x86_64::structures::idt::PageFaultErrorCode) {
    use x86_64::registers::control::Cr2;
    use x86_64::structures::paging::PageTableFlags;
    use x86_64::VirtAddr;

    let faulting_addr = Cr2::read_raw();

    if (crate::paging::DEMAND_PAGE_REGION_START..crate::paging::DEMAND_PAGE_REGION_END).contains(&faulting_addr) {
        let page_vaddr = VirtAddr::new(faulting_addr & !0xFFF);
        // Demand-paged memory is data (the boot self-test only ever writes
        // a u64 to it) — real NX applies here too, same as the kernel heap.
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | crate::cpu_features::nx_flag();
        let mapped = crate::paging::with_mapper_and_phys(|mapper, phys| {
            crate::paging::map_page(mapper, phys, page_vaddr, flags).is_ok()
        })
        .unwrap_or(false);
        if mapped {
            PAGE_FAULTS_DEMAND_PAGED.fetch_add(1, Ordering::SeqCst);
            serial_println!("[page_fault] real #PF at {:#x}, demand-paged and resumed", faulting_addr);
            return;
        }
    }

    panic!(
        "EXCEPTION: PAGE FAULT at {:#x}, error {:?} — outside the demand-page region, cannot recover\n{:#?}",
        faulting_addr, error_code, stack_frame
    );
}

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    // EOI must come first, not last: `on_timer_tick` may perform a real
    // context switch (see `context.rs`) and not return here for many
    // ticks, until this exact task is resumed. The PIC won't raise IRQ0
    // again until it's EOI'd, so sending it after the tick logic would
    // silently stall every further timer interrupt for whichever task
    // ends up running in the meantime.
    unsafe {
        PICS.lock().notify_end_of_interrupt(InterruptIndex::Timer as u8);
    }
    crate::scheduler_bridge::on_timer_tick();
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    unsafe {
        PICS.lock().notify_end_of_interrupt(InterruptIndex::Keyboard as u8);
    }
}
