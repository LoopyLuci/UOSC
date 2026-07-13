//! A real ring-3 → ring-0 privilege transition: actual user-mode (CPL3)
//! code, on real hardware, trapping into the kernel via `int 0x80` and
//! being observed by a real handler — not a simulation of a syscall, and
//! not CPL0 code merely pretending to be "userspace." This pass extends
//! the original one-shot demo to a real round trip: the ring-3 program
//! makes three real syscalls, is really resumed in ring 3 after each one,
//! and only abandons ring 3 on a fourth, distinct "exit" trap.
//!
//! **Scope, stated plainly**: this is a single, fixed, hand-assembled
//! ring-3 program (no ELF loader, no process abstraction, no general
//! syscall ABI), deliberately run *before* `scheduler_bridge::init()` so
//! it never overlaps with real task switching. Combining a real
//! privilege-level transition with real preemptive switching (a timer
//! tick landing mid-ring-3, deciding to switch tasks, while running on
//! the syscall entry's dedicated RSP0 stack) is real, additional
//! complexity not attempted in this pass. Also still not attempted: more
//! than one ring-3 program ever existing at once.
//!
//! **NX/SMEP/SMAP** (`cpu_features.rs`) now apply for real: the user code
//! page stays executable (real ring-3 code actually runs from it every
//! boot) but the user stack page carries real NX, and the one kernel write
//! into the user code page (copying `USER_PROGRAM` in, below) is wrapped
//! in real `stac`/`clac` so it survives real `CR4.SMAP` being set. SMEP
//! itself (forbidding the *kernel* from executing user-accessible pages)
//! is enabled and was empirically fault-tested manually (see
//! `cpu_features.rs`'s module docs and `STATUS.md`), but nothing in this
//! codebase — including this module — ever attempts it as part of normal
//! operation.
//!
//! ## The mechanism
//!
//! [`enter_ring3`] is a naked function that saves the current (kernel)
//! stack pointer and callee-saved registers — exactly like
//! [`crate::context::switch_to`]'s prologue — then builds a real `iretq`
//! frame (`SS`/`RSP`/`RFLAGS`/`CS`/`RIP`, all ring-3 selectors/values) and
//! executes `iretq` to actually drop the CPU to CPL3 for the first time.
//!
//! The ring-3 program itself is hand-assembled bytes (`USER_PROGRAM`), not
//! a compiled Rust function copied by address, so its exact length is
//! known with certainty when copying it into a freshly mapped
//! user-accessible page — no guessing where a compiled function's
//! boundary falls, and no risk of position-dependent relocations breaking
//! when the bytes move to a new address. It makes four real traps in
//! sequence: `mov eax,10; int 0x80`, `mov eax,20; int 0x80`,
//! `mov eax,30; int 0x80`, then `mov eax,999; int 0x80` (999 =
//! [`EXIT_SYSCALL`]), followed by a defensive `jmp $` that's only ever
//! reached if the exit handling below has a bug.
//!
//! `int 0x80`, with the IDT entry's DPL set to ring 3
//! (`interrupts.rs`), traps into [`syscall_entry_stub`] — also naked,
//! since reading the "syscall number" out of `rax` at the moment of the
//! trap needs the real register file, which the typed
//! `extern "x86-interrupt"` handler convention doesn't expose.
//!
//! **Two different real returns, not one**: for the three ordinary
//! syscalls, the stub does the standard, textbook thing — record the
//! value, then `iretq` using the *same* hardware-pushed interrupt frame
//! the trap already left on the stack, resuming ring 3 at the very next
//! instruction after `int 0x80`. Nothing needs to be reconstructed;
//! that's the whole point of `iretq`. Only the fourth, `EXIT_SYSCALL`
//! trap takes the other path — exactly like `scheduler_bridge.rs`'s
//! `BOOT_PID` handback, it loads the kernel stack pointer [`enter_ring3`]
//! saved and `ret`s straight back into `run_demo_syscall`'s caller,
//! abandoning ring 3 for good (the trailing `jmp $` is consequently never
//! actually reached).

use core::arch::naked_asm;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use x86_64::VirtAddr;
use x86_64::structures::paging::PageTableFlags;

use crate::serial_println;

pub const USER_CODE_ADDR: u64 = 0x_6666_6666_0000;
pub const USER_STACK_ADDR: u64 = 0x_7777_7777_0000;
const USER_STACK_SIZE: u64 = 4096;

/// The one syscall number that means "abandon ring 3 for good," distinct
/// from any of the three ordinary demo values (10/20/30) below.
const EXIT_SYSCALL: u64 = 999;

/// Four `mov eax, imm32` (B8 + 4-byte LE immediate) / `int 0x80` (CD 80)
/// pairs — three ordinary syscalls (10, 20, 30), then the exit syscall
/// (999 = 0x000003E7) — followed by a defensive `jmp $` (EB FE), never
/// actually reached if exit handling works.
const USER_PROGRAM: [u8; 30] = [
    0xB8, 0x0A, 0x00, 0x00, 0x00, 0xCD, 0x80, // mov eax, 10  ; int 0x80
    0xB8, 0x14, 0x00, 0x00, 0x00, 0xCD, 0x80, // mov eax, 20  ; int 0x80
    0xB8, 0x1E, 0x00, 0x00, 0x00, 0xCD, 0x80, // mov eax, 30  ; int 0x80
    0xB8, 0xE7, 0x03, 0x00, 0x00, 0xCD, 0x80, // mov eax, 999 ; int 0x80
    0xEB, 0xFE, // jmp $  (defensive; never reached)
];

