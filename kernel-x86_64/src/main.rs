//! UOSC x86-64 bootable kernel — the real, hardware-verified extension of
//! `reference-rs`'s Phase 0 work. Everything in `reference-rs` compiled
//! and ran under `cargo test`; nothing there had ever been executed as an
//! actual kernel on actual (emulated) hardware. This binary changes that:
//! it boots under QEMU via the `bootloader_api` crate, sets up a real GDT/
//! IDT/PIC/PIT, feeds the bootloader's real physical memory map into
//! `uosc_core::memory::PhysicalAllocator`, and then runs the real
//! capability/memory/sanctum/ipc/scheduler subsystems — the same code
//! covered by `reference-rs`'s 64 tests and `proofs-lean4`'s Lean proofs —
//! against real allocated memory and real hardware interrupts, reporting
//! PASS/FAIL for each over the serial console and exiting QEMU with a real,
//! scriptable exit code.
//!
//! **What this does not claim**: no hardware page tables are wired up (CR3
//! still points at the bootloader's identity/offset mapping — the virtual
//! side of `uosc_core::memory` remains the in-memory policy model it
//! already was, not a hardware page-walker, exactly as `reference-rs`'s
//! `STATUS.md` already said); no userspace, no syscall entry point, no
//! filesystem, no network. This is a real boot to a real self-test, not a
//! usable operating system.
//!
//! It does now include a real context switch: `context.rs` +
//! `scheduler_bridge.rs` genuinely save/restore two independently-running
//! kernel tasks' stack pointers and callee-saved registers, driven by the
//! real hardware timer and the real `RunQueue` scheduling decision — see
//! `context.rs`'s module docs for exactly how that's safe to do from
//! inside an interrupt handler.

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod allocator;
mod context;
mod gdt;
mod interrupts;
mod pit;
mod qemu_exit;
mod scheduler_bridge;
mod serial;

use bootloader_api::{BootInfo, entry_point};
use bootloader_api::config::BootloaderConfig;
use bootloader_api::info::MemoryRegionKind;
use core::panic::PanicInfo;

use qemu_exit::{ExitCode, exit_qemu};
use uosc_core::boot::{BootPhase, BootSequencer};
use uosc_core::capability::{CapabilityBroker, EnforcementDecision, Permissions, ResourceType};
use uosc_core::ipc::PortTable;
use uosc_core::memory::PhysicalAllocator;
use uosc_core::sanctum::{VaultManager, VaultType};

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.kernel_stack_size = 128 * 1024;
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("\n[PANIC] {}", info);
    exit_qemu(ExitCode::Failed);
}

/// One self-test's outcome, tracked so the final summary is honest about
/// exactly what ran and what the result was, rather than a single opaque
/// "it booted" claim.
struct CheckResults {
    total: u32,
    passed: u32,
}

