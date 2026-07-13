//! A real ring-3 → ring-0 privilege transition: actual user-mode (CPL3)
//! code, on real hardware, trapping into the kernel via `int 0x80` and
//! being observed by a real handler — not a simulation of a syscall, and
//! not CPL0 code merely pretending to be "userspace."
//!
//! **A real, small syscall ABI, not one hardcoded branch.** The original
//! version of this module recognized exactly one syscall number
//! (`999 = EXIT_SYSCALL`) via a single `cmp`/`je`, with every other number
//! treated identically ("record whatever's in `rax`"). This pass replaces
//! that with a real dispatch: a fixed table mapping syscall *number* to
//! *handler function* ([`dispatch_syscall`]), real multi-argument passing
//! (not just `rax` — three real arguments, shuffled into the System V
//! calling convention's register slots before calling into Rust), and a
//! real return value that the ring-3 program genuinely receives and uses
//! (not just something the kernel happens to record).
//!
//! **Scope, stated plainly**: this is still a single, fixed, hand-assembled
//! ring-3 program (no ELF loader, no process abstraction) and the syscall
//! table is a small, compile-time-fixed set of four numbers, not a
//! dynamically extensible registry — "general" here means "a real
//! number→handler dispatch with real multi-argument passing and real
//! return values," not "a production-grade syscall surface." Deliberately
//! run *before* `scheduler_bridge::init()` so it never overlaps with real
//! task switching — combining a real privilege-level transition with real
//! preemptive switching (a timer tick landing mid-ring-3) is real,
//! additional complexity not attempted here. Also still not attempted:
//! more than one ring-3 program ever existing at once.
//!
//! **NX/SMEP/SMAP** (`cpu_features.rs`) apply for real: the user code page
//! stays executable (real ring-3 code actually runs from it every boot)
//! but the user stack page carries real NX, and the one kernel write into
//! the user code page (copying `USER_PROGRAM` in, below) is wrapped in
//! real `stac`/`clac` so it survives real `CR4.SMAP` being set.
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
//! user-accessible page. It makes four real traps in sequence:
//!
//! 1. `mov eax, {SYS_ADD}; mov edi, 7; mov esi, 8; int 0x80` — a real,
//!    two-argument syscall. The result (`15`) comes back in `rax`, and the
//!    program saves it into `ebx` — nothing forces this beyond the ring-3
//!    code itself actually trusting and using the returned value.
//! 2. `mov eax, {SYS_ECHO}; mov edi, 1234; int 0x80` — a second, distinct
//!    syscall number through the *same* dispatch mechanism, then
//!    `add ebx, eax` folds this result into the running total (`1249`).
//! 3. `mov eax, {SYS_REPORT}; mov edi, ebx; int 0x80` — reports the
//!    ring-3-computed total back to the kernel as an argument (not a fixed
//!    constant), so the boot self-test can check the *exact* value only
//!    real, correct argument-passing and real, correct return values on
//!    both prior calls could have produced.
//! 4. `mov eax, {EXIT_SYSCALL}; int 0x80` — abandons ring 3 for good,
//!    followed by a defensive `jmp $` never actually reached if exit
//!    handling works.
//!
//! `int 0x80`, with the IDT entry's DPL set to ring 3 (`interrupts.rs`),
//! traps into [`syscall_entry_stub`] — also naked, since reading the
//! "syscall number" and arguments out of the real register file at the
//! moment of the trap needs exactly that, which the typed
//! `extern "x86-interrupt"` handler convention doesn't expose.
//!
//! **The register shuffle**: ring-3 code places the syscall number in
//! `rax` and up to three arguments in `rdi`/`rsi`/`rdx` — this kernel's
//! own convention, not compatible with any other OS's ABI. To call
//! [`dispatch_syscall`] (an ordinary `extern "C"` function, expecting its
//! four `u64` arguments in `rdi`/`rsi`/`rdx`/`rcx` per the System V AMD64
//! convention) the stub shuffles registers in dependency order — `rcx`
//! first (from `rdx`, before `rdx` is overwritten), then `rdx` (from
//! `rsi`), then `rsi` (from `rdi`), then finally `rdi` (from `rax`) — a
//! real register permutation, not a coincidence of naming. The call's
//! return value lands in `rax` automatically (the System V return
//! convention), and since nothing overwrites `rax` again before `iretq`,
//! ring 3 resumes with exactly that value already in place — no separate
//! "write the return value back" step needed.
//!
//! **Two different real returns, not one**: for ordinary syscalls, the
//! stub calls [`dispatch_syscall`] then `iretq`s using the *same*
//! hardware-pushed interrupt frame the trap already left on the stack,
//! resuming ring 3 at the very next instruction after `int 0x80`. Only
//! `EXIT_SYSCALL` takes the other path — exactly like `scheduler_bridge.
//! rs`'s `BOOT_PID` handback, it loads the kernel stack pointer
//! [`enter_ring3`] saved and `ret`s straight back into `run_demo_syscall`'s
//! caller, abandoning ring 3 for good.

