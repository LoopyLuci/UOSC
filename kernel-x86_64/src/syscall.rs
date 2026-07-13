//! A real ring-3 → ring-0 privilege transition: actual user-mode (CPL3)
//! code, on real hardware, trapping into the kernel via `int 0x80` and
//! being observed by a real handler — not a simulation of a syscall, and
//! not CPL0 code merely pretending to be "userspace."
//!
//! **Scope, stated plainly**: this is a single, fixed, hand-assembled
//! ring-3 program (no ELF loader, no process abstraction, no return to
//! ring 3 after the syscall — see below), deliberately run *before*
//! `scheduler_bridge::init()` so it never overlaps with real task
//! switching. Combining a real privilege-level transition with real
//! preemptive switching (a timer tick landing mid-ring-3, deciding to
//! switch tasks, while running on the syscall entry's dedicated RSP0
//! stack) is real, additional complexity not attempted in this pass.
//!
//! ## The mechanism
//!
//! [`enter_ring3`] is a naked function that saves the current (kernel)
//! stack pointer and callee-saved registers — exactly like
//! [`crate::context::switch_to`]'s prologue — then builds a real `iretq`
//! frame (`SS`/`RSP`/`RFLAGS`/`CS`/`RIP`, all ring-3 selectors/values) and
//! executes `iretq` to actually drop the CPU to CPL3 for the first time.
//!
//! The ring-3 program itself is nine hand-assembled bytes
//! (`USER_PROGRAM`): `mov eax, 42; int 0x80; jmp $`. Hand-assembled,
//! rather than a compiled Rust function copied by address, so its exact
//! length is known with certainty when copying it into a freshly mapped
//! user-accessible page — no guessing where a compiled function's
//! boundary falls, and no risk of position-dependent relocations breaking
//! when the bytes move to a new address.
//!
//! `int 0x80`, with the IDT entry's DPL set to ring 3
//! (`interrupts.rs`), traps into [`syscall_entry_stub`] — also naked,
//! since reading the "syscall number" out of `rax` at the moment of the
//! trap needs the real register file, which the typed
//! `extern "x86-interrupt"` handler convention doesn't expose. Critically,
//! this stub **never `iretq`s back to ring 3** — since nothing here
//! implements process exit or multiple syscalls, there's nothing useful
//! for the ring-3 program to do after the one trap. Instead, exactly like
//! `scheduler_bridge.rs`'s `BOOT_PID` handback, it loads the kernel
//! stack pointer [`enter_ring3`] saved and `ret`s straight back into
//! `run_demo_syscall`'s caller — abandoning the ring-3 context (whose
//! `jmp $` after the trap is consequently never actually reached) rather
//! than resuming it.
//!
//! **What this deliberately does not claim**: no SMEP/SMAP (the kernel
//! can freely read/write/execute the "user" pages back), no NX
//! enforcement (the user pages are mapped writable *and* executable, not
//! least-privilege), no return path to ring 3, no second syscall, no
//! process/exit semantics — one hand-written program, one trap, one
//! observed result.

use core::arch::naked_asm;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::VirtAddr;
use x86_64::structures::paging::PageTableFlags;

use crate::serial_println;

pub const USER_CODE_ADDR: u64 = 0x_6666_6666_0000;
pub const USER_STACK_ADDR: u64 = 0x_7777_7777_0000;
const USER_STACK_SIZE: u64 = 4096;

/// `mov eax, 42` (B8 2A 00 00 00) ; `int 0x80` (CD 80) ; `jmp $` (EB FE).
/// The `jmp $` is a defensive landing spot only — see the module docs for
/// why the syscall handler never actually returns control here.
const USER_PROGRAM: [u8; 9] = [0xB8, 0x2A, 0x00, 0x00, 0x00, 0xCD, 0x80, 0xEB, 0xFE];

static mut KERNEL_RETURN_RSP: u64 = 0;
static LAST_SYSCALL: AtomicU64 = AtomicU64::new(0);

