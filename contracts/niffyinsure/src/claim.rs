/// Claim lifecycle and DAO voting.
///
/// # Voter eligibility: snapshot model
///
/// At `file_claim` time the contract captures the live `Voters` Vec into
/// `DataKey::ClaimVoters(claim_id)`.  Only addresses in that snapshot may vote
/// on the claim.  This means:
///
/// - A holder who terminates their policy *after* filing retains their vote
///   right — they were a member when the loss event occurred.
/// - A holder who joins *after* filing cannot vote — they had no stake at the
///   time of the event.
/// - **UI copy alignment**: "Your vote stands even if your policy lapses before
///   the voting deadline."
///
/// # Vote immutability
///
/// Votes are immutable after first cast (`DataKey::Vote(claim_id, voter)` is
/// written once and never overwritten).  This prevents last-minute flip attacks
/// and keeps tally reconciliation trivial.
///
/// # Tally consistency
///
/// `approve_votes` / `reject_votes` on the `Claim` struct are incremented
/// immediately after the per-voter record is written.  Soroban contracts are
/// single-threaded so there is no race; the counters always equal the count of
/// `Vote(claim_id, *)` entries for each option.  Finalization is O(1).
///
/// # Pause interaction
///
/// Both `file_claim` and `vote_on_claim` panic with `ContractError::Paused`
/// when the contract is paused.  Existing votes and tallies are unaffected.
use soroban_sdk::{contracttype, panic_with_error, symbol_short, Address, Env, String, Vec};

use crate::{
    storage::{
        self, get_claim_voters, is_eligible_voter, next_claim_id, snapshot_voters_for_claim,
    },
    types::{Claim, ClaimStatus, VoteOption, VOTE_WINDOW_LEDGERS},
    validate::{check_claim_fields, check_claim_open},
};

// ── Contract-level error codes ────────────────────────────────────────────────

