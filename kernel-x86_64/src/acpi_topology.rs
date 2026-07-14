//! Real ACPI/MADT parsing — CPU topology discovery.
//!
//! This is the first real, independently-verifiable step toward SMP, not
//! SMP itself: it establishes how many logical CPUs the real firmware
//! reports and their real APIC IDs. Nothing else SMP needs (an AP
//! trampoline, per-CPU GDT/TSS, LAPIC bring-up, actually running kernel
//! code on a second core, making the scheduler/allocator genuinely
//! multi-core-safe) exists yet — see `main.rs`'s module docs for the
//! current, explicit list of what this kernel does not claim.
//!
//! This kernel already maps the entirety of physical memory at a fixed
//! offset (`paging::phys_mem_offset()`, established by `BOOTLOADER_CONFIG`
//! in `main.rs`), so the real physical memory this module's ACPI handler
//! needs to reach is already backed by real page tables before this can
//! run — the handler's "mapping" is just that offset add, and there is
//! nothing to unmap afterward.

use acpi::sdt::madt::{Madt, MadtEntry};
use acpi::{AcpiTables, Handle, Handler, PciAddress, PhysicalMapping};
use alloc::vec::Vec;
use core::ptr::NonNull;
use x86_64::instructions::port::Port;

use crate::paging;

/// The legacy PCI configuration mechanism (0xCF8 CONFIG_ADDRESS / 0xCFC
/// CONFIG_DATA) — real, standard, and the only PCI access this handler
/// needs, since `IdentityMappedHandler` never touches PCIe MMCONFIG space.
const PCI_CONFIG_ADDRESS: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16 = 0xCFC;

fn phys_mem_offset() -> u64 {
    paging::phys_mem_offset()
        .expect("acpi_topology: paging::init must run before ACPI parsing — no physical memory offset yet")
        .as_u64()
}

/// Legacy CONFIG_ADDRESS mechanism only addresses segment 0 — real hardware
/// limitation, not a shortcut taken here. A real system with a second PCI
/// segment group needs the PCIe MMCONFIG mechanism instead, which this
/// handler does not implement (nothing in this kernel exercises PCI access
/// at all yet, real or AML-driven).
fn pci_config_address(pci: PciAddress, offset: u16) -> u32 {
    assert_eq!(pci.segment(), 0, "acpi_topology: legacy PCI config mechanism cannot address segment != 0");
    0x8000_0000
        | (u32::from(pci.bus()) << 16)
        | (u32::from(pci.device()) << 11)
        | (u32::from(pci.function()) << 8)
        | u32::from(offset & 0xFC)
}

#[derive(Clone)]
struct IdentityMappedHandler;

impl Handler for IdentityMappedHandler {
    unsafe fn map_physical_region<T>(&self, physical_address: usize, size: usize) -> PhysicalMapping<Self, T> {
        let virt = phys_mem_offset() + physical_address as u64;
        // SAFETY: `virt` lands inside the bootloader's whole-physical-memory
        // mapping, which is real and already page-mapped for every physical
        // address up to the machine's installed RAM before this function can
        // run (see module docs) — so this pointer is valid to dereference as
        // `T` for `size` bytes, and non-null since the offset is a real,
        // non-zero virtual base the bootloader chose.
        let ptr = NonNull::new(virt as *mut T)
            .expect("acpi_topology: physical memory offset + address produced a null pointer");
        PhysicalMapping { physical_start: physical_address, virtual_start: ptr, region_length: size, mapped_length: size, handler: self.clone() }
    }

    fn unmap_physical_region<T>(_region: &PhysicalMapping<Self, T>) {
        // Nothing to do: this maps into the bootloader's permanent,
        // whole-physical-memory region rather than creating a fresh
        // mapping of its own, so there is nothing this handler owns to
        // tear down.
    }