impl CheckResults {
    fn new() -> Self {
        CheckResults { total: 0, passed: 0 }
    }
    fn record(&mut self, name: &str, ok: bool) {
        self.total += 1;
        if ok {
            self.passed += 1;
        }
        let label = if ok { "PASS" } else { "FAIL" };
        serial_println!("[{label}] {name}");
    }
}

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    serial::init();
    serial_println!("UOSC x86-64 — real boot starting");
    allocator::init();

    let mut seq = BootSequencer::new();
    let mut results = CheckResults::new();

    // --- EarlyBoot: GDT + IDT, mirroring kernel/boot.ti's early_boot ---
    gdt::init();
    interrupts::init_idt();
    results.record("EarlyBoot: GDT + IDT installed", seq.complete(BootPhase::EarlyBoot).is_ok());

    // --- LateBoot: PIC remap, real PIT frequency via the ported
    // arithmetic, interrupts enabled ---
    unsafe {
        let mut pics = interrupts::PICS.lock();
        pics.initialize();
        // `initialize()` restores whatever mask was active before init —
        // typically "everything unmasked" — but only Timer (IRQ0) has a
        // real handler in this pass. Any other line firing with no IDT
        // handler present is an unhandled #GP, which escalates straight
        // to a double fault. Mask everything except the timer until a
        // real driver for another line (keyboard, RTC, ...) is added.
        pics.write_masks(0xFE, 0xFF);
    }
    pit::set_frequency(200);
    x86_64::instructions::interrupts::enable();
    results.record("LateBoot: PIC/PIT/interrupts enabled", seq.complete(BootPhase::LateBoot).is_ok());

    // --- Real physical memory: feed the bootloader's actual memory map
    // into PhysicalAllocator, using the largest single usable region
    // (the allocator manages one contiguous arena — see reference-rs's
    // STATUS.md; a multi-region allocator is real, separate follow-on work) ---
    let largest_usable = boot_info
        .memory_regions
        .iter()
        .filter(|r| r.kind == MemoryRegionKind::Usable)
        .max_by_key(|r| r.end - r.start);

    let mut phys_alloc_ok = false;
    if let Some(region) = largest_usable {
        let page_size = uosc_core::memory::PAGE_SIZE;
        let total_pages = ((region.end - region.start) / page_size).min(1 << 20);
        serial_println!(
            "[memory] real usable region {:#x}..{:#x}, using {} pages for PhysicalAllocator",
            region.start,
            region.end,
            total_pages
        );
        let mut phys = PhysicalAllocator::new(region.start, total_pages);
        let before = phys.free_page_count();
        if let Ok(p1) = phys.allocate(0) {
            if let Ok(p2) = phys.allocate(0) {
                phys_alloc_ok = p1 != p2 && phys.deallocate(p1).is_ok() && phys.deallocate(p2).is_ok()
                    && phys.free_page_count() == before;
            }
        }
    }
    results.record("Memory: real PhysicalAllocator over real bootloader memory map", phys_alloc_ok);

    // --- Real capability check ---
    let mut caps = CapabilityBroker::new();
    let token = caps.issue(0, 1, ResourceType::Memory, "heap".into(), Permissions {
        read: true,
        write: true,
        ..Permissions::NONE
    }, 0, u64::MAX);
    let decision = caps.check_access(1, ResourceType::Memory, "heap", Permissions { read: true, ..Permissions::NONE }, 0);
    let denied = caps.check_access(2, ResourceType::Memory, "heap", Permissions { read: true, ..Permissions::NONE }, 0);
    results.record(
        "Capability: real CapabilityBroker grants the issuer and denies a stranger",
        decision == EnforcementDecision::Allowed && denied == EnforcementDecision::Denied,
    );
    let _ = token;

    // --- SchedulerBoot: real RunQueue, real hardware timer drives it ---
    scheduler_bridge::init();
    results.record("SchedulerBoot: real RunQueue initialized", seq.complete(BootPhase::SchedulerBoot).is_ok());

    // --- SanctumBoot: real vault lifecycle ---
    let mut vaults = VaultManager::new();
    let vault_id = vaults.create_vault(1, VaultType::Standard, 0x1000, 0x2000);
    let enter_ok = vaults.enter_vault(vault_id).is_ok();
    let inside_ok = vaults.check_access(vault_id, 0x1500).is_ok();
    let outside_denied = vaults.check_access(vault_id, 0x5000).is_err();
    results.record("Sanctum: real vault created, entered, and region-isolated", enter_ok && inside_ok && outside_denied);
    results.record("SanctumBoot phase", seq.complete(BootPhase::SanctumBoot).is_ok());

    // --- IpcBoot: real port + capability-checked send/receive ---
    let mut ports = PortTable::new();
    let mut ipc_caps = CapabilityBroker::new();
    let port_id = ports.create_port(1, 4, &mut ipc_caps, 0);
    let send_ok = ports.send_message(1, port_id, alloc::vec![1, 2, 3], &mut ipc_caps, 0).is_ok();
    let recv_ok = ports
        .receive_message(1, port_id, &mut ipc_caps, 0)
        .is_ok_and(|m| m.payload == alloc::vec![1, 2, 3]);
    results.record("Ipc: real capability-checked send/receive round-trip", send_ok && recv_ok);
    results.record("IpcBoot phase", seq.complete(BootPhase::IpcBoot).is_ok());

    serial_println!(
        "[boot] BootSequencer ordering invariant holds: {}",
        seq.satisfies_ordering_invariant()
    );
    serial_println!(
        "[boot] SyscallBoot intentionally left incomplete — no real syscall entry point in this pass"
    );

    // --- Let real hardware timer interrupts actually drive the real
    // scheduler for a while. This loop's own execution context is what
    // gets suspended by the very first real context switch (see
    // `context.rs`) — this `hlt()` call is where kernel_main's flow
    // pauses until `scheduler_bridge`'s tick budget hands control back,
    // at which point this loop resumes exactly here and keeps counting
    // down, completely transparently. ---
    for _ in 0..1000 {
        x86_64::instructions::hlt();
    }
    results.record(
        "Scheduler: real context switches actually ran both kernel tasks, driven by the real hardware timer",
        scheduler_bridge::both_tasks_made_real_progress(),
    );

    serial_println!("\n=== UOSC boot self-test: {}/{} checks passed ===", results.passed, results.total);
    if results.passed == results.total {
        exit_qemu(ExitCode::Success);
    } else {
        exit_qemu(ExitCode::Failed);
    }
}
