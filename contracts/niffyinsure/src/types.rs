use soroban_sdk::{contracttype, Address, String, Vec};

// ── Field size limits (enforced in mutating entrypoints) ─────────────────────
//
// These constants are the single source of truth referenced by both the
// contract entrypoints and the NestJS DTO validators / Next.js form limits.
//
// Storage griefing analysis:
//   DETAILS_MAX_LEN  = 256 bytes  → ~1 ledger entry, negligible rent
//   IMAGE_URL_MAX_LEN = 128 bytes → IPFS CIDv1 base32 ≤ 62 chars; URL wrapper ≤ 128
//   IMAGE_URLS_MAX   = 5          → caps Vec<String> at 5 × 128 = 640 bytes per claim
//   REASON_MAX_LEN   = 128 bytes  → termination reason string

pub const DETAILS_MAX_LEN: u32 = 256;
pub const IMAGE_URL_MAX_LEN: u32 = 128;
pub const IMAGE_URLS_MAX: u32 = 5;
pub const REASON_MAX_LEN: u32 = 128;

/// Voting window in ledgers (~7 days at 5 s/ledger ≈ 120_960 ledgers).
/// After this many ledgers from claim filing the vote is closed and
/// finalize_claim may be called to settle the outcome.
pub const VOTE_WINDOW_LEDGERS: u32 = 120_960;

// ── policy_id assignment ─────────────────────────────────────────────────────
//
// policy_id is a u32 scoped per holder: the contract increments a per-holder
// counter stored at DataKey::PolicyCounter(holder).  This means two holders
// can each have policy_id = 1 without collision; the canonical key is always
// (holder, policy_id).  A single holder may hold multiple active policies
// simultaneously; each active policy grants exactly one vote in claim
// governance (one-policy-one-vote, not one-holder-one-vote).

// ── Enums ────────────────────────────────────────────────────────────────────

/// Coverage category.  Categorical enum prevents unbounded string storage and
/// aligns with backend DTO `PolicyType` discriminated union.
#[contracttype]
#[derive(Clone, PartialEq, Debug)]
pub enum PolicyType {
    Auto,
    Health,
    Property,
}

/// Geographic risk tier.  Replaces a free-form region string; maps 1-to-1 with
/// the premium multiplier table in `premium.rs`.
#[contracttype]
#[derive(Clone, PartialEq, Debug)]
pub enum RegionTier {
    Low,    // rural / low-risk zone
    Medium, // suburban
    High,   // urban / high-risk zone
}

/// Claim lifecycle state machine.
///
/// ```text
/// [filed] → Processing
///               │
///        ┌──────┴──────┐
///        ▼             ▼
///    Approved       Rejected
/// ```
///
/// Transitions:
///   Processing → Approved  : majority Approve votes reached
///   Processing → Rejected  : majority Reject votes reached OR policy deactivated
///
/// Terminal states (Approved / Rejected) are immutable; no re-open path exists
/// on-chain.  Off-chain dispute resolution must open a new claim.
#[contracttype]
#[derive(Clone, PartialEq, Debug)]
pub enum ClaimStatus {
    Processing,
    Approved,
    Rejected,
}

impl ClaimStatus {
    /// Returns true only for the two terminal states.
    pub fn is_terminal(&self) -> bool {
        matches!(self, ClaimStatus::Approved | ClaimStatus::Rejected)
    }
}

/// Ballot option cast by a policyholder during claim voting.
#[contracttype]
#[derive(Clone, PartialEq, Debug)]
pub enum VoteOption {
    Approve,
    Reject,
}

// ── Core structs ─────────────────────────────────────────────────────────────

