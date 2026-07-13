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
//! **What this does not claim**: no general syscall ABI, no process/exit
//! semantics, no filesystem, no network stack, no SMP. This is a real
//! boot to a real self-test, not a usable operating system.
//!
//! It does now include a real context switch with real dynamic task
//! creation and real task exit (`context.rs` + `scheduler_bridge.rs`
//! genuinely save/restore independently-running kernel tasks' stack
//! pointers and callee-saved registers, driven by the real hardware
//! timer, within a fixed-size task pool), real hardware page tables
//! (`paging.rs`): a real `OffsetPageTable` over the CPU's actual CR3, a
//! kernel heap that's really mapped page-by-page instead of static BSS
//! (`allocator.rs`), a real page fault deliberately triggered and
//! demand-paged by `interrupts.rs`'s handler, and a real page unmap that
//! genuinely frees the physical frame back to the shared allocator — a
//! real ring-3 → ring-0 privilege transition (`syscall.rs`): actual CPL3
//! code, on real hardware, trapping into the kernel via a real `int 0x80`
//! and being observed by a real handler — and real NX/SMEP/SMAP
//! (`cpu_features.rs`): real `EFER.NXE` and `CR4.SMEP`/`CR4.SMAP`, each
//! gated on a real CPUID check, with every data page this kernel maps
//! (heap, demand-paged region, the ring-3 demo's user stack) actually
//! marked non-executable and the one real kernel write into a
//! user-accessible page wrapped in real `stac`/`clac`.

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod allocator;
mod context;
mod cpu_features;
mod gdt;
mod interrupts;
mod paging;
mod pit;
mod qemu_exit;
mod scheduler_bridge;
mod serial;
mod syscall;

use bootloader_api::{BootInfo, entry_point};
use bootloader_api::config::{BootloaderConfig, Mapping};
use bootloader_api::info::MemoryRegionKind;
use core::panic::PanicInfo;
use core::sync::atomic::Ordering;
use x86_64::VirtAddr;