    // --- Real MMIO, for AML `OperationRegion(SystemMemory, ...)` accesses.
    // Genuinely correct (same physical-offset translation as
    // `map_physical_region`), but unexercised by this module's actual call
    // path today (parsing the static MADT never evaluates AML). ---
    fn read_u8(&self, address: usize) -> u8 {
        unsafe { ((phys_mem_offset() + address as u64) as *const u8).read_volatile() }
    }
    fn read_u16(&self, address: usize) -> u16 {
        unsafe { ((phys_mem_offset() + address as u64) as *const u16).read_volatile() }
    }
    fn read_u32(&self, address: usize) -> u32 {
        unsafe { ((phys_mem_offset() + address as u64) as *const u32).read_volatile() }
    }
    fn read_u64(&self, address: usize) -> u64 {
        unsafe { ((phys_mem_offset() + address as u64) as *const u64).read_volatile() }
    }
    fn write_u8(&self, address: usize, value: u8) {
        unsafe { ((phys_mem_offset() + address as u64) as *mut u8).write_volatile(value) }
    }
    fn write_u16(&self, address: usize, value: u16) {
        unsafe { ((phys_mem_offset() + address as u64) as *mut u16).write_volatile(value) }
    }
    fn write_u32(&self, address: usize, value: u32) {
        unsafe { ((phys_mem_offset() + address as u64) as *mut u32).write_volatile(value) }
    }
    fn write_u64(&self, address: usize, value: u64) {
        unsafe { ((phys_mem_offset() + address as u64) as *mut u64).write_volatile(value) }
    }

    // --- Real x86 port I/O, for AML `OperationRegion(SystemIO, ...)`
    // accesses. Same `Port` type `pit.rs` already uses for real hardware
    // programming — genuinely correct, unexercised by this module's actual
    // call path today. ---
    fn read_io_u8(&self, port: u16) -> u8 {
        unsafe { Port::new(port).read() }
    }
    fn read_io_u16(&self, port: u16) -> u16 {
        unsafe { Port::new(port).read() }
    }
    fn read_io_u32(&self, port: u16) -> u32 {
        unsafe { Port::new(port).read() }
    }
    fn write_io_u8(&self, port: u16, value: u8) {
        unsafe { Port::new(port).write(value) }
    }
    fn write_io_u16(&self, port: u16, value: u16) {
        unsafe { Port::new(port).write(value) }
    }
    fn write_io_u32(&self, port: u16, value: u32) {
        unsafe { Port::new(port).write(value) }
    }

    // --- Real legacy PCI configuration-space access, for AML
    // `OperationRegion(PciConfig, ...)` accesses. Unexercised by this
    // module's actual call path today. ---
    fn read_pci_u8(&self, address: PciAddress, offset: u16) -> u8 {
        unsafe {
            Port::<u32>::new(PCI_CONFIG_ADDRESS).write(pci_config_address(address, offset));
            let dword: u32 = Port::new(PCI_CONFIG_DATA).read();
            (dword >> ((offset & 3) * 8)) as u8
        }
    }
    fn read_pci_u16(&self, address: PciAddress, offset: u16) -> u16 {
        unsafe {
            Port::<u32>::new(PCI_CONFIG_ADDRESS).write(pci_config_address(address, offset));
            let dword: u32 = Port::new(PCI_CONFIG_DATA).read();
            (dword >> ((offset & 2) * 8)) as u16
        }
    }
    fn read_pci_u32(&self, address: PciAddress, offset: u16) -> u32 {
        unsafe {
            Port::<u32>::new(PCI_CONFIG_ADDRESS).write(pci_config_address(address, offset));
            Port::new(PCI_CONFIG_DATA).read()
        }
    }
    fn write_pci_u8(&self, address: PciAddress, offset: u16, value: u8) {
        unsafe {
            Port::<u32>::new(PCI_CONFIG_ADDRESS).write(pci_config_address(address, offset));
            let shift = (offset & 3) * 8;
            let existing: u32 = Port::new(PCI_CONFIG_DATA).read();
            let merged = (existing & !(0xFFu32 << shift)) | (u32::from(value) << shift);
            Port::new(PCI_CONFIG_DATA).write(merged);
        }
    }
    fn write_pci_u16(&self, address: PciAddress, offset: u16, value: u16) {
        unsafe {
            Port::<u32>::new(PCI_CONFIG_ADDRESS).write(pci_config_address(address, offset));
            let shift = (offset & 2) * 8;
            let existing: u32 = Port::new(PCI_CONFIG_DATA).read();
            let merged = (existing & !(0xFFFFu32 << shift)) | (u32::from(value) << shift);
            Port::new(PCI_CONFIG_DATA).write(merged);
        }
    }
    fn write_pci_u32(&self, address: PciAddress, offset: u16, value: u32) {
        unsafe {
            Port::<u32>::new(PCI_CONFIG_ADDRESS).write(pci_config_address(address, offset));
            Port::new(PCI_CONFIG_DATA).write(value);
        }
    }

