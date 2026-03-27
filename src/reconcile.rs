use std::collections::{HashMap, HashSet};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::account::{Side, Transaction};


/// Result of matching a transaction during reconciliation.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum ReconciliationMatchStatus {
    Matched,
    PartialMatch,
    Mismatch,
    Unmatched,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum ReconciliationItem {
    Reconciliation(ReconciliationResult),
    Original(TargetResult),
}

impl ReconciliationItem {
    pub fn status(&self) -> ReconciliationMatchStatus {
        match self {
            ReconciliationItem::Reconciliation(r) => r.status.clone(),
            ReconciliationItem::Original(o) => o.status.clone(),
        }
    }

    pub fn set_status(&mut self, status: ReconciliationMatchStatus) {
        match self {
            ReconciliationItem::Reconciliation(r) => r.status = status,
            ReconciliationItem::Original(o) => o.status = status,
        }
    }
}

/// Wrapper for a single reconciliation transaction during reconciliation.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ReconciliationResult {
    pub transaction: Transaction,
    pub status: ReconciliationMatchStatus,
    pub matched_transaction_id: Option<Uuid>,
    pub balance: Option<Decimal>,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub signals: Vec<String>,
}

/// Wrapper for an existing transaction during reconciliation.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TargetResult {
    pub transaction: Transaction,
    pub status: ReconciliationMatchStatus,
    pub matched_reconciliation_id: Option<Uuid>,
}

pub fn score_reconciliation_items(account_id: Uuid, items: &mut [ReconciliationItem]) {
    let mut target_lookup: HashMap<Uuid, (chrono::NaiveDate, String, Decimal, Side, Option<Decimal>)> = HashMap::new();

    for item in items.iter() {
        if let ReconciliationItem::Original(original) = item {
            if let Some(entry) = original.transaction.find_entry_by_account(&account_id) {
                target_lookup.insert(
                    original.transaction.id,
                    (entry.date, entry.description.clone(), entry.amount, entry.entry_type, entry.balance),
                );
            }
        }
    }

    for item in items.iter_mut() {
        if let ReconciliationItem::Reconciliation(recon) = item {
            let (confidence, signals) = score_item(account_id, recon, &target_lookup);
            recon.confidence = confidence;
            recon.signals = signals;
        }
    }
}

fn score_item(
    account_id: Uuid,
    reconciliation: &ReconciliationResult,
    target_lookup: &HashMap<Uuid, (chrono::NaiveDate, String, Decimal, Side, Option<Decimal>)>,
) -> (f32, Vec<String>) {
    let mut score = match reconciliation.status {
        ReconciliationMatchStatus::Matched => 0.78,
        ReconciliationMatchStatus::PartialMatch => 0.55,
        ReconciliationMatchStatus::Mismatch => 0.32,
        ReconciliationMatchStatus::Unmatched => 0.08,
    };
    let mut signals: Vec<String> = Vec::new();

    let Some(target_id) = reconciliation.matched_transaction_id else {
        signals.push(rust_i18n::t!("reconciliation.signals.no_linked_target_transaction").to_string());
        let confidence = clamp_confidence(score);
        return (confidence, signals);
    };

    let Some((target_date, target_description, target_amount, target_side, target_balance)) = target_lookup.get(&target_id) else {
        signals.push(rust_i18n::t!("reconciliation.signals.missing_linked_target_transaction").to_string());
        let confidence = clamp_confidence(score);
        return (confidence, signals);
    };

    let Some(rec_entry) = reconciliation.transaction.find_entry_by_account(&account_id) else {
        signals.push(rust_i18n::t!("reconciliation.signals.missing_account_entry").to_string());
        let confidence = clamp_confidence(score - 0.1);
        return (confidence, signals);
    };

    let date_diff = (rec_entry.date - *target_date).num_days().abs();
    let amount_match = rec_entry.amount == *target_amount;
    let side_match = rec_entry.entry_type == *target_side;
    let desc_similarity = description_similarity(&rec_entry.description, target_description);
    let balance_match = rec_entry.balance.is_some() && rec_entry.balance == *target_balance;

    if amount_match {
        score += 0.12;
        signals.push(rust_i18n::t!("reconciliation.signals.amount_exact_match").to_string());
    } else {
        score -= 0.15;
        signals.push(rust_i18n::t!("reconciliation.signals.amount_differs").to_string());
    }

    if side_match {
        score += 0.08;
        signals.push(rust_i18n::t!("reconciliation.signals.side_matches").to_string());
    } else {
        score -= 0.1;
        signals.push(rust_i18n::t!("reconciliation.signals.side_differs").to_string());
    }

    if date_diff == 0 {
        score += 0.08;
        signals.push(rust_i18n::t!("reconciliation.signals.date_exact_match").to_string());
    } else if date_diff <= 1 {
        score += 0.05;
        signals.push(rust_i18n::t!("reconciliation.signals.date_within_1_day").to_string());
    } else if date_diff <= 3 {
        score += 0.02;
        signals.push(rust_i18n::t!("reconciliation.signals.date_within_3_days").to_string());
    } else if date_diff > 7 {
        score -= 0.05;
        signals.push(rust_i18n::t!("reconciliation.signals.date_differs_by_days", days => date_diff).to_string());
    }

    if desc_similarity > 0.8 {
        score += 0.09;
        signals.push(rust_i18n::t!("reconciliation.signals.description_high_similarity").to_string());
    } else if desc_similarity > 0.5 {
        score += 0.04;
        signals.push(rust_i18n::t!("reconciliation.signals.description_moderate_similarity").to_string());
    } else if desc_similarity < 0.25 {
        score -= 0.05;
        signals.push(rust_i18n::t!("reconciliation.signals.description_weak_similarity").to_string());
    }

    if rec_entry.balance.is_some() && target_balance.is_some() {
        if balance_match {
            score += 0.1;
            signals.push(rust_i18n::t!("reconciliation.signals.balance_aligns").to_string());
        } else {
            score -= 0.08;
            signals.push(rust_i18n::t!("reconciliation.signals.balance_differs").to_string());
        }
    }

    (clamp_confidence(score), signals)
}

fn description_similarity(a: &str, b: &str) -> f32 {
    let a_tokens = tokenize_description(a);
    let b_tokens = tokenize_description(b);
    if a_tokens.is_empty() && b_tokens.is_empty() {
        return 1.0;
    }
    if a_tokens.is_empty() || b_tokens.is_empty() {
        return 0.0;
    }

    let intersection = a_tokens.intersection(&b_tokens).count() as f32;
    let union = a_tokens.union(&b_tokens).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        intersection / union
    }
}

fn tokenize_description(description: &str) -> HashSet<String> {
    description
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| token.to_string())
        .collect()
}

fn clamp_confidence(value: f32) -> f32 {
    value.max(0.0).min(0.99)
}