/// Drops the CPU to CPL3 for the first time, saving the current (kernel)
/// execution context first so [`syscall_entry_stub`] can later abandon
/// ring 3 and resume exactly here.
///
/// # Safety
/// `user_code`/`user_stack_top` must be real, present, user-accessible
/// mappings, and `user_cs`/`user_ss` must be real ring-3 GDT selectors.
#[unsafe(naked)]
unsafe extern "C" fn enter_ring3(user_code: u64, user_stack_top: u64, user_cs: u64, user_ss: u64) {
    naked_asm!(
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "lea rax, [rip + {kernel_rsp}]",
        "mov [rax], rsp",
        "push rcx",   // SS  (user_ss, 4th arg)
        "push rsi",   // RSP (user_stack_top, 2nd arg)
        "pushfq",
        "pop rax",
        "or rax, 0x200", // guarantee IF=1 in the ring-3 flags
        "push rax",   // RFLAGS
        "push rdx",   // CS  (user_cs, 3rd arg)
        "push rdi",   // RIP (user_code, 1st arg)
        "iretq",
        kernel_rsp = sym KERNEL_RETURN_RSP,
    )
}

/// The raw IDT handler for vector `0x80`, installed directly by address
/// (`interrupts.rs`, since reading `rax` needs the real register file, not
/// the typed `extern "x86-interrupt"` frame). Reads the trapped `rax`,
/// records it, then abandons ring 3 by resuming the kernel context
/// [`enter_ring3`] saved — see the module docs for why.
///
/// **A second real bug found here**: the IDT entry is (by
/// `set_handler_addr`'s own documented default) an *interrupt* gate,
/// which clears `IF` automatically on entry — normally undone by the
/// `iretq` a typed handler ends with, which restores the saved `RFLAGS`.
/// This stub deliberately never `iretq`s (see above), so without the
/// explicit `sti` below, `IF` stayed cleared *permanently* after the demo
/// — no crash, just every later timer tick silently not firing, and
/// `hlt()` (which only wakes on NMI when `IF=0`) hanging forever the
/// first time `main.rs` reached its `hlt()` loop. Real, reproduced,
/// fixed: interrupts must be explicitly re-enabled before resuming a
/// context that expects them on.
#[unsafe(naked)]
pub unsafe extern "C" fn syscall_entry_stub() {
    naked_asm!(
        "mov rdi, rax",
        "call {handler}",
        "lea rax, [rip + {kernel_rsp}]",
        "mov rsp, [rax]",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "sti",
        "ret",
        handler = sym record_syscall,
        kernel_rsp = sym KERNEL_RETURN_RSP,
    )
}

extern "C" fn record_syscall(number: u64) {
    LAST_SYSCALL.store(number, Ordering::SeqCst);
    serial_println!("[syscall] real int 0x80 trap from ring 3, rax={number}");
}

/// Maps a real user-accessible code page and stack page, copies the real
/// hand-assembled ring-3 program into the code page, and actually drops
/// to CPL3 to run it. Returns the value the real syscall handler observed
/// in `rax` at the trap, once the ring-3 context has been abandoned and
/// this kernel context resumed — `None` if the pages couldn't be mapped
/// (paging not yet initialized).
pub fn run_demo_syscall() -> Option<u64> {
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

    crate::paging::with_mapper_and_phys(|mapper, phys| {
        crate::paging::map_page(mapper, phys, VirtAddr::new(USER_CODE_ADDR), flags)
    })?
    .ok()?;
    crate::paging::with_mapper_and_phys(|mapper, phys| {
        crate::paging::map_page(mapper, phys, VirtAddr::new(USER_STACK_ADDR), flags)
    })?
    .ok()?;

    unsafe {
        core::ptr::copy_nonoverlapping(USER_PROGRAM.as_ptr(), USER_CODE_ADDR as *mut u8, USER_PROGRAM.len());
    }

    let selectors = crate::gdt::selectors();
    let user_cs = selectors.user_code_selector.0 as u64;
    let user_ss = selectors.user_data_selector.0 as u64;
    let user_stack_top = USER_STACK_ADDR + USER_STACK_SIZE;

    unsafe {
        enter_ring3(USER_CODE_ADDR, user_stack_top, user_cs, user_ss);
    }

    Some(LAST_SYSCALL.load(Ordering::SeqCst))
}
