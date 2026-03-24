/// Privileged administration: admin rotation, token update, pause toggle, drain.
///
/// # Centralization disclosure (for users / auditors)
///
/// Community policyholders govern claim outcomes via DAO voting — no admin
/// override exists on individual claims.  However, the following protocol
/// parameters remain admin-controlled in the MVP:
///
/// - Token contract address (treasury asset)
/// - Pause / unpause (emergency circuit-breaker)
/// - Admin key itself (rotation)
/// - Treasury drain (emergency fund recovery)
///
/// This is a deliberate MVP trade-off.  The seams for future decentralisation
/// are documented below each function.  Production deployments SHOULD use a
/// Stellar multisig account (e.g. 3-of-5 signers) as the admin address.
///
/// # Multisig guidance for production
///
/// Stellar natively supports weighted multisig via `set_options`.  Recommended
/// setup:
///   - Create a dedicated admin account with master weight 0.
///   - Add 5 co-signer keys with weight 1 each; set thresholds to 3.
///   - The resulting address is the `admin` passed to `initialize`.
///   - All admin calls require 3-of-5 signatures in the transaction envelope.
///   - For higher assurance, use a hardware-wallet-backed signer set.
///
/// # Future timelock / governance seam
///
/// Each privileged setter is a single function call today.  To add a timelock:
///   1. Replace the direct write with a `Proposal { action, value, eta }` stored
///      at `DataKey::Proposal(action_id)`.
///   2. Add `execute_proposal(env, action_id)` that checks `env.ledger().timestamp()
///      >= eta` before applying.
///   3. The event schema below is already action-typed, so the NestJS
///      `admin_audit_log` ingestion requires no changes.
///
/// # Event schema (machine-readable for NestJS ingestion)
///
/// Every mutation emits:
///   topic:   ("admin", "<action_name>")
///   payload: depends on action — see individual functions below.
///
/// The NestJS handler can key on `topic[1]` (the action symbol) to route to
/// the correct `admin_audit_log` column without per-action parsers.
use soroban_sdk::{contracttype, panic_with_error, symbol_short, Address, Env};

use crate::storage;

// ── Error codes ───────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u32)]
pub enum AdminError {
    /// Caller is not the current admin.
    Unauthorized = 100,
    /// initialize() has already been called.
    AlreadyInitialized = 101,
    /// No pending admin proposal exists.
    NoPendingAdmin = 102,
    /// Caller is not the pending admin.
    NotPendingAdmin = 103,
    /// Supplied address is the zero/invalid sentinel.
    InvalidAddress = 104,
    /// Drain amount must be > 0.
    InvalidDrainAmount = 105,
}

// ── Auth helper ───────────────────────────────────────────────────────────────

/// Loads the admin, calls `require_auth()`, and returns the address.
/// Panics with `AdminError::Unauthorized` if storage has no admin yet
/// (should never happen after initialize, but guards against mis-ordering).
pub fn require_admin(env: &Env) -> Address {
    let admin = env
        .storage()
        .instance()
        .get::<_, Address>(&storage::DataKey::Admin)
        .unwrap_or_else(|| panic_with_error!(env, AdminError::Unauthorized));
    admin.require_auth();
    admin
}

// ── Admin rotation (two-step handoff) ────────────────────────────────────────
//
// Two-step pattern chosen over immediate replacement because:
//   - Immediate replacement risks locking out the protocol if the new address
//     is a typo or an uncontrolled key.
//   - Two-step requires the incoming admin to prove key control before the
//     handoff completes, eliminating that class of operational error.
//
// Flow:
//   1. current admin calls propose_admin(new_admin)
//   2. new_admin calls accept_admin()          → rotation complete
//   OR current admin calls cancel_admin()      → proposal withdrawn
//
// Future timelock seam: step 1 could store an `eta` and step 2 could check it.

