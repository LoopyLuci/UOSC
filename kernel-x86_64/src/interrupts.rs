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

use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

use crate::gdt::DOUBLE_FAULT_IST_INDEX;
use crate::serial_println;

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

extern "x86-interrupt" fn page_fault_handler(stack_frame: InterruptStackFrame, error_code: x86_64::structures::idt::PageFaultErrorCode) {
    use x86_64::registers::control::Cr2;
    serial_println!(
        "EXCEPTION: PAGE FAULT at {:?}, error {:?}\n{:#?}",
        Cr2::read(),
        error_code,
        stack_frame
    );
}

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    crate::scheduler_bridge::on_timer_tick();
    unsafe {
        PICS.lock().notify_end_of_interrupt(InterruptIndex::Timer as u8);
    }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    unsafe {
        PICS.lock().notify_end_of_interrupt(InterruptIndex::Keyboard as u8);
    }
}
