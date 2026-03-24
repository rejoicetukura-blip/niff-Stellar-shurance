#![no_std]

mod claim;
mod policy;
#[allow(dead_code)] // used by policy.rs once feat/policy-lifecycle lands
mod premium;
mod storage;
mod token;
pub mod types;
pub mod validate;

use soroban_sdk::{contract, contractimpl, Address, Env, String, Vec};

use crate::types::VoteOption;

#[contract]
pub struct NiffyInsure;

#[contractimpl]
impl NiffyInsure {
    /// One-time initialisation: store admin and token contract address.
    /// Must be called immediately after deployment.
    pub fn initialize(env: Env, admin: Address, token: Address) {
        storage::set_admin(&env, &admin);
        storage::set_token(&env, &token);
    }

    // ── Policy domain ────────────────────────────────────────────────────
    // generate_premium, initiate_policy, renew_policy, terminate_policy
    // implemented in policy.rs — issue: feat/policy-lifecycle

    // ── Claim domain ─────────────────────────────────────────────────────

    /// File a new claim against an active policy.
    /// `claimant` must authorize; must match the policy holder address.
    pub fn file_claim(
        env: Env,
        claimant: Address,
        policy_id: u32,
        amount: i128,
        details: String,
        image_urls: Vec<String>,
    ) -> u64 {
        claim::file_claim(&env, claimant, policy_id, amount, details, image_urls)
    }

    /// Cast an immutable ballot on an open claim.
    /// `voter` must authorize; must be in the claim's voter snapshot.
    pub fn vote_on_claim(env: Env, voter: Address, claim_id: u64, vote: VoteOption) {
        claim::vote_on_claim(&env, voter, claim_id, vote)
    }

    /// Settle a claim after the voting deadline without a majority.
    /// Permissionless — anyone may call once `vote_deadline` has passed.
    pub fn finalize_claim(env: Env, claim_id: u64) {
        claim::finalize_claim(&env, claim_id)
    }

    // ── Admin / treasury ─────────────────────────────────────────────────
    // drain
    // implemented in token.rs — issue: feat/admin

    // ── Test-only helpers ─────────────────────────────────────────────────
    // These are NOT part of the production ABI; they exist solely to let
    // integration tests seed state without the full policy-lifecycle feature.
    // Gated behind the `testutils` feature so they are excluded from WASM builds.

    /// Seed a policy record and register the holder as a voter.
    #[cfg(feature = "testutils")]
    pub fn test_seed_policy(
        env: Env,
        holder: Address,
        policy_id: u32,
        coverage: i128,
        end_ledger: u32,
    ) {
        use crate::types::{Policy, PolicyType, RegionTier};
        let policy = Policy {
            holder: holder.clone(),
            policy_id,
            policy_type: PolicyType::Auto,
            region: RegionTier::Medium,
            premium: 10_000_000,
            coverage,
            is_active: true,
            start_ledger: 1,
            end_ledger,
        };
        env.storage()
            .persistent()
            .set(&storage::DataKey::Policy(holder.clone(), policy_id), &policy);
        storage::add_voter(&env, &holder);
    }

    /// Remove a holder from the live voter set (simulates policy termination).
    #[cfg(feature = "testutils")]
    pub fn test_remove_voter(env: Env, holder: Address) {
        storage::remove_voter(&env, &holder);
    }
}

// Re-export error type so tests can reference it without the module path.
pub use claim::ContractError;