/// Propose a new admin address.  The current admin must authorize.
/// Emits: ("admin", "proposed") → (old_admin, new_admin)
pub fn propose_admin(env: &Env, new_admin: Address) {
    let current = require_admin(env);

    // Reject zero-address / self-proposal is allowed (idempotent re-proposal)
    // but the address must be a valid Soroban Address (type system guarantees this).

    storage::set_pending_admin(env, &new_admin);

    env.events().publish(
        (symbol_short!("admin"), symbol_short!("proposed")),
        (current, new_admin),
    );
}

/// Accept a pending admin proposal.  The *new* (pending) admin must authorize.
/// Emits: ("admin", "accepted") → (old_admin, new_admin)
pub fn accept_admin(env: &Env) {
    let pending = storage::get_pending_admin(env)
        .unwrap_or_else(|| panic_with_error!(env, AdminError::NoPendingAdmin));

    // The pending admin must sign — prevents hijack by unrelated signers
    pending.require_auth();

    let old_admin = storage::get_admin(env);
    storage::set_admin(env, &pending);
    storage::clear_pending_admin(env);

    env.events().publish(
        (symbol_short!("admin"), symbol_short!("accepted")),
        (old_admin, pending),
    );
}

/// Cancel a pending admin proposal.  Only the current admin may cancel.
/// Emits: ("admin", "cancelled") → (current_admin, cancelled_pending)
pub fn cancel_admin(env: &Env) {
    let current = require_admin(env);
    let pending = storage::get_pending_admin(env)
        .unwrap_or_else(|| panic_with_error!(env, AdminError::NoPendingAdmin));

    storage::clear_pending_admin(env);

    env.events().publish(
        (symbol_short!("admin"), symbol_short!("cancelled")),
        (current, pending),
    );
}

// ── Token update ──────────────────────────────────────────────────────────────
//
// Future governance seam: replace with a proposal + timelock so token
// migrations are visible on-chain before they take effect.

/// Update the treasury token contract address.
/// Emits: ("admin", "token_set") → (old_token, new_token)
pub fn set_token(env: &Env, new_token: Address) {
    let _admin = require_admin(env);

    let old_token = storage::get_token(env);
    storage::set_token(env, &new_token);

    env.events().publish(
        (symbol_short!("admin"), symbol_short!("token")),
        (old_token, new_token),
    );
}

// ── Pause toggle ──────────────────────────────────────────────────────────────
//
// Pause blocks file_claim and vote_on_claim (see claim.rs).
// It does NOT retroactively invalidate in-flight votes or tallies.
// Future seam: add a community-vote-triggered unpause path.

/// Pause the contract.  Admin must authorize.
/// Emits: ("admin", "paused") → (admin)
pub fn pause(env: &Env) {
    let admin = require_admin(env);
    storage::set_paused(env, true);
    env.events()
        .publish((symbol_short!("admin"), symbol_short!("paused")), (admin,));
}

/// Unpause the contract.  Admin must authorize.
/// Emits: ("admin", "unpaused") → (admin)
pub fn unpause(env: &Env) {
    let admin = require_admin(env);
    storage::set_paused(env, false);
    env.events().publish(
        (symbol_short!("admin"), symbol_short!("unpaused")),
        (admin,),
    );
}

// ── Treasury drain ────────────────────────────────────────────────────────────
//
// Emergency fund recovery.  Transfers `amount` stroops of the treasury token
// from the contract to `recipient`.  Admin must authorize.
//
// Future governance seam: require a time-delayed proposal before drain executes,
// giving policyholders a window to exit if they disagree with the action.

/// Drain `amount` stroops from the contract treasury to `recipient`.
/// Emits: ("admin", "drained") → (admin, recipient, amount)
pub fn drain(env: &Env, recipient: Address, amount: i128) {
    let admin = require_admin(env);

    if amount <= 0 {
        panic_with_error!(env, AdminError::InvalidDrainAmount);
    }

    let token = storage::get_token(env);
    crate::token::transfer(env, &token, &env.current_contract_address(), &recipient, amount);

    env.events().publish(
        (symbol_short!("admin"), symbol_short!("drained")),
        (admin, recipient, amount),
    );
}
