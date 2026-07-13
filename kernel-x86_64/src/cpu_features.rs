//! Real CPU security features: NX (EFER.NXE + `PageTableFlags::NO_EXECUTE`),
//! SMEP, and SMAP. Until now `syscall.rs`'s own module docs flagged this
//! plainly: "the kernel can freely read/write/execute the 'user' pages it
//! maps." This module closes that gap for real, hardware-checked, not
//! aspirational — every bit set here is gated on a real CPUID probe first,
//! since setting a control-register bit the CPU doesn't actually support is
//! a real `#GP` waiting to happen, not just bad practice.
//!
//! **Must run early**: [`init`] has to execute before any page is ever
//! mapped with `PageTableFlags::NO_EXECUTE` set — bit 63 of a page-table
//! entry is architecturally reserved (and `#PF`s the first time hardware
//! walks into it) until `EFER.NXE` is actually set — and before any
//! user-accessible page exists, since SMEP/SMAP change what the kernel
//! itself is allowed to do to such a page from the instant they're set.
//! `main.rs` calls this immediately after `paging::init`, before
//! `allocator::init_bootstrap` maps the first heap page.
//!
//! **What this does and doesn't prove**: NX is applied to every data page
//! this kernel maps (kernel heap, the demand-paged region, the ring-3
//! demo's user stack) while the ring-3 demo's *code* page deliberately
//! keeps NX off — real, load-bearing, exercised every boot (the syscall
//! demo would not run at all if that were wrong). SMAP is genuinely
//! exercised too: `syscall.rs`'s one real write into a user-accessible page
//! (copying `USER_PROGRAM` in) is wrapped in real `stac`/`clac`, and this
//! was verified by actually removing that wrapping and observing a real,
//! reproducible boot-time page fault before restoring it — see
//! `STATUS.md`.
//!
//! **SMEP is now empirically fault-tested too, manually, the same way SMAP
//! was.** Nothing in the checked-in codebase attempts a supervisor-mode
//! instruction fetch from a user-accessible page — this kernel has no
//! reason to — so there's no permanent, always-passing self-test for it,
//! the same reasoning that already applies to SMAP's break-and-restore
//! check. Instead this was verified once, manually: a temporary probe
//! mapped a fresh `USER_ACCESSIBLE` page holding a single `ret` byte and
//! called directly into it from ring 0 (a plain `call`, no `iretq`, no
//! privilege change — deliberately *not* going through `syscall.rs`'s
//! ring-3 machinery, to isolate SMEP from SMAP/interrupt-gate concerns).
//! With `-cpu qemu64,+smep` (no `+smap`, so the earlier write itself
//! wouldn't also fault and confuse the result) this produced a real,
//! immediate `#PF`:
//! ```text
//! EXCEPTION: PAGE FAULT at 0x222222220000, error PageFaultErrorCode(PROTECTION_VIOLATION | INSTRUCTION_FETCH) — outside the demand-page region, cannot recover
//! ```
//! with `instruction_pointer` reported as that exact address — the fetch
//! itself faulted before executing a single instruction there. On QEMU's
//! default CPU model (SMEP unsupported, so `init` above never sets the
//! bit) the identical probe ran the `ret` and returned harmlessly, as
//! expected. The probe was then fully removed and the kernel rebuilt back
//! to a clean, 0-warning, 17/17-passing state — see `STATUS.md` for the
//! exact captured output on both configurations.

use core::arch::x86_64::__cpuid;
use core::sync::atomic::{AtomicBool, Ordering};
use x86_64::registers::control::{Cr4, Cr4Flags};
use x86_64::registers::model_specific::{Efer, EferFlags};
use x86_64::structures::paging::PageTableFlags;

use crate::serial_println;

static NX_ENABLED: AtomicBool = AtomicBool::new(false);

/// What [`init`] actually managed to turn on, each backed by a real CPUID
/// check — read back by the boot self-test to confirm the real hardware
/// state, not just that `init` was called.
pub struct CpuSecurityFeatures {
    pub nx: bool,
    pub smep: bool,
    pub smap: bool,
}

/// Probes CPUID for NX/SMEP/SMAP support and enables each one that's really
/// present: `EFER.NXE` (via `wrmsr`) for NX, `CR4.SMEP`/`CR4.SMAP` (via
/// `mov cr4`) for the other two. Never sets a bit the CPU didn't actually
/// advertise support for.
pub fn init() -> CpuSecurityFeatures {
    // CPUID.80000001H:EDX.bit20 = execute-disable (NX) available.
    let ext = __cpuid(0x8000_0001);
    let nx = ext.edx & (1 << 20) != 0;
    if nx {
        unsafe {
            Efer::update(|flags| flags.insert(EferFlags::NO_EXECUTE_ENABLE));
        }
    }
    NX_ENABLED.store(nx, Ordering::SeqCst);

    // CPUID.7.0:EBX.bit7 = SMEP, CPUID.7.0:EBX.bit20 = SMAP.
    let ext_features = __cpuid(7);
    let smep = ext_features.ebx & (1 << 7) != 0;
    let smap = ext_features.ebx & (1 << 20) != 0;
    unsafe {
        Cr4::update(|flags| {
            flags.set(Cr4Flags::SUPERVISOR_MODE_EXECUTION_PROTECTION, smep);
            flags.set(Cr4Flags::SUPERVISOR_MODE_ACCESS_PREVENTION, smap);
        });
    }

    serial_println!(
        "[cpu] NX={} (EFER.NXE) SMEP={} SMAP={} (CR4), each gated on a real CPUID check",
        nx, smep, smap
    );

    CpuSecurityFeatures { nx, smep, smap }
}

/// `PageTableFlags::NO_EXECUTE` if [`init`] found real NX support, or an
/// empty flag set otherwise — setting that bit with `EFER.NXE=0` is a real
/// reserved-bit `#PF` the first time hardware walks into the entry, not a
/// hypothetical one. Every data page this kernel maps after boot (kernel
/// heap, the demand-paged region, the ring-3 demo's user stack) ORs this
/// in; the ring-3 demo's *code* page deliberately does not.
pub fn nx_flag() -> PageTableFlags {
    if NX_ENABLED.load(Ordering::SeqCst) {
        PageTableFlags::NO_EXECUTE
    } else {
        PageTableFlags::empty()
    }
}
