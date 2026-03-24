use soroban_sdk::{contracttype, Address, Env, Vec};

#[contracttype]
pub enum DataKey {
    Admin,
    Token,
    /// (holder, policy_id) — policy_id is per-holder u32
    Policy(Address, u32),
    /// Per-holder policy counter; next policy_id = counter + 1
    PolicyCounter(Address),
    Claim(u64),
    /// (claim_id, voter_address) → VoteOption; immutable after first write
    Vote(u64, Address),
    /// Vec<Address> of all current active policyholders (live voter set)
    Voters,
    /// Vec<Address> snapshot of eligible voters captured at claim-filing time.
    /// Keyed per claim so each claim has an independent, immutable electorate.
    ClaimVoters(u64),
    /// Global monotonic claim id counter
    ClaimCounter,
    /// Pause flag: if present and true the contract is paused
    Paused,
}

pub fn set_admin(env: &Env, admin: &Address) {
    env.storage().instance().set(&DataKey::Admin, admin);
}

/// Used by initialize and admin drain (feat/admin).
#[allow(dead_code)]
pub fn get_admin(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Admin).unwrap()
}

pub fn set_token(env: &Env, token: &Address) {
    env.storage().instance().set(&DataKey::Token, token);
}

/// Used by claim payout (feat/claim-voting).
#[allow(dead_code)]
pub fn get_token(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Token).unwrap()
}

/// Returns the next policy_id for `holder` and increments the counter.
/// Used by feat/policy-lifecycle.
#[allow(dead_code)]
pub fn next_policy_id(env: &Env, holder: &Address) -> u32 {
    let key = DataKey::PolicyCounter(holder.clone());
    let next: u32 = env.storage().persistent().get(&key).unwrap_or(0) + 1;
    env.storage().persistent().set(&key, &next);
    next
}

/// Returns the next global claim_id and increments the counter.
pub fn next_claim_id(env: &Env) -> u64 {
    let next: u64 = env
        .storage()
        .instance()
        .get(&DataKey::ClaimCounter)
        .unwrap_or(0u64)
        + 1;
    env.storage().instance().set(&DataKey::ClaimCounter, &next);
    next
}

/// Returns the live voter set (all current active policyholders).
pub fn get_voters(env: &Env) -> Vec<Address> {
    env.storage()
        .instance()
        .get(&DataKey::Voters)
        .unwrap_or_else(|| Vec::new(env))
}

/// Overwrites the live voter set.
pub fn set_voters(env: &Env, voters: &Vec<Address>) {
    env.storage().instance().set(&DataKey::Voters, voters);
}

/// Adds `holder` to the live voter set if not already present.
pub fn add_voter(env: &Env, holder: &Address) {
    let mut voters = get_voters(env);
    for v in voters.iter() {
        if v == *holder {
            return;
        }
    }
    voters.push_back(holder.clone());
    set_voters(env, &voters);
}

/// Removes `holder` from the live voter set.
pub fn remove_voter(env: &Env, holder: &Address) {
    let voters = get_voters(env);
    let mut updated: Vec<Address> = Vec::new(env);
    for v in voters.iter() {
        if v != *holder {
            updated.push_back(v);
        }
    }
    set_voters(env, &updated);
}

/// Snapshots the current voter set for a specific claim at filing time.
/// This is the authoritative electorate for that claim; immutable after set.
pub fn snapshot_voters_for_claim(env: &Env, claim_id: u64) {
    let voters = get_voters(env);
    env.storage()
        .persistent()
        .set(&DataKey::ClaimVoters(claim_id), &voters);
}

/// Returns the voter snapshot for a claim.
pub fn get_claim_voters(env: &Env, claim_id: u64) -> Vec<Address> {
    env.storage()
        .persistent()
        .get(&DataKey::ClaimVoters(claim_id))
        .unwrap_or_else(|| Vec::new(env))
}

/// Returns true if `voter` is in the snapshot for `claim_id`.
pub fn is_eligible_voter(env: &Env, claim_id: u64, voter: &Address) -> bool {
    let snapshot = get_claim_voters(env, claim_id);
    for v in snapshot.iter() {
        if v == *voter {
            return true;
        }
    }
    false
}

/// Returns true if the contract is paused.
pub fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
}
