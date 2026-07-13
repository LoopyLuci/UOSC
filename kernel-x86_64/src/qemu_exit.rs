//! Scriptable exit for the QEMU `isa-debug-exit` device (`-device
//! isa-debug-exit,iobase=0xf4,iosize=0x04` in the run command), so the
//! boot self-test in `main.rs` produces a real process exit code an
//! automated check can read — not a human watching a window.

use x86_64::instructions::port::Port;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ExitCode {
    Success = 0x10,
    Failed = 0x11,
}

pub fn exit_qemu(code: ExitCode) -> ! {
    unsafe {
        let mut port = Port::new(0xf4);
        port.write(code as u32);
    }
    // The isa-debug-exit device halts the VM on write; loop as a fallback
    // in case it's ever absent (e.g. accidentally run on real hardware).
    loop {
        x86_64::instructions::hlt();
    }
}