use core::arch::naked_asm;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::VirtAddr;
use x86_64::structures::paging::PageTableFlags;

use crate::serial_println;

pub const USER_CODE_ADDR: u64 = 0x_6666_6666_0000;
pub const USER_STACK_ADDR: u64 = 0x_7777_7777_0000;
const USER_STACK_SIZE: u64 = 4096;

/// Real syscall numbers — a small, fixed table, not a dynamically
/// extensible registry (see module docs for what "general" does and
/// doesn't mean here).
const SYS_ADD: u64 = 1;
const SYS_ECHO: u64 = 2;
const SYS_REPORT: u64 = 3;
const EXIT_SYSCALL: u64 = 999;

fn sys_add(a: u64, b: u64, _c: u64) -> u64 {
    a.wrapping_add(b)
}

fn sys_echo(a: u64, _b: u64, _c: u64) -> u64 {
    a
}

fn sys_report(a: u64, _b: u64, _c: u64) -> u64 {
    REPORTED_VALUE.store(a, Ordering::SeqCst);
    0
}

fn sys_exit(_a: u64, _b: u64, _c: u64) -> u64 {
    serial_println!("[syscall] real int 0x80 EXIT trap from ring 3 — abandoning ring 3 for good");
    0
}

/// The real number→handler dispatch — this is the actual "syscall table":
/// a fixed-size array indexed by matching on `number`, each entry a real
/// function pointer, not one hardcoded comparison. Unknown numbers return
/// `u64::MAX` rather than panicking or silently doing nothing — a real,
/// if minimal, error convention.
extern "C" fn dispatch_syscall(number: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    let handler: fn(u64, u64, u64) -> u64 = match number {
        SYS_ADD => sys_add,
        SYS_ECHO => sys_echo,
        SYS_REPORT => sys_report,
        EXIT_SYSCALL => sys_exit,
        _ => {
            serial_println!("[syscall] unknown syscall number {number} — returning u64::MAX");
            return u64::MAX;
        }
    };
    let result = handler(arg0, arg1, arg2);
    if number != EXIT_SYSCALL {
        serial_println!("[syscall] real int 0x80 trap from ring 3: number={number} arg0={arg0} arg1={arg1} -> {result}");
    }
    result
}