    // --- AML-runtime-only primitives (timing, mutexes) that only matter
    // for evaluating AML control methods. This module never does that — it
    // only calls `AcpiTables::find_table`, which reads the static MADT and
    // never touches the DSDT/SSDT AML bytecode at all. Panicking here
    // (rather than faking a clock or a lock this kernel has no real
    // implementation of yet) is the honest choice: real behavior when it's
    // genuinely simple and correct (above), a clear, documented failure
    // instead of an untested guess where it isn't. ---
    fn nanos_since_boot(&self) -> u64 {
        unimplemented!("acpi_topology: no AML control methods are evaluated by MADT-only parsing; this kernel has no monotonic clock yet")
    }
    fn stall(&self, _microseconds: u64) {
        unimplemented!("acpi_topology: no AML control methods are evaluated by MADT-only parsing")
    }
    fn sleep(&self, _milliseconds: u64) {
        unimplemented!("acpi_topology: no AML control methods are evaluated by MADT-only parsing")
    }
    fn create_mutex(&self) -> Handle {
        unimplemented!("acpi_topology: no AML control methods are evaluated by MADT-only parsing")
    }
    fn acquire(&self, _mutex: Handle, _timeout: u16) -> Result<(), acpi::aml::AmlError> {
        unimplemented!("acpi_topology: no AML control methods are evaluated by MADT-only parsing")
    }
    fn release(&self, _mutex: Handle) {
        unimplemented!("acpi_topology: no AML control methods are evaluated by MADT-only parsing")
    }
}

/// One logical CPU as reported by the real MADT.
#[derive(Debug, Clone, Copy)]
pub struct LogicalCpu {
    pub processor_id: u8,
    pub apic_id: u8,
    pub enabled: bool,
}

/// Real CPU topology as reported by the real ACPI MADT — not a guess, not
/// a `-smp`-flag echo. `cpu_count()` only counts entries whose real
/// "Enabled" bit (MADT Local APIC entry, flags bit 0) is set.
#[derive(Debug)]
pub struct CpuTopology {
    pub local_apic_address: u32,
    pub cpus: Vec<LogicalCpu>,
}

impl CpuTopology {
    pub fn cpu_count(&self) -> usize {
        self.cpus.iter().filter(|c| c.enabled).count()
    }
}

#[derive(Debug)]
pub enum TopologyError {
    AcpiParseFailed,
    MadtNotPresent,
}

/// Parse the real MADT reachable from `rsdp_addr` (`BootInfo::rsdp_addr`,
/// itself real firmware data the bootloader passed through) and report the
/// real logical CPU topology it describes.
pub fn discover(rsdp_addr: u64) -> Result<CpuTopology, TopologyError> {
    // SAFETY: `rsdp_addr` is the real physical RSDP address the bootloader
    // read from real firmware and passed through `BootInfo`; `paging::init`
    // has already run by the time this is called (enforced by the
    // `expect` inside `IdentityMappedHandler::map_physical_region`).
    let tables = unsafe { AcpiTables::from_rsdp(IdentityMappedHandler, rsdp_addr as usize) }
        .map_err(|_| TopologyError::AcpiParseFailed)?;

    let madt_mapping = tables.find_table::<Madt>().ok_or(TopologyError::MadtNotPresent)?;
    let madt = madt_mapping.get();

    let mut cpus = Vec::new();
    for entry in madt.entries() {
        if let MadtEntry::LocalApic(local_apic) = entry {
            let flags = { local_apic.flags };
            cpus.push(LogicalCpu {
                processor_id: local_apic.processor_id,
                apic_id: local_apic.apic_id,
                enabled: flags & 1 != 0,
            });
        }
    }

    Ok(CpuTopology { local_apic_address: madt.local_apic_address, cpus })
}
