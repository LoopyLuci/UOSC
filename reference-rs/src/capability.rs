//! Capability-based security model — real port of `kernel/capability.ti`.
//!
//! The Titan source defines the full data model (`CapabilityToken`,
//! `CapabilityBroker`, `EnforcementDecision`, ...) but its `interface
//! CapabilityEnforcer` block is a signature list with no bodies — there is
//! no actual enforcement logic to port. Everything in [`CapabilityBroker`]
//! below is a new, real implementation of that interface, written to
//! satisfy the specific properties UOSC's own proof file claims:
//!
//! - `theorem process_capability_confinement` → [`CapabilityBroker::check_access`]
//! - `theorem capability_revocation` → [`CapabilityBroker::revoke`]
//! - `theorem capability_delegation` → [`CapabilityBroker::delegate`]
//!
//! This module deliberately takes the current time as a parameter rather
//! than reading a wall clock, so it has no hardware dependency and stays
//! portable and directly testable.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

pub type TokenId = u64;
pub type PrincipalId = u64;
pub type Timestamp = u64;

/// Mirrors `capability.ti`'s `ResourceType` enum exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResourceType {
    FileSystem,
    Network,
    Gpu,
    Usb,
    Audio,
    Input,
    Hardware,
    System,
    Ipc,
    Memory,
}

/// Mirrors `capability.ti`'s `Permissions` struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Permissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
    pub list: bool,
    pub create: bool,
    pub delete: bool,
    pub modify: bool,
    pub delegate: bool,
}

impl Permissions {
    pub const NONE: Permissions = Permissions {
        read: false,
        write: false,
        execute: false,
        list: false,
        create: false,
        delete: false,
        modify: false,
        delegate: false,
    };

    /// True if every bit set in `other` is also set in `self` — used to
    /// enforce that a delegated grant can never exceed its source (the
    /// property `capability_delegation` claims).
    pub fn covers(&self, other: &Permissions) -> bool {
        (!other.read || self.read)
            && (!other.write || self.write)
            && (!other.execute || self.execute)
            && (!other.list || self.list)
            && (!other.create || self.create)
            && (!other.delete || self.delete)
            && (!other.modify || self.modify)
            && (!other.delegate || self.delegate)
    }

