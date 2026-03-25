use crate::{
    premium,
    types::{PolicyType, PremiumQuote, RegionTier},
};
use soroban_sdk::{contracterror, contracttype, Env, String};

/// How long a quote stays valid (in ledgers) from generation time.
pub const QUOTE_TTL_LEDGERS: u32 = 100;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum QuoteError {
    InvalidAge = 1,
    InvalidRiskScore = 2,
    InvalidQuoteTtl = 3,
    ArithmeticOverflow = 4,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuoteFailure {
    pub code: u32,
    pub message: String,
}

pub fn generate_premium(
    env: &Env,
    policy_type: PolicyType,
    region: RegionTier,
    age: u32,
    risk_score: u32,
    include_breakdown: bool,
) -> Result<PremiumQuote, QuoteError> {
    if age == 0 || age > 120 {
        return Err(QuoteError::InvalidAge);
    }
    if risk_score == 0 || risk_score > 10 {
        return Err(QuoteError::InvalidRiskScore);
    }
    // QUOTE_TTL_LEDGERS is a compile-time constant > 0; no runtime check needed.

    let total = premium::compute_premium_checked(&policy_type, &region, age, risk_score)
        .ok_or(QuoteError::ArithmeticOverflow)?;

    let line_items = if include_breakdown {
        Some(
            premium::build_line_items(env, &policy_type, &region, age, risk_score)
                .ok_or(QuoteError::ArithmeticOverflow)?,
        )
    } else {
        None
    };

    let current_ledger = env.ledger().sequence();
    let valid_until_ledger = current_ledger
        .checked_add(QUOTE_TTL_LEDGERS)
        .ok_or(QuoteError::ArithmeticOverflow)?;

    Ok(PremiumQuote {
        total_premium: total,
        line_items,
        valid_until_ledger,
    })
}

pub fn map_quote_error(env: &Env, err: QuoteError) -> QuoteFailure {
    let message = match err {
        QuoteError::InvalidAge => "invalid age: expected 1..=120",
        QuoteError::InvalidRiskScore => "invalid risk_score: expected 1..=10",
        QuoteError::InvalidQuoteTtl => "quote ttl misconfigured: contact support",
        QuoteError::ArithmeticOverflow => "pricing arithmetic overflow: contact support",
    };
    QuoteFailure {
        code: err as u32,
        message: String::from_str(env, message),
    }
}