/// On-chain policy record.
///
/// | Field          | Authoritative | Notes |
/// |----------------|---------------|-------|
/// | holder         | on-chain      | Soroban Address; used as storage key component |
/// | policy_id      | on-chain      | per-holder u32 counter; see note above |
/// | policy_type    | on-chain      | categorical enum |
/// | region         | on-chain      | risk tier enum |
/// | premium        | on-chain      | stroops; computed by premium.rs at bind time |
/// | coverage       | on-chain      | stroops; max payout for this policy |
/// | is_active      | on-chain      | false after termination or expiry |
/// | start_ledger   | on-chain      | ledger sequence at activation |
/// | end_ledger     | on-chain      | ledger sequence at expiry; must be > start_ledger |
#[contracttype]
#[derive(Clone)]
pub struct Policy {
    /// Policyholder address; component of the storage key.
    pub holder: Address,
    /// Per-holder monotonic identifier (starts at 1).
    pub policy_id: u32,
    pub policy_type: PolicyType,
    pub region: RegionTier,
    /// Annual premium in stroops paid at activation / renewal.
    pub premium: i128,
    /// Maximum claim payout in stroops; must be > 0.
    pub coverage: i128,
    pub is_active: bool,
    /// Ledger sequence when the policy became active.
    pub start_ledger: u32,
    /// Ledger sequence when the policy expires; end_ledger > start_ledger.
    pub end_ledger: u32,
}

/// On-chain claim record.
///
/// ## Snapshot vs live-voter fairness decision
///
/// **Design choice: snapshot at claim-filing time.**
///
/// When `file_claim` is called the contract captures the current `Voters`
/// Vec<Address> into `DataKey::ClaimVoters(claim_id)`.  Only addresses present
/// in that snapshot may cast a ballot on this claim.
///
/// Rationale:
/// - A holder who terminates their policy *after* a claim is filed but *before*
///   voting closes retains their vote right; they were a member when the claim
///   arose.  This prevents a griefing vector where an adversary joins, files a
///   claim, then mass-terminates policies to shrink the quorum denominator.
/// - New policyholders who join after filing cannot influence an existing claim;
///   they had no stake when the loss event occurred.
/// - Fairness trade-off: a holder who terminates mid-vote keeps their vote
///   weight.  UI copy must reflect this ("Your vote stands even if your policy
///   lapses before the deadline").
///
/// ## Vote mutability
///
/// Votes are **immutable after first cast**.  Once `DataKey::Vote(claim_id,
/// voter)` is written it cannot be overwritten.  This prevents last-minute
/// flip attacks and simplifies tally reconciliation.
///
/// ## Tally maintenance
///
/// `approve_votes` and `reject_votes` are incremented atomically inside
/// `vote_on_claim` immediately after the per-voter record is written.  Because
/// Soroban contracts are single-threaded there is no race condition; the
/// running counters always equal the number of `Vote(claim_id, *)` entries for
/// each option.  Finalization reads only the two counters — O(1), no scan.
///
/// | Field          | Authoritative | Notes |
/// |----------------|---------------|-------|
/// | claim_id       | on-chain      | global monotonic u64 from ClaimCounter |
/// | policy_id      | on-chain      | references Policy(holder, policy_id) |
/// | claimant       | on-chain      | must equal policy.holder |
/// | amount         | on-chain      | stroops; 0 < amount ≤ policy.coverage |
/// | details        | on-chain      | ≤ DETAILS_MAX_LEN bytes |
/// | image_urls     | on-chain      | ≤ IMAGE_URLS_MAX items, each ≤ IMAGE_URL_MAX_LEN |
/// | status         | on-chain      | ClaimStatus state machine |
/// | approve_votes  | on-chain      | running tally; reconciles with Vote entries |
/// | reject_votes   | on-chain      | running tally; reconciles with Vote entries |
/// | vote_deadline  | on-chain      | ledger seq after which no new votes accepted |
/// | snapshot_size  | on-chain      | number of eligible voters at filing time |
#[contracttype]
#[derive(Clone)]
pub struct Claim {
    pub claim_id: u64,
    pub policy_id: u32,
    pub claimant: Address,
    /// Requested payout in stroops.
    pub amount: i128,
    /// Human-readable description; max DETAILS_MAX_LEN bytes.
    pub details: String,
    /// IPFS URLs for supporting images; max IMAGE_URLS_MAX items.
    pub image_urls: Vec<String>,
    pub status: ClaimStatus,
    pub approve_votes: u32,
    pub reject_votes: u32,
    /// Ledger sequence after which `vote_on_claim` is rejected.
    /// Set to `env.ledger().sequence() + VOTE_WINDOW_LEDGERS` at filing.
    pub vote_deadline: u32,
    /// Number of addresses in the voter snapshot taken at filing.
    /// Used by the frontend to display quorum progress.
    pub snapshot_size: u32,
}
