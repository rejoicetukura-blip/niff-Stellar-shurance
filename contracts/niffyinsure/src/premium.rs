use crate::types::{PolicyType, PremiumQuoteLineItem, RegionTier};
use soroban_sdk::{Env, String, Vec};

/// Base annual premium in stroops (1 XLM = 10_000_000 stroops).
const BASE: i128 = 10_000_000;

/// Returns the type risk factor.
/// Optimization: pure function, no allocations, inlined by compiler.
pub fn type_factor(policy_type: &PolicyType) -> i128 {
    match policy_type {
        PolicyType::Auto => 15,
        PolicyType::Health => 20,
        PolicyType::Property => 10,
    }
}

/// Returns the region risk factor.
pub fn region_factor(region: &RegionTier) -> i128 {
    match region {
        RegionTier::Low => 8,
        RegionTier::Medium => 10,
        RegionTier::High => 14,
    }
}

/// Returns the age risk factor.
pub fn age_factor(age: u32) -> i128 {
    if age < 25 {
        15
    } else if age > 60 {
        13
    } else {
        10
    }
}

/// Computes total premium with overflow protection.
/// Replaces the former unchecked `compute_premium` (which used `*` and `/` directly,
/// risking silent wrapping on adversarial inputs).
/// Write count: 0 (pure computation).
pub fn compute_premium_checked(
    policy_type: &PolicyType,
    region: &RegionTier,
    age: u32,
    risk_score: u32,
) -> Option<i128> {
    let raw = type_factor(policy_type)
        .checked_add(region_factor(region))?
        .checked_add(age_factor(age))?
        .checked_add(risk_score as i128)?;
    BASE.checked_mul(raw)?.checked_div(10)
}

/// Builds premium breakdown line items.
/// Optimization: factors computed once and reused for both `factor` and `amount` fields,
/// eliminating 4 redundant `type_factor`/`region_factor`/`age_factor` calls vs the
/// previous implementation that called each helper twice.
/// Write count: 0 (pure computation, no storage).
#[allow(dead_code)]
pub fn build_line_items(
    env: &Env,
    policy_type: &PolicyType,
    region: &RegionTier,
    age: u32,
    risk_score: u32,
) -> Option<Vec<PremiumQuoteLineItem>> {
    let tf = type_factor(policy_type);
    let rf = region_factor(region);
    let af = age_factor(age);
    let rsk = risk_score as i128;

    // Each amount = BASE * factor / 10 — computed once per factor.
    let amt_type = BASE.checked_mul(tf)?.checked_div(10)?;
    let amt_region = BASE.checked_mul(rf)?.checked_div(10)?;
    let amt_age = BASE.checked_mul(af)?.checked_div(10)?;
    let amt_risk = BASE.checked_mul(rsk)?.checked_div(10)?;

    let mut items = Vec::new(env);
    items.push_back(PremiumQuoteLineItem {
        component: String::from_str(env, "type"),
        factor: tf,
        amount: amt_type,
    });
    items.push_back(PremiumQuoteLineItem {
        component: String::from_str(env, "region"),
        factor: rf,
        amount: amt_region,
    });
    items.push_back(PremiumQuoteLineItem {
        component: String::from_str(env, "age"),
        factor: af,
        amount: amt_age,
    });
    items.push_back(PremiumQuoteLineItem {
        component: String::from_str(env, "risk_score"),
        factor: rsk,
        amount: amt_risk,
    });
    Some(items)
}