use qemu_exit::{ExitCode, exit_qemu};
use uosc_core::boot::{BootPhase, BootSequencer};
use uosc_core::capability::{CapabilityBroker, EnforcementDecision, Permissions, ResourceType};
use uosc_core::ipc::PortTable;
use uosc_core::sanctum::{VaultManager, VaultType};

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.kernel_stack_size = 128 * 1024;
    // Ask the bootloader to map the entire real physical address space at
    // some virtual offset it picks (`Mapping::Dynamic`) and report that
    // offset back in `BootInfo::physical_memory_offset`. Without this,
    // there is no way to reach arbitrary physical frames (e.g. a
    // newly-allocated page table's own backing frame) from virtual
    // addresses at all — `paging.rs` depends on it.
    config.mappings.physical_memory = Some(Mapping::Dynamic);
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

    // --- Real hardware page tables: build the OffsetPageTable first (no
    // heap needed for this part — see paging.rs). ---
    let physical_memory_offset = VirtAddr::new(
        boot_info
            .physical_memory_offset
            .into_option()
            .expect("bootloader did not map physical memory — check BOOTLOADER_CONFIG.mappings.physical_memory"),
    );
    let mut mapper = unsafe { paging::init(physical_memory_offset) };

    // --- Real NX/SMEP/SMAP, each gated on a real CPUID check. Must run
    // before any page is mapped with NO_EXECUTE (allocator::init_bootstrap,
    // right below, is the very first) and before any user-accessible page
    // exists (syscall::run_demo_syscall, much later) — see cpu_features.rs
    // module docs. ---
    let cpu_security = cpu_features::init();

    let largest_usable = boot_info
        .memory_regions
        .iter()
        .filter(|r| r.kind == MemoryRegionKind::Usable)
        .max_by_key(|r| r.end - r.start)
        .expect("no usable memory region reported by the bootloader");
    let region_start = largest_usable.start;
    let total_pages = ((largest_usable.end - largest_usable.start) / uosc_core::memory::PAGE_SIZE).min(1 << 20);
    serial_println!(
        "[memory] real usable region {:#x}..{:#x}, using {} pages for the real PhysicalAllocator",
        largest_usable.start,
        largest_usable.end,
        total_pages
    );

    // --- Phase 1 of the kernel heap: a heap-free bump allocator maps the
    // bootstrap heap, since the real, persistent PhysicalAllocator that
    // will come next itself needs a working heap to construct (see
    // allocator.rs's module docs for the two real bugs this took to get
    // right). Its frames come off the front of the real usable region;
    // the real PhysicalAllocator below is handed everything *after* them,
    // so nothing double-allocates those pages. ---
    let bootstrap_pages = allocator::BOOTSTRAP_HEAP_SIZE / uosc_core::memory::PAGE_SIZE;
    let mut bump = paging::BumpFrameAllocator::new(region_start);
    allocator::init_bootstrap(&mut mapper, &mut bump);

    // --- Now that a real heap exists, the real, persistent
    // PhysicalAllocator can actually be constructed. ---
    paging::store_globals(
        mapper,
        region_start + bootstrap_pages * uosc_core::memory::PAGE_SIZE,
        total_pages - bootstrap_pages,
    );

    // --- Phase 2 of the kernel heap: extend the same live heap with more
    // real pages, now that the real PhysicalAllocator is available. ---
    allocator::extend_with_real_pages();

    let mut seq = BootSequencer::new();
    let mut results = CheckResults::new();

    // --- Real NX/SMEP/SMAP: read back the actual CR4/EFER hardware state
    // (not just "cpu_features::init() was called") and confirm every bit
    // CPUID said was supported really did get set. The kernel heap mapped
    // just above already has NO_EXECUTE on it if cpu_security.nx is true —
    // this kernel booting cleanly at all past that point is itself real
    // evidence the EFER.NXE-before-NO_EXECUTE-mapping ordering held. ---
    let cpu_security_ok = {
        use x86_64::registers::control::{Cr4, Cr4Flags};
        use x86_64::registers::model_specific::{Efer, EferFlags};
        let efer = Efer::read();
        let cr4 = Cr4::read();
        cpu_security.nx == efer.contains(EferFlags::NO_EXECUTE_ENABLE)
            && cpu_security.smep == cr4.contains(Cr4Flags::SUPERVISOR_MODE_EXECUTION_PROTECTION)
            && cpu_security.smap == cr4.contains(Cr4Flags::SUPERVISOR_MODE_ACCESS_PREVENTION)
    };
    results.record(
        "CpuSecurity: real EFER.NXE/CR4.SMEP/CR4.SMAP match what CPUID said was supported",
        cpu_security_ok,
    );

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

    // --- Real physical memory: the alloc/dealloc round-trip now runs
    // against the one persistent global PhysicalAllocator that paging and
    // the heap already share, not a disposable scratch instance. ---
    let phys_alloc_ok = paging::with_phys(|phys| {
        let before = phys.free_page_count();
        match (phys.allocate(0), phys.allocate(0)) {
            (Ok(p1), Ok(p2)) => {
                p1 != p2
                    && phys.deallocate(p1).is_ok()
                    && phys.deallocate(p2).is_ok()
                    && phys.free_page_count() == before
            }
            _ => false,
        }
    })
    .unwrap_or(false);
    results.record("Memory: real PhysicalAllocator over real bootloader memory map", phys_alloc_ok);

    // --- Real hardware page tables: the kernel heap allocated above is
    // already proof mapping works, but a direct check makes the causality
    // explicit — allocate a real, multi-page Vec through the real
    // #[global_allocator], write recognizable values across a range wide
    // enough to span more than one of the pages allocator::init mapped,
    // and read them back. ---
    let heap_ok = {
        let mut v: alloc::vec::Vec<u64> = alloc::vec::Vec::with_capacity(4096);
        for i in 0..4096u64 {
            v.push(i.wrapping_mul(0x9E3779B97F4A7C15));
        }
        v.iter().enumerate().all(|(i, &x)| x == (i as u64).wrapping_mul(0x9E3779B97F4A7C15))
    };
    results.record("Paging: real kernel heap, mapped through real hardware page tables, holds real data", heap_ok);

    // --- Real page fault, deliberately triggered and demand-paged. The
    // write below faults on real hardware (nothing has mapped this
    // address yet), the real page_fault_handler catches it, maps a real
    // page on the spot, and the faulting instruction genuinely resumes —
    // the read-back below only succeeds if that whole real recovery path
    // actually ran. ---
    let page_fault_ok = {
        let ptr = paging::DEMAND_PAGE_REGION_START as *mut u64;
        unsafe {
            ptr.write_volatile(0xDEAD_BEEF_CAFE_D00D);
        }
        let value_ok = unsafe { ptr.read_volatile() } == 0xDEAD_BEEF_CAFE_D00D;
        let handler_ran = interrupts::PAGE_FAULTS_DEMAND_PAGED.load(Ordering::SeqCst) == 1;
        value_ok && handler_ran
    };
    results.record(
        "PageFault: a real #PF was triggered and demand-paged by the real handler, then resumed",
        page_fault_ok,
    );

    // --- Real page unmapping: the counterpart to the demand-paged mapping
    // just above. Unmap the exact same page, confirm the real physical
    // frame actually came back to the shared PhysicalAllocator (free count
    // goes up by one), then write through the same pointer again — since
    // the page table entry is genuinely gone, this is a real, *second* #PF
    // at the same address, not a no-op. The demand-page handler catches it
    // again (still inside the designated region) and maps a fresh page, so
    // PAGE_FAULTS_DEMAND_PAGED should read 2, not 1 — proof the unmap
    // really removed the mapping rather than just relabeling it. ---
    let unmap_ok = {
        let free_before = paging::with_phys(|phys| phys.free_page_count()).unwrap_or(0);
        let page_vaddr = VirtAddr::new(paging::DEMAND_PAGE_REGION_START);
        let unmapped = paging::with_mapper_and_phys(|mapper, phys| paging::unmap_page(mapper, phys, page_vaddr))
            .map(|r| r.is_ok())
            .unwrap_or(false);
        let free_after = paging::with_phys(|phys| phys.free_page_count()).unwrap_or(0);

        let ptr = paging::DEMAND_PAGE_REGION_START as *mut u64;
        unsafe {
            ptr.write_volatile(0xFEED_FACE_0BAD_F00D);
        }
        let value_ok = unsafe { ptr.read_volatile() } == 0xFEED_FACE_0BAD_F00D;
        let refaulted = interrupts::PAGE_FAULTS_DEMAND_PAGED.load(Ordering::SeqCst) == 2;

        unmapped && free_after == free_before + 1 && value_ok && refaulted
    };
    results.record(
        "Unmap: a real page unmap freed the real physical frame, and the address really re-faulted",
        unmap_ok,
    );

    // --- Real ring-3 → ring-0 privilege transition, deliberately run
    // before scheduler_bridge::init() below — see syscall.rs's module
    // docs for why real preemption and this demo don't overlap in this
    // pass. A real hand-assembled program runs at CPL3, makes three real
    // syscalls (really resumed in ring 3 after each one via a real
    // iretq), then a fourth "exit" syscall that really abandons ring 3
    // for good. Passing requires all three ordinary values to have been
    // observed, in order — proof the round trip genuinely repeated, not
    // just that one trap fired. ---
    let syscall_ok = syscall::run_demo_syscall() == Some([10, 20, 30]);
    results.record(
        "Syscall: real CPL3 code made 3 real round-trip syscalls + 1 real exit trap via int 0x80",
        syscall_ok,
    );

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
        "[boot] SyscallBoot: a real int 0x80 entry point now exists (see the Syscall check above) — \
         BootSequencer's SyscallBoot phase itself is still not completed, since there is no general \
         syscall ABI or process model behind it yet"
    );

    // --- Let real hardware timer interrupts actually drive the real
    // scheduler for a while. In practice the very first real context
    // switch (see `context.rs`) fires well before this loop even starts —
    // any timer tick from the moment `scheduler_bridge::init()` populates
    // the RunQueue onward is eligible, so kernel_main's own flow can get
    // suspended mid-instruction anywhere after that point (observed: mid-
    // way through the Sanctum/Ipc checks above). Wherever it happens,
    // `scheduler_bridge`'s tick budget hands control back to exactly that
    // suspended point once it's spent, completely transparently — this
    // loop is just where kernel_main is guaranteed to still be waiting if
    // the switch-away happened even earlier than here. ---
    for _ in 0..1000 {
        x86_64::instructions::hlt();
    }
    results.record(
        "Scheduler: real context switches actually ran both kernel tasks, driven by the real hardware timer",
        scheduler_bridge::both_tasks_made_real_progress(),
    );
    results.record(
        "TaskExit: a real dynamically spawned task ran, then really exited and left the real RunQueue",
        scheduler_bridge::task_c_really_exited(),
    );

    serial_println!("\n=== UOSC boot self-test: {}/{} checks passed ===", results.passed, results.total);
    if results.passed == results.total {
        exit_qemu(ExitCode::Success);
    } else {
        exit_qemu(ExitCode::Failed);
    }
}