    /// Bitwise AND — the actual set of permissions a delegation can carry.
    pub fn intersect(&self, other: &Permissions) -> Permissions {
        Permissions {
            read: self.read && other.read,
            write: self.write && other.write,
            execute: self.execute && other.execute,
            list: self.list && other.list,
            create: self.create && other.create,
            delete: self.delete && other.delete,
            modify: self.modify && other.modify,
            delegate: self.delegate && other.delegate,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityToken {
    pub token_id: TokenId,
    pub issuer: PrincipalId,
    pub subject: PrincipalId,
    pub resource_type: ResourceType,
    pub resource_path: String,
    pub permissions: Permissions,
    pub issued_at: Timestamp,
    pub expires_at: Timestamp,
    pub revoked: bool,
}

impl CapabilityToken {
    fn is_live(&self, now: Timestamp) -> bool {
        !self.revoked && now < self.expires_at
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnforcementDecision {
    Allowed,
    Denied,
    Revoked,
    Expired,
}

#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub timestamp: Timestamp,
    pub subject: PrincipalId,
    pub resource_type: ResourceType,
    pub resource_path: String,
    pub decision: EnforcementDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityError {
    NotFound,
    NotDelegatable,
    ExceedsSourcePermissions,
    IssuerLacksPermissions,
    PathNotWithinSourceScope,
}

/// Real, working port of `interface CapabilityEnforcer`.
#[derive(Debug, Default)]
pub struct CapabilityBroker {
    tokens: BTreeMap<TokenId, CapabilityToken>,
    audit_log: Vec<AuditEntry>,
    next_token_id: TokenId,
}

impl CapabilityBroker {
    pub fn new() -> Self {
        CapabilityBroker {
            tokens: BTreeMap::new(),
            audit_log: Vec::new(),
            next_token_id: 1,
        }
    }

    /// `interface CapabilityEnforcer::issue_capability`.
    #[allow(clippy::too_many_arguments)] // mirrors CapabilityToken's own field list from capability.ti 1:1
    pub fn issue(
        &mut self,
        issuer: PrincipalId,
        subject: PrincipalId,
        resource_type: ResourceType,
        resource_path: String,
        permissions: Permissions,
        now: Timestamp,
        expires_at: Timestamp,
    ) -> TokenId {
        let token_id = self.next_token_id;
        self.next_token_id += 1;
        self.tokens.insert(
            token_id,
            CapabilityToken {
                token_id,
                issuer,
                subject,
                resource_type,
                resource_path,
                permissions,
                issued_at: now,
                expires_at,
                revoked: false,
            },
        );
        token_id
    }

    /// `interface CapabilityEnforcer::revoke_capability`.
    ///
    /// Corresponds to `theorem capability_revocation`: once this returns,
    /// every subsequent [`check_access`](Self::check_access) call against
    /// this token id is denied — there is no path in this implementation
    /// that reads `revoked` and treats `true` as anything but final.
    pub fn revoke(&mut self, token_id: TokenId) -> Result<(), CapabilityError> {
        let token = self.tokens.get_mut(&token_id).ok_or(CapabilityError::NotFound)?;
        token.revoked = true;
        Ok(())
    }

    /// `interface CapabilityEnforcer::verify_token`.
    pub fn verify(&self, token_id: TokenId, now: Timestamp) -> bool {
        self.tokens.get(&token_id).is_some_and(|t| t.is_live(now))
    }

    /// `interface CapabilityEnforcer::check_access`.
    ///
    /// Corresponds to `theorem process_capability_confinement`: a subject is
    /// allowed only if a live (unrevoked, unexpired) token it holds grants
    /// the requested resource type, a covering path, and the requested
    /// permission bit.
    pub fn check_access(
        &mut self,
        subject: PrincipalId,
        resource_type: ResourceType,
        resource_path: &str,
        needs: Permissions,
        now: Timestamp,
    ) -> EnforcementDecision {
        let decision = self.decide(subject, resource_type, resource_path, needs, now);
        self.audit_log.push(AuditEntry {
            timestamp: now,
            subject,
            resource_type,
            resource_path: String::from(resource_path),
            decision,
        });
        decision
    }

    fn decide(
        &self,
        subject: PrincipalId,
        resource_type: ResourceType,
        resource_path: &str,
        needs: Permissions,
        now: Timestamp,
    ) -> EnforcementDecision {
        let mut saw_revoked = false;
        let mut saw_expired = false;

        for token in self.tokens.values() {
            if token.subject != subject
                || token.resource_type != resource_type
                || !path_covers(&token.resource_path, resource_path)
            {
                continue;
            }
            if token.revoked {
                saw_revoked = true;
                continue;
            }
            if now >= token.expires_at {
                saw_expired = true;
                continue;
            }
            if token.permissions.covers(&needs) {
                return EnforcementDecision::Allowed;
            }
        }

        if saw_revoked {
            EnforcementDecision::Revoked
        } else if saw_expired {
            EnforcementDecision::Expired
        } else {
            EnforcementDecision::Denied
        }
    }

    /// `interface CapabilityEnforcer::delegate_capability`.
    ///
    /// Corresponds to `theorem capability_delegation`: the returned token's
    /// permissions and resource scope are always a subset of the source
    /// token's, never wider — enforced structurally, not by convention.
    pub fn delegate(
        &mut self,
        source_token_id: TokenId,
        delegatee: PrincipalId,
        requested: Permissions,
        now: Timestamp,
    ) -> Result<TokenId, CapabilityError> {
        let source = self
            .tokens
            .get(&source_token_id)
            .ok_or(CapabilityError::NotFound)?;

        if !source.is_live(now) {
            return Err(CapabilityError::NotFound);
        }
        if !source.permissions.delegate {
            return Err(CapabilityError::NotDelegatable);
        }

        let granted = source.permissions.intersect(&requested);
        if granted != requested {
            return Err(CapabilityError::ExceedsSourcePermissions);
        }

        let new_id = self.issue(
            source.subject,
            delegatee,
            source.resource_type,
            source.resource_path.clone(),
            granted,
            now,
            source.expires_at,
        );
        Ok(new_id)
    }

    pub fn audit_log(&self) -> &[AuditEntry] {
        &self.audit_log
    }
}

/// A granted path of `"/home/user/*"` covers a request for
/// `"/home/user/Documents/file.txt"`; an exact path covers only itself.
fn path_covers(granted: &str, requested: &str) -> bool {
    match granted.strip_suffix('*') {
        Some(prefix) => requested.starts_with(prefix),
        None => granted == requested,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perm(read: bool, write: bool) -> Permissions {
        Permissions { read, write, ..Permissions::NONE }
    }

    #[test]
    fn confinement_denies_without_any_grant() {
        let mut broker = CapabilityBroker::new();
        let d = broker.check_access(1, ResourceType::FileSystem, "/etc/passwd", perm(true, false), 0);
        assert_eq!(d, EnforcementDecision::Denied);
    }

    #[test]
    fn confinement_allows_exact_covering_grant() {
        let mut broker = CapabilityBroker::new();
        broker.issue(0, 1, ResourceType::FileSystem, "/home/user/*".into(), perm(true, true), 0, 1000);
        let d = broker.check_access(1, ResourceType::FileSystem, "/home/user/doc.txt", perm(true, false), 5);
        assert_eq!(d, EnforcementDecision::Allowed);
    }

    #[test]
    fn confinement_denies_insufficient_permission_bit() {
        let mut broker = CapabilityBroker::new();
        broker.issue(0, 1, ResourceType::FileSystem, "/home/user/*".into(), perm(true, false), 0, 1000);
        // Granted read-only; requesting write must be denied.
        let d = broker.check_access(1, ResourceType::FileSystem, "/home/user/doc.txt", perm(true, true), 5);
        assert_eq!(d, EnforcementDecision::Denied);
    }

    #[test]
    fn confinement_denies_outside_granted_scope() {
        let mut broker = CapabilityBroker::new();
        broker.issue(0, 1, ResourceType::FileSystem, "/home/user/*".into(), perm(true, true), 0, 1000);
        let d = broker.check_access(1, ResourceType::FileSystem, "/etc/passwd", perm(true, false), 5);
        assert_eq!(d, EnforcementDecision::Denied);
    }

    #[test]
    fn revocation_is_immediate_and_final() {
        let mut broker = CapabilityBroker::new();
        let id = broker.issue(0, 1, ResourceType::Network, "*".into(), perm(true, true), 0, 1000);
        assert_eq!(
            broker.check_access(1, ResourceType::Network, "any", perm(true, false), 5),
            EnforcementDecision::Allowed
        );
        broker.revoke(id).unwrap();
        assert_eq!(
            broker.check_access(1, ResourceType::Network, "any", perm(true, false), 6),
            EnforcementDecision::Revoked
        );
    }

    #[test]
    fn expiry_is_enforced() {
        let mut broker = CapabilityBroker::new();
        broker.issue(0, 1, ResourceType::Gpu, "*".into(), perm(true, true), 0, 100);
        assert_eq!(
            broker.check_access(1, ResourceType::Gpu, "any", perm(true, false), 100),
            EnforcementDecision::Expired
        );
    }

    #[test]
    fn delegation_cannot_exceed_source_permissions() {
        let mut broker = CapabilityBroker::new();
        let id = broker.issue(
            0,
            1,
            ResourceType::FileSystem,
            "/home/user/*".into(),
            Permissions { read: true, delegate: true, ..Permissions::NONE },
            0,
            1000,
        );
        // Delegatee asks for write too — source never had write, so this must fail.
        let result = broker.delegate(id, 2, perm(true, true), 5);
        assert_eq!(result, Err(CapabilityError::ExceedsSourcePermissions));
    }

    #[test]
    fn delegation_denied_without_delegate_bit() {
        let mut broker = CapabilityBroker::new();
        let id = broker.issue(0, 1, ResourceType::FileSystem, "/home/user/*".into(), perm(true, true), 0, 1000);
        let result = broker.delegate(id, 2, perm(true, false), 5);
        assert_eq!(result, Err(CapabilityError::NotDelegatable));
    }

    #[test]
    fn delegation_grants_a_real_working_subset_capability() {
        let mut broker = CapabilityBroker::new();
        let id = broker.issue(
            0,
            1,
            ResourceType::FileSystem,
            "/home/user/*".into(),
            Permissions { read: true, write: true, delegate: true, ..Permissions::NONE },
            0,
            1000,
        );
        let delegated = broker.delegate(id, 2, perm(true, false), 5).unwrap();
        assert!(broker.verify(delegated, 6));
        assert_eq!(
            broker.check_access(2, ResourceType::FileSystem, "/home/user/x", perm(true, false), 6),
            EnforcementDecision::Allowed
        );
        // Delegatee never got write.
        assert_eq!(
            broker.check_access(2, ResourceType::FileSystem, "/home/user/x", perm(false, true), 6),
            EnforcementDecision::Denied
        );
    }

    #[test]
    fn revoking_source_does_not_touch_an_independently_issued_delegate_token() {
        // Delegation issues a *new* token; revoking the source must not
        // implicitly revoke it (that's a distinct, harder property this
        // module does not claim — documented instead of silently assumed).
        let mut broker = CapabilityBroker::new();
        let source = broker.issue(
            0,
            1,
            ResourceType::FileSystem,
            "/home/user/*".into(),
            Permissions { read: true, delegate: true, ..Permissions::NONE },
            0,
            1000,
        );
        let delegated = broker.delegate(source, 2, perm(true, false), 5).unwrap();
        broker.revoke(source).unwrap();
        assert!(broker.verify(delegated, 6), "documents current behavior: cascading revocation is not implemented");
    }
}