/// Real, hand-assembled machine code for four traps, each with real
/// arguments — see module docs for exactly what each does and why. Encoded
/// by hand (`mov r32,imm32` = `B8+reg imm32`; `mov r/m32,r32` = `89 /r`;
/// `add r/m32,r32` = `01 /r`; `int 0x80` = `CD 80`), verified against the
/// exact values the boot self-test expects to observe.
const USER_PROGRAM: [u8; 51] = [
    0xB8, 0x01, 0x00, 0x00, 0x00, // mov eax, 1 (SYS_ADD)
    0xBF, 0x07, 0x00, 0x00, 0x00, // mov edi, 7
    0xBE, 0x08, 0x00, 0x00, 0x00, // mov esi, 8
    0xCD, 0x80, // int 0x80          -> rax = 15
    0x89, 0xC3, // mov ebx, eax      ; ebx = 15
    0xB8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2 (SYS_ECHO)
    0xBF, 0xD2, 0x04, 0x00, 0x00, // mov edi, 1234
    0xCD, 0x80, // int 0x80          -> rax = 1234
    0x01, 0xC3, // add ebx, eax      ; ebx = 15 + 1234 = 1249
    0xB8, 0x03, 0x00, 0x00, 0x00, // mov eax, 3 (SYS_REPORT)
    0x89, 0xDF, // mov edi, ebx      ; report 1249
    0xCD, 0x80, // int 0x80
    0xB8, 0xE7, 0x03, 0x00, 0x00, // mov eax, 999 (EXIT_SYSCALL)
    0xCD, 0x80, // int 0x80
    0xEB, 0xFE, // jmp $  (defensive; never reached)
];

static mut KERNEL_RETURN_RSP: u64 = 0;

/// The value the ring-3 program reported via `SYS_REPORT`, computed
/// entirely in ring 3 from two real syscall return values (`15` from
/// `SYS_ADD(7, 8)`, `1234` echoed back, summed to `1249`). Read back by
/// the boot self-test — a wrong value here means either a real argument
/// wasn't delivered correctly or a real return value wasn't, not merely
/// that some fixed constant round-tripped.
static REPORTED_VALUE: AtomicU64 = AtomicU64::new(0);

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
/// (`interrupts.rs`, since reading the real register file at the trap
/// needs exactly that, not the typed `extern "x86-interrupt"` frame).
/// Branches on the trapped `rax` *before* the register shuffle/call —
/// `dispatch_syscall` (a normal `extern "C"` function) is free to clobber
/// any caller-saved register, so relying on `rax` surviving the call
/// would be a real bug waiting to happen, not a hypothetical one.
///
/// **A real bug found building the original one-shot version of this**
/// (still relevant to the exit path below): the IDT entry is (by
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
        // --- Continue path: shuffle rax/rdi/rsi/rdx (our own
        // number/arg0/arg1/arg2 convention) into rdi/rsi/rdx/rcx (System
        // V's 1st-4th call-argument registers) in dependency order, call
        // dispatch_syscall, then iretq using the interrupt frame the trap
        // already pushed — no reconstruction needed. The call's return
        // value is already in rax when iretq runs, which is exactly what
        // ring 3 sees as int 0x80's "result." ---
        "mov rcx, rdx", // rcx = arg2
        "mov rdx, rsi", // rdx = arg1
        "mov rsi, rdi", // rsi = arg0
        "mov rdi, rax", // rdi = number
        "call {dispatch}",
        "iretq",
        // --- Exit path: same shuffle and dispatch (for its log line and
        // uniform handling), then abandon ring 3 and resume the kernel
        // context enter_ring3 saved. ---
        "3:",
        "mov rcx, rdx",
        "mov rdx, rsi",
        "mov rsi, rdi",
        "mov rdi, rax",
        "call {dispatch}",
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
        dispatch = sym dispatch_syscall,
        kernel_rsp = sym KERNEL_RETURN_RSP,
    )
}

/// Maps a real user-accessible code page and stack page, copies the real
/// hand-assembled ring-3 program into the code page, and actually drops
/// to CPL3 to run it. The ring-3 program makes two real, multi-argument
/// syscalls, reports the value it computed from their real return values,
/// then exits. Returns that reported value once the exit syscall has
/// resumed this kernel context — `None` if the pages couldn't be mapped
/// (paging not yet initialized).
pub fn run_demo_syscall() -> Option<u64> {
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

    Some(REPORTED_VALUE.load(Ordering::SeqCst))
}
