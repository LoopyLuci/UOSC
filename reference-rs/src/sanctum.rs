//! Sanctum vaults — real port of `kernel/sanctum.ti`'s vault lifecycle and
//! isolation-checking logic.
//!
//! Everything hardware-specific in the Titan source — TLB/cache partition
//! setup, CPU context switch, the actual `enter_vault_code` assembly jump —
//! is already stubbed `Ok(())` in the source itself (`setup_tlb_isolation`,
//! `setup_cache_partition`, `configure_cpu_for_vault`, ...), so there's no
//! real hardware logic there to port, portable or otherwise. What *is*
//! real, testable logic, and what this module ports:
//!
//! - the vault registry (`create_vault`/`find`),
//! - the attestation-must-match gate on `enter_vault` — this is the actual
//!   enforcement point behind `theorem sanctum_vault_isolation`'s premise
//!   that a vault's state hasn't been tampered with between attestations,
//! - the region-based access check, which is the same shape of check
//!   `memory.rs`'s `PageTable::translate` makes for an ordinary process,
//!   applied here to a vault's memory region instead of a page table (see
//!   `../proofs-lean4/Memory.lean`'s `no_translation_no_access`, whose
//!   model already covers this case — a vault is a process address space
//!   under a different name).
//!
//! Attestation here is a real, deterministic checksum of vault config
//! state (not a cryptographic HMAC-SHA256 like the Titan source's comment
//! describes — that needs a real crypto primitive and a key-management
//! model this pass doesn't build). It's real enough to catch tampering
//! between attestations, which is the only property `enter_vault` actually
//! needs to enforce.

use alloc::collections::BTreeMap;

