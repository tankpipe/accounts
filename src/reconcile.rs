use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::account::{Transaction};


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
}

/// Wrapper for an existing transaction during reconciliation.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TargetResult {
    pub transaction: Transaction,
    pub status: ReconciliationMatchStatus,
    pub matched_reconciliation_id: Option<Uuid>,
}