#[contracttype]
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u32)]
pub enum ContractError {
    /// Contract is administratively paused.
    Paused = 1,
    /// Caller is not the policy holder.
    NotPolicyHolder = 2,
    /// Policy does not exist or is not active.
    PolicyNotActive = 3,
    /// Claim amount is zero or exceeds coverage.
    InvalidClaimAmount = 4,
    /// Claim details or URL fields violate size limits.
    InvalidClaimFields = 5,
    /// Claim does not exist.
    ClaimNotFound = 6,
    /// Claim is already in a terminal state.
    ClaimTerminal = 7,
    /// Voting window has closed (current ledger > vote_deadline).
    VotingClosed = 8,
    /// Caller is not in the voter snapshot for this claim.
    NotEligibleVoter = 9,
    /// Voter has already cast a ballot on this claim (immutable).
    AlreadyVoted = 10,
    /// Claim is still within the voting window; cannot finalize yet.
    VotingStillOpen = 11,
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn load_claim(env: &Env, claim_id: u64) -> Claim {
    env.storage()
        .persistent()
        .get(&storage::DataKey::Claim(claim_id))
        .unwrap_or_else(|| panic_with_error!(env, ContractError::ClaimNotFound))
}

fn save_claim(env: &Env, claim: &Claim) {
    env.storage()
        .persistent()
        .set(&storage::DataKey::Claim(claim.claim_id), claim);
}

/// Simple majority: strictly more than half of snapshot_size.
/// If snapshot_size is 0 (no voters at filing) the claim cannot reach majority
/// and will be rejected at finalization.
fn majority(snapshot_size: u32) -> u32 {
    snapshot_size / 2 + 1
}

// ── Public entrypoints ────────────────────────────────────────────────────────

/// File a new insurance claim against an active policy.
///
/// # Authentication
/// `claimant` must authorize this call (`claimant.require_auth()`).  The
/// address must match `policy.holder`; mismatched addresses are rejected before
/// any storage write.
///
/// # Events emitted
/// `ClaimFiled { claim_id, policy_id, claimant, amount, vote_deadline, snapshot_size }`
pub fn file_claim(
    env: &Env,
    claimant: Address,
    policy_id: u32,
    amount: i128,
    details: String,
    image_urls: Vec<String>,
) -> u64 {
    // Pause guard
    if storage::is_paused(env) {
        panic_with_error!(env, ContractError::Paused);
    }

    // Authenticate the claimant — cannot be spoofed by a mismatched address
    claimant.require_auth();

    // Load and validate the policy
    let policy: crate::types::Policy = env
        .storage()
        .persistent()
        .get(&storage::DataKey::Policy(claimant.clone(), policy_id))
        .unwrap_or_else(|| panic_with_error!(env, ContractError::PolicyNotActive));

    if !policy.is_active || env.ledger().sequence() >= policy.end_ledger {
        panic_with_error!(env, ContractError::PolicyNotActive);
    }

    // Validate claim fields
    check_claim_fields(env, amount, policy.coverage, &details, &image_urls)
        .unwrap_or_else(|_| panic_with_error!(env, ContractError::InvalidClaimFields));

    // Assign claim id and snapshot voters
    let claim_id = next_claim_id(env);
    snapshot_voters_for_claim(env, claim_id);
    let snapshot = get_claim_voters(env, claim_id);
    let snapshot_size = snapshot.len();

    let vote_deadline = env.ledger().sequence() + VOTE_WINDOW_LEDGERS;

    let claim = Claim {
        claim_id,
        policy_id,
        claimant: claimant.clone(),
        amount,
        details,
        image_urls,
        status: ClaimStatus::Processing,
        approve_votes: 0,
        reject_votes: 0,
        vote_deadline,
        snapshot_size,
    };
    save_claim(env, &claim);

    // Emit ClaimFiled event — enough data for the Next.js claim detail page
    env.events().publish(
        (symbol_short!("claim"), symbol_short!("filed")),
        (claim_id, policy_id, claimant, amount, vote_deadline, snapshot_size),
    );

    claim_id
}

/// Cast a ballot on an open claim.
///
/// # Authentication
/// `voter` must authorize this call (`voter.require_auth()`).
///
/// # Eligibility
/// `voter` must appear in the snapshot taken at `file_claim` time.  Addresses
/// not in the snapshot are rejected immediately to prevent storage bloat /
/// griefing.
///
/// # Immutability
/// A voter may cast exactly one ballot.  Attempting to vote again panics with
/// `ContractError::AlreadyVoted`.
///
/// # Auto-finalization
/// After recording the vote the function checks whether a simple majority has
/// been reached.  If so, the claim is immediately transitioned to
/// `Approved` or `Rejected` and a `ClaimSettled` event is emitted.
///
/// # Events emitted
/// `VoteLogged { claim_id, voter, vote, approve_votes, reject_votes, snapshot_size }`
/// Optionally: `ClaimSettled { claim_id, status }` on majority reached.
pub fn vote_on_claim(env: &Env, voter: Address, claim_id: u64, vote: VoteOption) {
    // Pause guard
    if storage::is_paused(env) {
        panic_with_error!(env, ContractError::Paused);
    }

    // Authenticate — require_auth prevents address spoofing
    voter.require_auth();

    // Load claim and verify it is still open
    let mut claim = load_claim(env, claim_id);
    check_claim_open(&claim)
        .unwrap_or_else(|_| panic_with_error!(env, ContractError::ClaimTerminal));

    // Voting window check
    if env.ledger().sequence() > claim.vote_deadline {
        panic_with_error!(env, ContractError::VotingClosed);
    }

    // Eligibility: voter must be in the snapshot (early rejection prevents map spam)
    if !is_eligible_voter(env, claim_id, &voter) {
        panic_with_error!(env, ContractError::NotEligibleVoter);
    }

    // Duplicate vote check (immutable ballot)
    let vote_key = storage::DataKey::Vote(claim_id, voter.clone());
    if env.storage().persistent().has(&vote_key) {
        panic_with_error!(env, ContractError::AlreadyVoted);
    }

    // Record the ballot — written once, never overwritten
    env.storage().persistent().set(&vote_key, &vote);

    // Update running tallies transactionally
    match &vote {
        VoteOption::Approve => claim.approve_votes += 1,
        VoteOption::Reject => claim.reject_votes += 1,
    }

    // Emit VoteLogged — sufficient for the claim detail timeline in Next.js
    env.events().publish(
        (symbol_short!("vote"), symbol_short!("logged")),
        (
            claim_id,
            voter.clone(),
            vote,
            claim.approve_votes,
            claim.reject_votes,
            claim.snapshot_size,
        ),
    );

    // Auto-finalize on simple majority
    let threshold = majority(claim.snapshot_size);
    if claim.approve_votes >= threshold {
        claim.status = ClaimStatus::Approved;
        env.events().publish(
            (symbol_short!("claim"), symbol_short!("settled")),
            (claim_id, ClaimStatus::Approved),
        );
    } else if claim.reject_votes >= threshold {
        claim.status = ClaimStatus::Rejected;
        env.events().publish(
            (symbol_short!("claim"), symbol_short!("settled")),
            (claim_id, ClaimStatus::Rejected),
        );
    }

    save_claim(env, &claim);
}

/// Finalize a claim after the voting deadline has passed without a majority.
///
/// Compares tallies and sets status to Approved or Rejected based on plurality.
/// If tallies are equal the claim is Rejected (tie goes to insurer).
///
/// Can be called by anyone after `vote_deadline` — no auth required.
pub fn finalize_claim(env: &Env, claim_id: u64) {
    let mut claim = load_claim(env, claim_id);
    check_claim_open(&claim)
        .unwrap_or_else(|_| panic_with_error!(env, ContractError::ClaimTerminal));

    if env.ledger().sequence() <= claim.vote_deadline {
        panic_with_error!(env, ContractError::VotingStillOpen);
    }

    claim.status = if claim.approve_votes > claim.reject_votes {
        ClaimStatus::Approved
    } else {
        // Tie or majority reject → Rejected
        ClaimStatus::Rejected
    };

    env.events().publish(
        (symbol_short!("claim"), symbol_short!("settled")),
        (claim_id, claim.status.clone()),
    );

    save_claim(env, &claim);
}