pub type VaultId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultType {
    Standard,
    Privileged,
    Isolated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultError {
    VaultNotFound,
    AttestationFailed,
    OutsideVaultRegion,
}

#[derive(Debug, Clone, Copy)]
pub struct VaultConfig {
    pub vault_id: VaultId,
    pub owner_pid: u64,
    pub vault_type: VaultType,
    pub memory_start: u64,
    pub memory_end: u64,
}

/// A real, deterministic checksum of the fields that must not change
/// between attestations. Not cryptographically secure — see module docs —
/// but sufficient to detect any in-place tamper of the config this crate
/// itself would make, which is what `enter_vault`'s check actually gates.
fn attest(config: &VaultConfig) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    for word in [
        config.vault_id,
        config.owner_pid,
        config.vault_type as u64,
        config.memory_start,
        config.memory_end,
    ] {
        h ^= word;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[derive(Debug, Clone, Copy)]
struct VaultState {
    config: VaultConfig,
    attestation_hash: u64,
}

/// Real, working port of `VaultManager` + the vault lifecycle functions.
#[derive(Debug, Default)]
pub struct VaultManager {
    vaults: BTreeMap<VaultId, VaultState>,
    next_vault_id: VaultId,
}

impl VaultManager {
    pub fn new() -> Self {
        VaultManager { vaults: BTreeMap::new(), next_vault_id: 1 }
    }

    /// `create_vault`. Attestation is computed and stored at creation time,
    /// exactly like the Titan source's `create_attestation` call in
    /// `create_vault`.
    pub fn create_vault(
        &mut self,
        owner_pid: u64,
        vault_type: VaultType,
        memory_start: u64,
        memory_end: u64,
    ) -> VaultId {
        let vault_id = self.next_vault_id;
        self.next_vault_id += 1;
        let config = VaultConfig { vault_id, owner_pid, vault_type, memory_start, memory_end };
        let attestation_hash = attest(&config);
        self.vaults.insert(vault_id, VaultState { config, attestation_hash });
        vault_id
    }

    /// `enter_vault`'s real enforcement point: recompute the attestation
    /// from current config and refuse entry if it doesn't match what was
    /// stored. The Titan source performs this exact check
    /// (`kernel/sanctum.ti:109-113`) before jumping to vault code; this
    /// port keeps the check and drops only the unportable jump itself.
    pub fn enter_vault(&self, vault_id: VaultId) -> Result<(), VaultError> {
        let vault = self.vaults.get(&vault_id).ok_or(VaultError::VaultNotFound)?;
        if attest(&vault.config) != vault.attestation_hash {
            return Err(VaultError::AttestationFailed);
        }
        Ok(())
    }

    /// `attest_vault`: recompute and store a fresh attestation.
    pub fn attest_vault(&mut self, vault_id: VaultId) -> Result<u64, VaultError> {
        let vault = self.vaults.get_mut(&vault_id).ok_or(VaultError::VaultNotFound)?;
        vault.attestation_hash = attest(&vault.config);
        Ok(vault.attestation_hash)
    }

    /// Simulates state corruption between attestations — a test hook only
    /// a compromised vault (or a bug elsewhere) would ever trigger in a
    /// real system. Exists so `enter_vault`'s failure path is actually
    /// exercised rather than only ever taking the happy path.
    #[cfg(test)]
    fn corrupt_stored_attestation(&mut self, vault_id: VaultId) {
        if let Some(vault) = self.vaults.get_mut(&vault_id) {
            vault.attestation_hash ^= 0xFFFF_FFFF_FFFF_FFFF;
        }
    }

    /// **`theorem sanctum_vault_isolation`**: an address outside the
    /// vault's memory region is never accessible through it, regardless of
    /// access type requested. Mirrors `PageTable::translate`'s
    /// bounds-then-permission check shape from `memory.rs`, specialized to
    /// a single contiguous region since that's what a vault actually is.
    pub fn check_access(&self, vault_id: VaultId, vaddr: u64) -> Result<(), VaultError> {
        let vault = self.vaults.get(&vault_id).ok_or(VaultError::VaultNotFound)?;
        if vaddr >= vault.config.memory_start && vaddr < vault.config.memory_end {
            Ok(())
        } else {
            Err(VaultError::OutsideVaultRegion)
        }
    }

    pub fn config(&self, vault_id: VaultId) -> Option<VaultConfig> {
        self.vaults.get(&vault_id).map(|v| v.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn create_then_enter_a_fresh_vault_succeeds() {
        let mut mgr = VaultManager::new();
        let id = mgr.create_vault(1, VaultType::Standard, 0x1000, 0x2000);
        assert_eq!(mgr.enter_vault(id), Ok(()));
    }

    #[test]
    fn enter_unknown_vault_is_not_found() {
        let mgr = VaultManager::new();
        assert_eq!(mgr.enter_vault(999), Err(VaultError::VaultNotFound));
    }

    #[test]
    fn tampered_attestation_denies_entry() {
        let mut mgr = VaultManager::new();
        let id = mgr.create_vault(1, VaultType::Isolated, 0x1000, 0x2000);
        mgr.corrupt_stored_attestation(id);
        assert_eq!(mgr.enter_vault(id), Err(VaultError::AttestationFailed));
    }

    #[test]
    fn re_attesting_after_corruption_restores_entry() {
        let mut mgr = VaultManager::new();
        let id = mgr.create_vault(1, VaultType::Isolated, 0x1000, 0x2000);
        mgr.corrupt_stored_attestation(id);
        assert!(mgr.enter_vault(id).is_err());
        mgr.attest_vault(id).unwrap();
        assert_eq!(mgr.enter_vault(id), Ok(()));
    }

    #[test]
    fn access_inside_region_is_allowed() {
        let mut mgr = VaultManager::new();
        let id = mgr.create_vault(1, VaultType::Standard, 0x1000, 0x2000);
        assert_eq!(mgr.check_access(id, 0x1500), Ok(()));
    }

    #[test]
    fn access_outside_region_is_denied() {
        let mut mgr = VaultManager::new();
        let id = mgr.create_vault(1, VaultType::Standard, 0x1000, 0x2000);
        assert_eq!(mgr.check_access(id, 0x5000), Err(VaultError::OutsideVaultRegion));
        // Exactly at the end boundary (exclusive) must also be denied.
        assert_eq!(mgr.check_access(id, 0x2000), Err(VaultError::OutsideVaultRegion));
    }

    proptest! {
        /// theorem sanctum_vault_isolation, generalized: for any region and
        /// any address, access is allowed if and only if the address falls
        /// in [start, end) — never a false allow outside it.
        #[test]
        fn access_check_matches_region_bounds_exactly(
            start in 0u64..1_000_000,
            len in 1u64..10_000,
            probe_offset in -10_000i64..20_000i64,
        ) {
            let end = start + len;
            let mut mgr = VaultManager::new();
            let id = mgr.create_vault(1, VaultType::Standard, start, end);
            let vaddr = (start as i64 + probe_offset).max(0) as u64;
            let expect_allowed = vaddr >= start && vaddr < end;
            let result = mgr.check_access(id, vaddr);
            prop_assert_eq!(result.is_ok(), expect_allowed);
        }
    }
}
