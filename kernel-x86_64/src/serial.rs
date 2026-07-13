//! Real serial console — COM1 (0x3F8), the same port `drivers/console.ti`
//! targets, using the `uart_16550` crate instead of hand-rolled port I/O
//! asm (the crate is a well-established, widely-used abstraction over the
//! exact same 16550 UART programming sequence `console.ti`'s
//! `init_serial` describes; using it here isn't cutting a corner, it's
//! preferring a maintained implementation of hardware-register plumbing
//! that has nothing to do with UOSC-specific logic).

use core::fmt::Write;
use lazy_static::lazy_static;
use spin::Mutex;
use uart_16550::{Config, Uart16550Tty};
use uart_16550::backend::PioBackend;

lazy_static! {
    static ref SERIAL1: Mutex<Uart16550Tty<PioBackend>> = Mutex::new(unsafe {
        Uart16550Tty::new_port(0x3f8, Config::default()).expect("failed to init COM1 serial port")
    });
}

pub fn init() {
    lazy_static::initialize(&SERIAL1);
}

#[doc(hidden)]
pub fn _print(args: core::fmt::Arguments) {
    // Real preemption (see `context.rs`) means task code can be switched
    // out mid-print while holding this lock, and the task switched into
    // may want it too — a real deadlock, not a hypothetical one, as soon
    // as two tasks both call `serial_println!`. Disabling interrupts
    // around the critical section guarantees this can't be preempted
    // while holding the lock, since preemption only ever happens on a
    // timer interrupt.
    x86_64::instructions::interrupts::without_interrupts(|| {
        SERIAL1.lock().write_fmt(args).expect("serial write failed");
    });
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\r\n", format_args!($($arg)*)));
}