static mut KERNEL_RETURN_RSP: u64 = 0;

/// The three ordinary syscall values actually observed, in trap order —
/// read back by the boot self-test to confirm the real round trip really
/// happened (ring 3 really resumed and really made each subsequent trap),
/// not just that one syscall fired.
static OBSERVED: [AtomicU64; 3] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];
static OBSERVED_COUNT: AtomicUsize = AtomicUsize::new(0);

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
/// the typed `extern "x86-interrupt"` frame). Branches on the trapped
/// `rax` *before* calling into Rust, since `record_syscall` (a normal
/// `extern "C"` function) is free to clobber `rax` as scratch — relying on
/// it surviving the call would be a real bug waiting to happen, not a
/// hypothetical one.
///
/// **A second real bug found building the original one-shot version of
/// this** (still relevant to the exit path below): the IDT entry is (by
/// `set_handler_addr`'s own documented default) an *interrupt* gate,
/// which clears `IF` automatically on entry — normally undone by the
/// `iretq` the *continue* path below already performs. The *exit* path
/// deliberately never `iretq`s, so it still needs the same explicit `sti`
/// the original fix added — otherwise `IF` stays cleared permanently the
/// instant the demo actually exits, and every later timer tick silently
/// stops firing.
#[unsafe(naked)]
pub unsafe extern "C" fn syscall_entry_stub() {
    naked_asm!(
        "cmp eax, {exit}",
        "je 3f",
        // --- Continue path: record, then iretq using the interrupt
        // frame the trap already pushed — no reconstruction needed,
        // this is exactly what a normal interrupt return does. ---
        "mov rdi, rax",
        "call {handler}",
        "iretq",
        // --- Exit path: record, then abandon ring 3 and resume the
        // kernel context enter_ring3 saved. ---
        "3:",
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
        exit = const EXIT_SYSCALL,
        handler = sym record_syscall,
        kernel_rsp = sym KERNEL_RETURN_RSP,
    )
}

extern "C" fn record_syscall(number: u64) {
    if number == EXIT_SYSCALL {
        serial_println!("[syscall] real int 0x80 EXIT trap from ring 3 — abandoning ring 3 for good");
        return;
    }
    let idx = OBSERVED_COUNT.fetch_add(1, Ordering::SeqCst);
    if idx < OBSERVED.len() {
        OBSERVED[idx].store(number, Ordering::SeqCst);
    }
    serial_println!("[syscall] real int 0x80 trap from ring 3, rax={number} — returning to ring 3");
}

/// Maps a real user-accessible code page and stack page, copies the real
/// hand-assembled ring-3 program into the code page, and actually drops
/// to CPL3 to run it. The ring-3 program makes three ordinary syscalls
/// (really resumed in ring 3 after each) and one exit syscall (which
/// really abandons ring 3). Returns the three ordinary values observed,
/// in trap order, once the exit syscall has resumed this kernel context —
/// `None` if the pages couldn't be mapped (paging not yet initialized).
pub fn run_demo_syscall() -> Option<[u64; 3]> {
    // The code page deliberately does *not* carry the NX flag — this is
    // the one page in the whole kernel that real ring-3 code must actually
    // execute out of. The stack page does: a user stack has no legitimate
    // reason to ever be fetched from, and real NX now forbids it.
    let code_flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    let stack_flags = code_flags | crate::cpu_features::nx_flag();

    crate::paging::with_mapper_and_phys(|mapper, phys| {
        crate::paging::map_page(mapper, phys, VirtAddr::new(USER_CODE_ADDR), code_flags)
    })?
    .ok()?;
    crate::paging::with_mapper_and_phys(|mapper, phys| {
        crate::paging::map_page(mapper, phys, VirtAddr::new(USER_STACK_ADDR), stack_flags)
    })?
    .ok()?;

    // The one real write this kernel (CPL0) makes into a user-accessible
    // (U/S=1) page. With real CR4.SMAP set, doing this without stac/clac
    // first is a real #PF, not a hypothetical one — verified by actually
    // removing this wrapping and observing exactly that fault; see
    // STATUS.md. `Smap::new()` returns `None` on CPUID that doesn't
    // support SMAP at all, in which case CR4.SMAP was never set either
    // (cpu_features::init) and the plain write below is already safe.
    match x86_64::instructions::smap::Smap::new() {
        Some(smap) => smap.without_smap(|| unsafe {
            core::ptr::copy_nonoverlapping(USER_PROGRAM.as_ptr(), USER_CODE_ADDR as *mut u8, USER_PROGRAM.len());
        }),
        None => unsafe {
            core::ptr::copy_nonoverlapping(USER_PROGRAM.as_ptr(), USER_CODE_ADDR as *mut u8, USER_PROGRAM.len());
        },
    }

    let selectors = crate::gdt::selectors();
    let user_cs = selectors.user_code_selector.0 as u64;
    let user_ss = selectors.user_data_selector.0 as u64;
    let user_stack_top = USER_STACK_ADDR + USER_STACK_SIZE;

    unsafe {
        enter_ring3(USER_CODE_ADDR, user_stack_top, user_cs, user_ss);
    }

    Some([
        OBSERVED[0].load(Ordering::SeqCst),
        OBSERVED[1].load(Ordering::SeqCst),
        OBSERVED[2].load(Ordering::SeqCst),
    ])
}
