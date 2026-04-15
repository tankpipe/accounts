use crate::books_error;
use chrono::NaiveDate;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::{cmp::Ordering, collections::HashMap};
use uuid::Uuid;

use crate::account::{
    Account, AccountType, Entry, ReconciledStatus, Source, Transaction, TransactionStatus,
};
use crate::interest::{calculate_interest_for_accounts, Interest};
use crate::reconcile::{
    Field, ReconciliationItem, ReconciliationMatchStatus, ReconciliationResult, Signal,
    TargetResult,
};
use crate::schedule::{Modifier, Schedule};
use crate::scheduler::Scheduler;
use crate::serializer::{deserialize_option_naivedate, serialize_option_naivedate};

const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_PROJECTION_MONTHS: u32 = 12;
pub const MAX_PROJECTION_MONTHS: u32 = 1200;

/// Configuration for this books instance.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct Settings {
    pub require_double_entry: bool,
    #[serde(default = "default_projection_months")]
    pub projection_months: u32,
    #[serde(default)]
    #[serde(serialize_with = "serialize_option_naivedate")]
    #[serde(deserialize_with = "deserialize_option_naivedate")]
    pub projected_to: Option<NaiveDate>,
}

/// Book of accounts a.k.a The Books.
#[derive(Serialize, Deserialize)]
pub struct Books {
    pub id: Uuid,
    pub name: String,
    pub version: String,
    accounts: HashMap<Uuid, Account>,
    scheduler: Scheduler,
    transactions: Vec<Transaction>,
    interests: HashMap<Uuid, Interest>,
    pub settings: Settings,

    #[serde(skip)]
    recalculate_interest: HashSet<Uuid>,
}

impl Books {
    pub fn generate(&mut self, end_date: NaiveDate) {
        let transactions = self.scheduler.generate(end_date);
        for transaction in transactions.iter() {
            let _ = self.add_transaction(transaction.clone());
        }
        sort_transactions_by_account(
            &mut self.transactions,
            None,
            TransactionSortOrder::OldestFirst,
        );
    }

    pub fn generate_by_schedule(
        &mut self,
        end_date: NaiveDate,
        schedule_id: Uuid,
    ) -> Vec<Transaction> {
        let transactions = self.scheduler.generate_by_schedule(end_date, schedule_id);
        for transaction in transactions.iter() {
            let _ = self.add_transaction(transaction.clone());
        }
        sort_transactions_by_account(
            &mut self.transactions,
            None,
            TransactionSortOrder::OldestFirst,
        );
        transactions
    }

    pub fn build_empty(name: &str) -> Books {
        Books {
            id: Uuid::new_v4(),
            name: name.to_string(),
            version: VERSION.to_string(),
            accounts: HashMap::new(),
            scheduler: Scheduler::build_empty(),
            transactions: Vec::new(),
            interests: HashMap::new(),
            settings: Settings {
                require_double_entry: false,
                projection_months: DEFAULT_PROJECTION_MONTHS,
                projected_to: None,
            },
            recalculate_interest: HashSet::new(),
        }
    }

    pub fn with_components(
        id: Uuid,
        name: String,
        version: String,
        accounts: HashMap<Uuid, Account>,
        scheduler: Scheduler,
        transactions: Vec<Transaction>,
        interests: HashMap<Uuid, Interest>,
        settings: Settings,
    ) -> Books {
        Books {
            id,
            name,
            version,
            accounts,
            scheduler,
            transactions,
            interests,
            settings,
            recalculate_interest: HashSet::new(),
        }
    }

    pub fn add_account(&mut self, account: Account) {
        let mut account = account;
        account.reconciliation_info = None;
        self.flag_interest_outdated_by_account(&account);
        if !self.accounts.contains_key(&account.id) {
            self.accounts.insert(account.id, account);
        }
    }

    pub fn update_account(&mut self, account: Account) -> Result<(), BooksError> {
        let existing = self
            .accounts
            .get(&account.id)
            .ok_or_else(|| books_error!("errors.account_not_found", id => account.id))?;

        let same_reconciliation =
            match (&account.reconciliation_info, &existing.reconciliation_info) {
                (None, None) => true,
                (Some(a), Some(b)) => {
                    a.date == b.date
                        && a.balance == b.balance
                        && a.transaction_id == b.transaction_id
                }
                _ => false,
            };

        if !same_reconciliation {
            return Err(books_error!("errors.account_reconciliation_info_immutable"));
        }

        if account.account_type != existing.account_type
            && self
                .transactions
                .iter()
                .any(|t| t.involves_account(&account.id))
        {
            return Err(books_error!(
                "errors.account_type_immutable_with_transactions"
            ));
        }

        if account.starting_balance != existing.starting_balance
            && existing.reconciliation_info.is_some()
        {
            return Err(books_error!(
                "errors.account_starting_balance_immutable_after_reconciliation"
            ));
        }

        self.flag_interest_outdated_by_account(&account);
        self.accounts.insert(account.id, account);
        Ok(())
    }

    pub fn delete_account(&mut self, id: &Uuid) -> Result<(), BooksError> {
        if !self.accounts.contains_key(id) {
            return Err(books_error!("errors.account_not_found", id => id));
        }

        if self.transactions.iter().any(|t| t.involves_account(id)) {
            return Err(books_error!("errors.account_cannot_delete_with_transactions", id => id));
        }

        if let Some(account) = self.accounts.remove(id) {
            self.flag_interest_outdated_by_account(&account);
        }

        Ok(())
    }

    pub fn get_account(&self, id: &Uuid) -> Result<Account, BooksError> {
        self.accounts
            .get(id)
            .cloned()
            .ok_or(books_error!("errors.account_not_found", id => id))
    }

    pub fn accounts(&self) -> Vec<Account> {
        let mut accounts_clone: Vec<Account> = Vec::new();
        for a in self.accounts.values() {
            accounts_clone.push(a.clone());
        }

        accounts_clone.sort_by(|a, b| {
            let result = a.account_type.order().cmp(&b.account_type.order());
            if result == Ordering::Equal {
                return a.name.cmp(&b.name);
            }
            return result;
        });
        accounts_clone
    }

    pub fn add_transaction(&mut self, transaction: Transaction) -> Result<(), BooksError> {
        self.validate_transaction(&transaction)?;
        self.flag_interest_outdated(&transaction);
        self.transactions.push(transaction);
        Ok(())
    }

    fn validate_transaction(&mut self, transaction: &Transaction) -> Result<(), BooksError> {
        for e in transaction.entries.as_slice() {
            self.valid_account_id(e.account_id)?;
        }

        if self.settings.require_double_entry && transaction.entries.len() < 2 {
            return Err(books_error!("errors.transaction_requires_two_entries"));
        } else if transaction.entries.len() < 1 {
            return Err(books_error!("errors.transaction_requires_one_entry"));
        }

        // Using transaction date to avoid potential reconciliation edge case for split date entries.
        if transaction.status == TransactionStatus::Recorded
            && transaction.date() > Some(chrono::Utc::now().date_naive())
        {
            return Err(books_error!("errors.future_transaction_set_as_recorded"));
        }

        self.valid_account_id(transaction.entries[0].account_id)?;

        // Check that no account has more than one entry in the transaction.
        let mut account_ids = std::collections::HashSet::new();
        for entry in &transaction.entries {
            if !account_ids.insert(entry.account_id) {
                return Err(books_error!("errors.transaction_single_entry_per_account"));
            }
        }

        // for each original transaction entry that is reconciled or outstanding find the matching transaction entry
        // if the transaction entry is not the same return error
        if let Some(orginal_transaction) = self.transactions.iter().find(|t| t.id == transaction.id)
        {
            for original_entry in orginal_transaction.entries.iter() {
                if original_entry.is_reconciled_or_outstanding() {
                    let matching_entry = transaction
                        .entries
                        .iter()
                        .find(|e| e.account_id == original_entry.account_id);
                    if matching_entry.is_none() || matching_entry.unwrap() != original_entry {
                        return Err(books_error!("errors.reconciled_entry_immutable"));
                    }
                }
            }
        }

        // If the transaction is net new,
        // or the original_transaction has entries that are not flaged as reconciled or outstanding,
        // or the orginal_transaction does not have an entry
        // check that the entrie's dates are after their account's reconciliation date

        let original_transaction = self.transactions.iter().find(|t| t.id == transaction.id);

        for entry in &transaction.entries {

            if original_transaction.is_none_or(|original_transaction| {
                let matching_entries: Vec<_> = original_transaction
                    .entries
                    .iter()
                    .filter(|e| e.account_id == entry.account_id)
                    .collect();

                matching_entries.is_empty() || matching_entries.iter().any(|e| !e.is_reconciled_or_outstanding())
            }) {
                // Check if account exists and has reconciliation info
                if let Some(account) = self.accounts.get(&entry.account_id) {
                    if let Some(reconciliation_info) = &account.reconciliation_info {
                        if reconciliation_info.date > entry.date {
                            return Err(books_error!(
                                "errors.transaction_before_reconciliation_date",
                                transaction_date = entry.date,
                                reconciliation_date = reconciliation_info.date
                            ));
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn update_transaction(&mut self, transaction: Transaction) -> Result<(), BooksError> {
        self.validate_transaction(&transaction)?;

        if let Some(index) = self
            .transactions
            .iter()
            .position(|t| t.id == transaction.id)
        {
            self.flag_interest_outdated(&transaction);
            let old = std::mem::replace(&mut self.transactions[index], transaction);
            self.flag_interest_outdated(&old);
            Ok(())
        } else {
            Err(books_error!("errors.transaction_not_found", id => transaction.id))
        }
    }

    fn reconcile_transaction(
        &mut self,
        mut transaction: Transaction,
        account_id: Uuid,
        status: ReconciledStatus,
    ) -> Result<(), BooksError> {
        self.validate_transaction(&transaction)?;

        let transaction_id = transaction.id;

        if let Some(entry) = transaction
            .entries
            .iter_mut()
            .find(|e| e.account_id == account_id)
        {
            if entry.reconciled_status.is_none_or(|rs| rs != status) {
                entry.reconciled_status = Some(status);
                transaction.status = TransactionStatus::Recorded;
                if let Some(index) = self
                    .transactions
                    .iter()
                    .position(|t| t.id == transaction_id)
                {
                    let _old = std::mem::replace(&mut self.transactions[index], transaction);
                } else {
                    return Err(books_error!("errors.transaction_not_found", id => transaction_id));
                }
            }
        }

        Ok(())
    }

    pub fn delete_transaction(&mut self, id: &Uuid) -> Result<(), BooksError> {
        if let Some(index) = self.transactions.iter().position(|t| t.id == *id) {
            // Check reconciled status first
            if self.transactions[index]
                .entries
                .iter()
                .any(|e| e.is_reconciled_or_outstanding())
            {
                return Err(books_error!("errors.cannot_delete_reconciled_transaction"));
            }
            let transaction = self.transactions.remove(index);
            self.flag_interest_outdated(&transaction);
            Ok(())
        } else {
            return Err(books_error!("errors.transaction_not_found", id => id));
        }
    }

    pub fn transactions(&self) -> &[Transaction] {
        self.transactions.as_slice()
    }

    pub fn transaction(&self, transaction_id: Uuid) -> Option<Transaction> {
        let matches: Vec<Transaction> = self
            .transactions
            .iter()
            .filter(|t| t.id == transaction_id)
            .map(|t| t.clone())
            .collect();

        if matches.len() > 0 {
            return Some(matches[0].clone());
        }

        None
    }

    fn flag_interest_outdated(&mut self, transaction: &Transaction) -> bool {
        if self.interest_outdated() {
            return true;
        }
        transaction.entries.iter().any(|entry| {
            if let Some(account) = self.accounts.get(&entry.account_id) {
                if account.interest_id.is_some() {
                    self.recalculate_interest.insert(account.id);
                }
            }
            self.interest_outdated()
        })
    }

    fn flag_interest_outdated_by_account(&mut self, account: &Account) -> bool {
        if self.interest_outdated() {
            return true;
        }

        if account.interest_id.is_some() {
            self.recalculate_interest.insert(account.id);
            return true;
        }
        false
    }

    /// Get a copy of the entries with balances for a given Account.
    pub fn account_entries(&self, account_id: Uuid) -> Result<Vec<Entry>, BooksError> {
        if !self.accounts.contains_key(&account_id) {
            return Err(books_error!("errors.account_not_found", id => account_id));
        }

        let mut account_transactions: Vec<Transaction> = self
            .transactions
            .iter()
            .filter(|t| t.involves_account(&account_id))
            .map(|t| t.clone())
            .collect();

        sort_transactions_by_account(
            &mut account_transactions,
            Some(account_id),
            TransactionSortOrder::OldestFirst,
        );
        let account = self.accounts.get(&account_id).unwrap();
        let mut balance = account.starting_balance;
        let mut account_entries: Vec<Entry> = Vec::new();
        account_transactions.iter().for_each(|t| {
            t.account_entries(account_id).iter().for_each(|e| {
                if e.entry_type == account.normal_balance() {
                    balance = balance + e.amount;
                } else {
                    balance = balance - e.amount;
                };
                let mut new_e = e.clone();
                new_e.set_balance(Some(balance.clone()));
                account_entries.push(new_e);
            })
        });

        account_entries.sort_by(|a, b| a.date.cmp(&b.date));
        Ok(account_entries)
    }

    /// Get a copy of the transactions with balances for a given Account.
    pub fn account_transactions(&self, account_id: Uuid) -> Result<Vec<Transaction>, BooksError> {
        if !self.accounts.contains_key(&account_id) {
            return Err(books_error!("errors.account_not_found", id => account_id));
        }

        let account_transactions: Vec<(usize, Transaction)> = self
            .transactions
            .iter()
            .filter(|t| t.involves_account(&account_id))
            .enumerate()
            .map(|(idx, t)| (idx, t.clone()))
            .collect();

        let mut account_transactions: Vec<Transaction> =
            account_transactions.into_iter().map(|(_, t)| t).collect();

        sort_transactions_by_account(
            &mut account_transactions,
            Some(account_id),
            TransactionSortOrder::OldestFirst,
        );
        let account = self.accounts.get(&account_id).unwrap();
        let mut balance = account.starting_balance;

        for i in 0..account_transactions.len() {
            balance = account_transactions[i].update_balance(balance, account);
        }
        Ok(account_transactions)
    }

    pub fn add_schedule(&mut self, schedule: Schedule) -> Result<(), BooksError> {
        self.validate_schedule(&schedule)?;
        self.scheduler.add_schedule(schedule);
        Ok(())
    }

    fn validate_schedule(&mut self, schedule: &Schedule) -> Result<(), BooksError> {
        if schedule.entries.len() < 1 {
            return Err(books_error!("errors.schedule_requires_entry"));
        }

        for e in schedule.entries.iter() {
            self.valid_account_id(e.account_id)?;
        }

        Ok(())
    }

    pub fn update_schedule(&mut self, schedule: Schedule) -> Result<(), BooksError> {
        self.validate_schedule(&schedule)?;
        self.scheduler.update_schedule(schedule)
    }

    pub fn delete_schedule(&mut self, id: &Uuid) -> Result<(), BooksError> {
        // Check if schedule exists
        if !self.scheduler.schedules().iter().any(|s| s.id == *id) {
            return Err(books_error!("errors.schedule_not_found", id => id));
        }

        // Check if any transactions reference this schedule
        if self
            .transactions
            .iter()
            .any(|t| t.source_type == Some(Source::Schedule) && t.source_id == Some(*id))
        {
            return Err(books_error!("errors.schedule_cannot_delete_with_transactions", id => id));
        }

        self.scheduler.delete_schedule(id)
    }

    pub fn schedules(&self) -> &[Schedule] {
        self.scheduler.schedules()
    }

    pub fn get_schedule(&self, schedule_id: Uuid) -> Result<Schedule, BooksError> {
        self.scheduler.get_schedule(schedule_id).map(|s| s.clone())
    }

    pub fn end_date(&self) -> Option<NaiveDate> {
        self.scheduler.end_date()
    }

    pub fn transactions_by_schedule(
        &self,
        schedule_id: Uuid,
        status: Option<TransactionStatus>,
    ) -> Vec<Transaction> {
        self.transactions
            .iter()
            .filter(|t| t.source_type == Some(Source::Schedule) && t.source_id == Some(schedule_id))
            .filter(|t| match status {
                Some(filter_status) => t.status == filter_status,
                None => true,
            })
            .map(|t| t.clone())
            .collect()
    }

    pub fn transactions_by_interest(
        &self,
        interest_id: Uuid,
        status: Option<TransactionStatus>,
        from: Option<NaiveDate>,
    ) -> Vec<Transaction> {
        self.transactions
            .iter()
            .filter(|t| t.source_type == Some(Source::Interest) && t.source_id == Some(interest_id))
            .filter(|t| match from {
                Some(filter_from) => t.date() >= Some(filter_from),
                None => true,
            })
            .filter(|t| match status {
                Some(filter_status) => t.status == filter_status,
                None => true,
            })
            .map(|t| t.clone())
            .collect()
    }

    pub fn add_modifier(&mut self, modifier: Modifier) -> Result<(), BooksError> {
        if let Some(value) = self.validate_modifier(&modifier) {
            return value;
        }

        self.scheduler.add_modifier(modifier);
        Ok(())
    }

    fn validate_modifier(&mut self, _modifier: &Modifier) -> Option<Result<(), BooksError>> {
        // Add validation logic for modifiers if needed
        // For now, no validation required
        None
    }

    pub fn update_modifier(&mut self, modifier: Modifier) -> Result<(), BooksError> {
        if let Some(value) = self.validate_modifier(&modifier) {
            return value;
        }

        self.scheduler.update_modifier(modifier)
    }

    pub fn delete_modifier(&mut self, id: &Uuid) -> Result<(), BooksError> {
        // Check if modifier exists
        if !self.scheduler.modifiers().iter().any(|m| m.id == *id) {
            return Err(books_error!("errors.modifier_not_found", id => id));
        }

        self.scheduler.delete_modifier(id)
    }

    pub fn modifiers(&self) -> Vec<&Modifier> {
        self.scheduler.modifiers()
    }

    pub fn get_modifier(&self, modifier_id: Uuid) -> Result<Modifier, BooksError> {
        self.scheduler.get_modifier(modifier_id).map(|m| m.clone())
    }

    pub fn add_interest(&mut self, interest: Interest) -> Result<(), BooksError> {
        if let Some(account) = self.accounts.get_mut(&interest.account_id) {
            account.interest_id = Some(interest.id);
            self.validate_interest(&interest)?;
            self.check_recalculate_interest(&interest);
            self.interests.insert(interest.id, interest);
            Ok(())
        } else {
            return Err(books_error!("errors.account_not_found", id => interest.account_id));
        }
    }

    pub fn get_interest(&self, interest_id: &Uuid) -> Result<Interest, BooksError> {
        self.interests
            .get(interest_id)
            .cloned()
            .ok_or(books_error!("errors.interest_info_not_found", id => interest_id))
    }

    pub fn update_interest(&mut self, interest: Interest) -> Result<(), BooksError> {
        self.validate_interest(&interest)?;
        self.check_recalculate_interest(&interest);

        let account = self
            .accounts
            .get(&interest.account_id)
            .ok_or(books_error!("errors.account_not_found", id => interest.account_id))?;

        if let Some(interest_id) = account.interest_id {
            let old = self.interests.insert(interest_id, interest);
            if let Some(old_interest) = old {
                self.check_recalculate_interest(&old_interest);
            }
        } else {
            self.add_interest(interest)?;
        }

        Ok(())
    }

    fn check_recalculate_interest(&mut self, interest: &Interest) {
        self.recalculate_interest.insert(interest.account_id);

        for t in &interest.terms {
            if let Some(interest_account_id) = t.interest_account_id {
                self.recalculate_interest.insert(interest_account_id);
            }
        }
    }

    pub fn interests(&self) -> Vec<&Interest> {
        self.interests.values().collect()
    }

    fn validate_interest(&mut self, interest: &Interest) -> Result<(), BooksError> {
        self.valid_account_id(interest.account_id)?;

        for t in interest.terms.as_slice() {
            if let Some(interest_account_id) = t.interest_account_id {
                self.valid_account_id(interest_account_id)?;
                let account = self.get_account(&interest_account_id)?;
                if account.account_type != AccountType::Asset
                    && account.account_type != AccountType::Liability
                {
                    return Err(books_error!("errors.invalid_account_type", name => account.name));
                }
            }

            if let Some(income_account_id) = t.income_account_id {
                self.valid_account_id(income_account_id)?;
                let account = self.get_account(&income_account_id)?;
                if account.account_type != AccountType::Revenue
                    && account.account_type != AccountType::Expense
                {
                    return Err(books_error!("errors.invalid_account_type", name => account.name));
                }
            }
        }

        Ok(())
    }

    pub fn reset_schedule_last_date(
        &mut self,
        schedule_id: Uuid,
    ) -> Result<Option<NaiveDate>, BooksError> {
        let mut transactions: Vec<Transaction> = self
            .transactions
            .iter()
            .filter(|t| t.source_type == Some(Source::Schedule) && t.source_id == Some(schedule_id))
            .map(|t| t.clone())
            .collect();

        // Sort transactions by date to find the latest one
        sort_transactions_by_account(&mut transactions, None, TransactionSortOrder::OldestFirst);

        let new_last = transactions.last().and_then(|t| t.date());

        let existing_schedule = self.scheduler.get_schedule(schedule_id)?;
        self.scheduler.update_schedule(Schedule {
            id: schedule_id,
            last_date: new_last,
            ..existing_schedule.clone()
        })?;
        Ok(new_last)
    }

    /// Reconcile a list of transactions against the books for a given account.
    pub fn prepare_reconciliation(
        &self,
        account_id: Uuid,
        reconcile_transactions: Vec<Transaction>,
    ) -> Result<Vec<ReconciliationItem>, BooksError> {
        // 1) Normalize input to account-related transactions and sort by account date/order.
        let mut input_txns: Vec<Transaction> = reconcile_transactions
            .into_iter()
            .filter(|t| t.involves_account(&account_id))
            .collect();
        sort_transactions_by_account(
            &mut input_txns,
            Some(account_id),
            TransactionSortOrder::OldestFirst,
        );

        // 2) Load existing account transactions (with balances) and track matched indices.
        let existing_txns = self.account_transactions(account_id)?;
        let existing_targets: Vec<(usize, Uuid, Entry)> = existing_txns
            .iter()
            .enumerate()
            .filter_map(|(i, txn)| {
                txn.find_entry_by_account(&account_id)
                    .map(|target| (i, txn.id, target.clone()))
            })
            .collect();
        let mut matched_indices: HashSet<usize> = HashSet::new();
        let preselected_same_day_matches = select_same_day_set_matches(
            &input_txns,
            account_id,
            &existing_targets,
            &mut matched_indices,
        );

        // Process rows with strongest potential linkage first so weak candidates
        // do not consume targets needed by stronger exact matches later.
        let empty_matches: HashSet<usize> = HashSet::new();
        let input_priorities: Vec<Option<MatchPriority>> = input_txns
            .iter()
            .map(|input| {
                let entry = input
                    .find_entry_by_account(&account_id)
                    .expect("transaction involves account");
                select_best_candidate(entry, &existing_targets, &empty_matches)
                    .map(|(_, _, candidate)| MatchPriority::from_candidate(&candidate))
            })
            .collect();
        let mut input_match_order: Vec<usize> = (0..input_txns.len()).collect();
        input_match_order.sort_by(|a_idx, b_idx| {
            match (input_priorities[*a_idx], input_priorities[*b_idx]) {
                (Some(a), Some(b)) => cmp_match_priority(b, a).then_with(|| a_idx.cmp(b_idx)),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => a_idx.cmp(b_idx),
            }
        });

        let mut reconciliation_results: Vec<Option<ReconciliationItem>> =
            vec![None; input_txns.len()];

        for input_idx in input_match_order {
            let input = &input_txns[input_idx];
            // 3) Extract the account entry details from the input transaction.
            let entry = input
                .find_entry_by_account(&account_id)
                .expect("transaction involves account");
            let expected_balance = entry.balance;

            // 4) Score candidate matches inline and choose the highest-confidence eligible target.
            let best_candidate = preselected_same_day_matches
                .get(&input_idx)
                .map(|selected| {
                    let mut candidate = selected.candidate.clone();
                    if selected.promote_to_matched
                        && candidate.status != ReconciliationMatchStatus::Matched
                    {
                        candidate.confidence = adjust_confidence_for_status(
                            candidate.confidence,
                            candidate.status,
                            ReconciliationMatchStatus::Matched,
                        );
                        candidate.status = ReconciliationMatchStatus::Matched;
                    }
                    (selected.target_idx, selected.target_txn_id, candidate)
                })
                .or_else(|| select_best_candidate(entry, &existing_targets, &matched_indices));

            let (status, matched_id, confidence, signals) =
                if let Some((match_idx, matched_txn_id, candidate)) = best_candidate {
                    matched_indices.insert(match_idx);
                    (
                        candidate.status,
                        Some(matched_txn_id),
                        candidate.confidence,
                        candidate.signals,
                    )
                } else {
                    let (confidence, signals) = score_unmatched_reconciliation(
                        entry,
                        existing_targets
                            .iter()
                            .filter(|(idx, _, _)| !matched_indices.contains(idx))
                            .map(|(_, _, candidate)| candidate),
                    );
                    (
                        ReconciliationMatchStatus::Unmatched,
                        None,
                        confidence,
                        signals,
                    )
                };

            // 5) Record this input transaction's reconciliation outcome.
            reconciliation_results[input_idx] =
                Some(ReconciliationItem::Reconciliation(ReconciliationResult {
                transaction: input.clone(),
                status,
                balance: expected_balance,
                matched_transaction_id: matched_id,
                confidence,
                signals,
            }));
        }
        let results: Vec<ReconciliationItem> = reconciliation_results
            .into_iter()
            .map(|item| item.expect("reconciliation result generated for each input row"))
            .collect();

        // 6) Add existing transactions to results, splicing matched reconciliation transactions immediately after their targets
        let mut final_results: Vec<ReconciliationItem> =
            Vec::with_capacity(existing_txns.len() + results.len());
        let mut reconciliation_lookup: std::collections::HashMap<Uuid, Vec<&ReconciliationItem>> =
            std::collections::HashMap::new();

        // Group reconciliation transactions by their matched target ID
        for reconciliation_item in &results {
            if let ReconciliationItem::Reconciliation(recon_result) = reconciliation_item {
                if let Some(matched_id) = recon_result.matched_transaction_id {
                    reconciliation_lookup
                        .entry(matched_id)
                        .or_insert_with(Vec::new)
                        .push(reconciliation_item);
                }
            }
        }

        // Add existing transactions with their matched reconciliation transactions
        for existing_txn in existing_txns.iter() {
            // Check if this existing transaction has any matches
            let matched_reconciliations = reconciliation_lookup.get(&existing_txn.id);

            // Determine the status based on matches
            let status = if let Some(matches) = matched_reconciliations {
                if matches.len() > 1 {
                    // If multiple matches, use the first one's status
                    matches[0].status()
                } else if matches.len() == 1 {
                    matches[0].status()
                } else {
                    ReconciliationMatchStatus::Unmatched
                }
            } else {
                ReconciliationMatchStatus::Unmatched
            };

            // Add the original existing transaction with updated status
            let mut original_item = ReconciliationItem::Original(TargetResult {
                transaction: existing_txn.clone(),
                status,
                matched_reconciliation_id: None,
                confidence: 0.0,
                signals: Vec::new(),
            });

            // If there are matches, set the matched_reconciliation_id to the first match's transaction ID
            if let Some(matches) = matched_reconciliations {
                if !matches.is_empty() {
                    if let ReconciliationItem::Reconciliation(recon_result) = matches[0] {
                        if let Some(_target_id) = recon_result.matched_transaction_id {
                            if let ReconciliationItem::Original(ref mut target_result) =
                                original_item
                            {
                                target_result.matched_reconciliation_id =
                                    Some(recon_result.transaction.id);
                                target_result.confidence = recon_result.confidence;
                                target_result.signals = recon_result.signals.clone();
                            }
                        }
                    }
                }
            }

            final_results.push(original_item);

            // Add any reconciliation transactions that matched this existing transaction
            if let Some(matched_reconciliations) = matched_reconciliations {
                for reconciliation_item in matched_reconciliations {
                    final_results.push((*reconciliation_item).clone());
                }
            }
        }

        // Add unmatched reconciliation transactions, inserting them by date
        // Group unmatched transactions by date for efficient insertion
        let mut unmatched_by_date: std::collections::HashMap<
            chrono::NaiveDate,
            Vec<&ReconciliationItem>,
        > = std::collections::HashMap::new();

        for reconciliation_item in &results {
            if let ReconciliationItem::Reconciliation(recon_result) = reconciliation_item {
                if recon_result.matched_transaction_id.is_none() {
                    let entry = recon_result
                        .transaction
                        .find_entry_by_account(&account_id)
                        .expect("reconciliation transaction involves account");
                    unmatched_by_date
                        .entry(entry.date)
                        .or_insert_with(Vec::new)
                        .push(reconciliation_item);
                }
            }
        }

        // Insert unmatched transactions by date
        for (date, unmatched_items) in unmatched_by_date {
            // Find the position where this date should be inserted (as last item of this date)
            let mut insert_position = final_results.len(); // Default to end if no suitable position found

            for (i, result_item) in final_results.iter().enumerate() {
                let item_date = match result_item {
                    ReconciliationItem::Reconciliation(recon) => {
                        recon
                            .transaction
                            .find_entry_by_account(&account_id)
                            .expect("reconciliation transaction involves account")
                            .date
                    }
                    ReconciliationItem::Original(target) => {
                        target
                            .transaction
                            .find_entry_by_account(&account_id)
                            .expect("target transaction involves account")
                            .date
                    }
                };

                if item_date > date {
                    insert_position = i;
                    break;
                } else if item_date == date {
                    // Continue looking to find the last item with this date
                    insert_position = i + 1;
                }
            }

            // Insert all unmatched transactions for this date at the calculated position
            for (offset, unmatched_item) in unmatched_items.iter().enumerate() {
                final_results.insert(insert_position + offset, (*unmatched_item).clone());
            }
        }

        let realignment_eligible_reconciliation_ids: HashSet<Uuid> = final_results
            .iter()
            .filter_map(|item| match item {
                ReconciliationItem::Reconciliation(recon)
                    if is_balance_only_partial_for_realignment(recon) =>
                {
                    Some(recon.transaction.id)
                }
                _ => None,
            })
            .collect();

        // 7) If balances realign later via a direct Matched row (and no Unmatched in between):
        // - treat earlier Mismatch as PartialMatch
        // - upgrade earlier balance-only PartialMatch to Matched
        let mut mismatched_indices: Vec<usize> = Vec::new();
        let mut partial_indices: Vec<usize> = Vec::new();
        for i in 0..final_results.len() {
            match final_results[i].status() {
                ReconciliationMatchStatus::Unmatched => {
                    mismatched_indices.clear();
                    partial_indices.clear();
                }
                ReconciliationMatchStatus::Mismatch => {
                    mismatched_indices.push(i);
                }
                ReconciliationMatchStatus::PartialMatch => {
                    if is_realignment_upgrade_candidate_item(
                        &final_results[i],
                        &realignment_eligible_reconciliation_ids,
                    ) {
                        partial_indices.push(i);
                    }
                }
                ReconciliationMatchStatus::Matched => {
                    for idx in mismatched_indices.drain(..) {
                        final_results[idx].set_status(ReconciliationMatchStatus::PartialMatch);
                        if let ReconciliationItem::Reconciliation(recon) = &mut final_results[idx] {
                            recon.confidence = adjust_confidence_for_status(
                                recon.confidence,
                                ReconciliationMatchStatus::Mismatch,
                                ReconciliationMatchStatus::PartialMatch,
                            );
                        }
                    }
                    for idx in partial_indices.drain(..) {
                        final_results[idx].set_status(ReconciliationMatchStatus::Matched);
                        if let ReconciliationItem::Reconciliation(recon) = &mut final_results[idx] {
                            recon.confidence = adjust_confidence_for_status(
                                recon.confidence,
                                ReconciliationMatchStatus::PartialMatch,
                                ReconciliationMatchStatus::Matched,
                            );
                        }
                    }
                }
            }
        }

        Ok(final_results)
    }

    pub fn reconcile_account_transactions(
        &mut self,
        account_id: Uuid,
        transaction_ids: Vec<Uuid>,
    ) -> Result<(), BooksError> {
        println!(
            "Reconciling account transactions for account {} transactions: {:?}",
            account_id, transaction_ids
        );
        if !self.accounts.contains_key(&account_id) {
            return Err(books_error!("errors.account_not_found", id => account_id));
        }

        let mut account_transactions = self.account_transactions(account_id)?;
        let mut new_recon_transaction: Option<Transaction> = None;
        // set the last index to the account reconciliation_info transaction_id index
        let mut last_index: Option<usize> = self
            .accounts
            .get(&account_id)
            .unwrap()
            .reconciliation_info
            .as_ref()
            .map(|info| info.transaction_id)
            .map(|id| {
                account_transactions
                    .iter()
                    .position(|t| t.id == id)
                    .unwrap()
            });
        let mut first_index: Option<usize> = None;

        // Reconcile each transaction.
        for transaction_id in transaction_ids {
            let idx = account_transactions.iter().position(|t| t.id == transaction_id).ok_or_else(|| {
                books_error!("errors.transaction_not_found_for_account", transaction_id => transaction_id, account_id => account_id)
            })?;

            let transaction = account_transactions.iter_mut().find(|t| t.id == transaction_id).ok_or_else(|| {
                books_error!("errors.transaction_not_found_for_account", transaction_id => transaction_id, account_id => account_id)
            })?;

            self.reconcile_transaction(
                transaction.clone(),
                account_id,
                ReconciledStatus::Reconciled,
            )?;

            if last_index.is_none_or(|li| li < idx) {
                last_index = Some(idx);
                new_recon_transaction = Some(transaction.clone())
            }

            if first_index.is_none_or(|fi| fi > idx) {
                first_index = Some(idx);
            }
        }

        // Flag any now outstanding transactions before the first transaction.
        if let Some(first_index) = first_index {
            for earlier_transaction in
                account_transactions
                    .iter_mut()
                    .take(first_index)
                    .filter(|t| {
                        t.find_entry_by_account(&account_id)
                            .is_some_and(|e| e.reconciled_status.is_none())
                    })
            {
                self.reconcile_transaction(
                    earlier_transaction.clone(),
                    account_id,
                    ReconciledStatus::Outstanding,
                )?;
            }
        }

        // Set the reconciliation info for the account.
        if let Some(t) = new_recon_transaction {
            if let Some(account) = self.accounts.get_mut(&account_id) {
                let last_entry = t.find_entry_by_account(&account_id).unwrap();
                account.reconciliation_info = Some(crate::account::ReconciliationInfo {
                    date: last_entry.date,
                    balance: last_entry.balance.unwrap(),
                    transaction_id: t.id,
                });
            }
        }

        Ok(())
    }

    pub fn rollback_reconciliation(
        &mut self,
        account_id: Uuid,
        to_date: NaiveDate,
    ) -> Result<(), BooksError> {
        if !self.accounts.contains_key(&account_id) {
            return Err(books_error!("errors.account_not_found", id => account_id));
        }

        let account_transactions = self.account_transactions(account_id)?;
        let mut last_reconciled_index: Option<usize> = None;
        let mut last_reconciled_info: Option<crate::account::ReconciliationInfo> = None;

        for (idx, transaction) in account_transactions.iter().enumerate() {
            if let Some(entry) = transaction.find_entry_by_account(&account_id) {
                if entry.is_reconciled() && entry.date <= to_date {
                    last_reconciled_index = Some(idx);
                    let balance = entry.balance.ok_or_else(|| {
                        books_error!("errors.reconciliation_rollback_requires_balances")
                    })?;
                    last_reconciled_info = Some(crate::account::ReconciliationInfo {
                        date: entry.date,
                        balance,
                        transaction_id: transaction.id,
                    });
                }
            }
        }

        if let Some(account) = self.accounts.get_mut(&account_id) {
            account.reconciliation_info = last_reconciled_info;
        }

        for (idx, transaction) in account_transactions.iter().enumerate() {
            let should_clear = match last_reconciled_index {
                Some(last_idx) => idx > last_idx,
                None => true,
            };

            if should_clear {
                if let Some(existing) = self
                    .transactions
                    .iter_mut()
                    .find(|t| t.id == transaction.id)
                {
                    if let Some(entry) = existing
                        .entries
                        .iter_mut()
                        .find(|e| e.account_id == account_id)
                    {
                        entry.reconciled_status = None;
                    }
                }
            }
        }

        Ok(())
    }

    fn valid_account_id(&self, id: Uuid) -> Result<(), BooksError> {
        if self.accounts.contains_key(&id) {
            Ok(())
        } else {
            Err(books_error!("errors.account_not_found", id => id))
        }
    }

    pub fn recalculate_interest(&mut self) -> Result<(), BooksError> {
       let projection_date = self.get_projection_date();
        println!("Calculating interest to {}...", projection_date);
        let interest_accounts = self
            .accounts()
            .into_iter()
            .filter(|a| self.recalculate_interest.contains(&a.id) && a.interest_id.is_some())
            .collect();
        calculate_interest_for_accounts(self, interest_accounts, projection_date)?;
        println!("Interest up-to-date ✅");
        Ok(())
    }

    pub fn reset_interest_flag(&mut self) {
        self.recalculate_interest.clear();
    }

    pub fn interest_outdated(&self) -> bool {
        !self.recalculate_interest.is_empty()
    }

    pub fn run_checks_and_update(&mut self) -> Result<(), BooksError> {
        let projection_date = self.get_projection_date();
        println!("Running checks 📋  Projection date: {}", projection_date);
        println!("Generating schedules...");
        self.generate(projection_date);
        let interest_accounts = self
            .accounts
            .values()
            .filter(|a| a.interest_id.is_some())
            .cloned()
            .collect();
        println!("Calculating interest...");
        calculate_interest_for_accounts(self, interest_accounts, projection_date)?;
        self.settings.projected_to = Some(projection_date);
        println!("Checks completed ✅");
        Ok(())
    }

    fn get_projection_date(&mut self) -> NaiveDate {
        let today = chrono::Utc::now().date_naive();
        let projection_date = today
            .checked_add_months(chrono::Months::new(self.settings.projection_months))
            .unwrap();
        projection_date
    }
}

fn default_projection_months() -> u32 {
    DEFAULT_PROJECTION_MONTHS
}

#[derive(Clone)]
struct MatchCandidate {
    status: ReconciliationMatchStatus,
    confidence: f32,
    amount_variance: f32,
    side_variance: f32,
    description_variance: f32,
    date_days: f32,
    balance_variance: Option<f32>,
    signals: Vec<Signal>,
}

struct CandidateVariances {
    amount: f32,
    side: f32,
    date: f32,
    date_days: f32,
    description: f32,
    balance: Option<f32>,
}

fn evaluate_match_candidate(rec_entry: &Entry, target_entry: &Entry) -> Option<MatchCandidate> {
    let within_14_days = (target_entry.date - rec_entry.date).num_days().abs() <= 14;
    if !within_14_days {
        return None;
    }

    let variances = calculate_candidate_variances(rec_entry, target_entry);
    let confidence = variance_score(&variances);
    let strong_anchor_count = [
        variances.amount <= 0.10,
        variances.date <= 0.15,
        variances.description <= 0.35,
    ]
    .into_iter()
    .filter(|is_anchor| *is_anchor)
    .count();

    // Guard against opportunistic weak matches that can steal a better target.
    if strong_anchor_count < 2 {
        return None;
    }

    let exact_identity_match = variances.amount == 0.0
        && variances.side == 0.0
        && variances.date_days <= 1.0
        && variances.description <= 0.5;
    let exact_balance_match = variances.balance.unwrap_or(0.0) == 0.0;
    let exact_match = exact_identity_match && variances.date_days <= 1.0 && exact_balance_match;

    // Balance drift should not overrule a clear identity match (amount+description+side+close date).
    let status = if exact_match {
        ReconciliationMatchStatus::Matched
    } else if exact_identity_match {
        ReconciliationMatchStatus::PartialMatch
    } else if variances.balance.is_some_and(|b| b > 0.0) {
        if confidence >= 0.45 {
            ReconciliationMatchStatus::Mismatch
        } else {
            return None;
        }
    } else if confidence >= 0.75 {
        ReconciliationMatchStatus::PartialMatch
    } else if confidence >= 0.45 {
        ReconciliationMatchStatus::Mismatch
    } else {
        return None;
    };

    let mut signals: Vec<Signal> = vec![
        Signal::new(Field::Amount, variances.amount),
        Signal::new(Field::Side, variances.side),
        Signal::new(Field::Date, variances.date_days),
        Signal::new(Field::Description, variances.description),
    ];
    if let Some(balance_variance) = variances.balance {
        signals.push(Signal::new(Field::Balance, balance_variance));
    }

    Some(MatchCandidate {
        status,
        confidence: clamp_reconciliation_confidence(confidence),
        amount_variance: variances.amount,
        side_variance: variances.side,
        description_variance: variances.description,
        date_days: variances.date_days,
        balance_variance: variances.balance,
        signals,
    })
}

fn calculate_candidate_variances(rec_entry: &Entry, target_entry: &Entry) -> CandidateVariances {
    let amount = relative_decimal_variance(rec_entry.amount, target_entry.amount);
    let side = if rec_entry.entry_type == target_entry.entry_type {
        0.0
    } else {
        1.0
    };
    let date_days = (rec_entry.date - target_entry.date).num_days().abs() as f32;
    let date = (date_days / 14.0).min(1.0);
    let description_similarity =
        reconciliation_description_similarity(&rec_entry.description, &target_entry.description);
    let description = (1.0 - description_similarity).clamp(0.0, 1.0);
    let balance = match (rec_entry.balance, target_entry.balance) {
        (Some(a), Some(b)) => Some(relative_decimal_variance(a, b)),
        _ => None,
    };

    CandidateVariances {
        amount,
        side,
        date,
        date_days,
        description,
        balance,
    }
}

fn relative_decimal_variance(a: Decimal, b: Decimal) -> f32 {
    let delta = (a - b).abs();
    let scale = a.abs().max(b.abs()).max(Decimal::ONE);
    (delta / scale).to_f32().unwrap_or(1.0).min(1.0)
}

fn variance_score(variances: &CandidateVariances) -> f32 {
    let mut weighted_sum = 0.0;
    let mut total_weight = 0.0;

    let weights = [
        (variances.amount, 0.35),
        (variances.side, 0.15),
        (variances.date, 0.20),
        (variances.description, 0.20),
    ];

    for (variance, weight) in weights {
        weighted_sum += variance * weight;
        total_weight += weight;
    }

    if let Some(balance_variance) = variances.balance {
        let balance_weight = 0.10;
        weighted_sum += balance_variance * balance_weight;
        total_weight += balance_weight;
    }

    if total_weight == 0.0 {
        0.0
    } else {
        1.0 - (weighted_sum / total_weight)
    }
}

fn score_unmatched_reconciliation<'a, I>(rec_entry: &Entry, candidates: I) -> (f32, Vec<Signal>)
where
    I: Iterator<Item = &'a Entry>,
{
    let mut signals: Vec<Signal> = vec![Signal::new(Field::Linkage, -1.0)];
    signals.push(build_unmatched_signal(rec_entry, candidates));
    (
        clamp_reconciliation_confidence(status_base_confidence(
            &ReconciliationMatchStatus::Unmatched,
        )),
        signals,
    )
}

fn build_unmatched_signal<'a, I>(rec_entry: &Entry, candidates: I) -> Signal
where
    I: Iterator<Item = &'a Entry>,
{
    let mut best_candidate: Option<(i32, i64)> = None;

    for target in candidates {
        let date_diff = (rec_entry.date - target.date).num_days().abs();
        let desc_similarity =
            reconciliation_description_similarity(&rec_entry.description, &target.description);
        let mut candidate_score = 0_i32;

        if rec_entry.amount == target.amount {
            candidate_score += 4;
        }
        if rec_entry.entry_type == target.entry_type {
            candidate_score += 2;
        }
        if date_diff == 0 {
            candidate_score += 3;
        } else if date_diff <= 1 {
            candidate_score += 2;
        } else if date_diff <= 3 {
            candidate_score += 1;
        }
        if desc_similarity > 0.8 {
            candidate_score += 2;
        } else if desc_similarity > 0.5 {
            candidate_score += 1;
        }

        match best_candidate {
            None => best_candidate = Some((candidate_score, date_diff)),
            Some((best_score, best_diff)) => {
                if candidate_score > best_score
                    || (candidate_score == best_score && date_diff < best_diff)
                {
                    best_candidate = Some((candidate_score, date_diff));
                }
            }
        }
    }

    if let Some((score, date_diff)) = best_candidate {
        if score >= 4 {
            return Signal::new(Field::Candidate, date_diff as f32);
        }
    }

    Signal::new(Field::Candidate, -1.0)
}

fn reconciliation_description_similarity(a: &str, b: &str) -> f32 {
    let a_tokens = tokenize_reconciliation_description(a);
    let b_tokens = tokenize_reconciliation_description(b);
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

fn tokenize_reconciliation_description(description: &str) -> HashSet<String> {
    description
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| token.to_string())
        .collect()
}

fn clamp_reconciliation_confidence(value: f32) -> f32 {
    value.max(0.0).min(0.99)
}

fn status_base_confidence(status: &ReconciliationMatchStatus) -> f32 {
    match status {
        ReconciliationMatchStatus::Matched => 0.78,
        ReconciliationMatchStatus::PartialMatch => 0.55,
        ReconciliationMatchStatus::Mismatch => 0.32,
        ReconciliationMatchStatus::Unmatched => 0.08,
    }
}

fn adjust_confidence_for_status(
    current_confidence: f32,
    old_status: ReconciliationMatchStatus,
    new_status: ReconciliationMatchStatus,
) -> f32 {
    let delta = status_base_confidence(&new_status) - status_base_confidence(&old_status);
    clamp_reconciliation_confidence(current_confidence + delta)
}

fn status_rank(status: &ReconciliationMatchStatus) -> u8 {
    match status {
        ReconciliationMatchStatus::Matched => 3,
        ReconciliationMatchStatus::PartialMatch => 2,
        ReconciliationMatchStatus::Mismatch => 1,
        ReconciliationMatchStatus::Unmatched => 0,
    }
}

fn get_signal_deviation(signals: &[Signal], field: Field) -> Option<f32> {
    signals
        .iter()
        .find(|signal| signal.field == field)
        .map(|signal| signal.deviation)
}

fn is_balance_only_partial_for_realignment(recon: &ReconciliationResult) -> bool {
    if recon.status != ReconciliationMatchStatus::PartialMatch {
        return false;
    }

    const EPSILON: f32 = 0.0001;
    let amount_ok = get_signal_deviation(&recon.signals, Field::Amount)
        .is_some_and(|deviation| deviation.abs() <= EPSILON);
    let side_ok = get_signal_deviation(&recon.signals, Field::Side)
        .is_some_and(|deviation| deviation.abs() <= EPSILON);
    let date_ok = get_signal_deviation(&recon.signals, Field::Date)
        .is_some_and(|deviation| deviation.abs() <= EPSILON);
    let description_ok = get_signal_deviation(&recon.signals, Field::Description)
        .is_some_and(|deviation| deviation <= 0.5 + EPSILON);

    amount_ok && side_ok && date_ok && description_ok
}

fn is_realignment_upgrade_candidate_item(
    item: &ReconciliationItem,
    eligible_reconciliation_ids: &HashSet<Uuid>,
) -> bool {
    match item {
        ReconciliationItem::Reconciliation(recon) => {
            eligible_reconciliation_ids.contains(&recon.transaction.id)
        }
        ReconciliationItem::Original(target) => target
            .matched_reconciliation_id
            .is_some_and(|id| eligible_reconciliation_ids.contains(&id)),
    }
}

#[derive(Clone)]
struct SameDayCandidateOption {
    input_idx: usize,
    target_idx: usize,
    target_txn_id: Uuid,
    candidate: MatchCandidate,
    score: i64,
}

#[derive(Default)]
struct SameDayAssignmentBest {
    count: usize,
    score: i64,
    picks: Vec<SameDayCandidateOption>,
}

struct PreselectedSameDayMatch {
    target_idx: usize,
    target_txn_id: Uuid,
    candidate: MatchCandidate,
    promote_to_matched: bool,
}

fn score_candidate_for_set_matching(candidate: &MatchCandidate) -> i64 {
    let amount_score = ((1.0 - candidate.amount_variance).clamp(0.0, 1.0) * 1_000_000.0) as i64;
    let description_score =
        ((1.0 - candidate.description_variance).clamp(0.0, 1.0) * 100_000.0) as i64;
    let date_score = ((14.0 - candidate.date_days).clamp(0.0, 14.0) * 1_000.0) as i64;
    let confidence_score = (candidate.confidence.clamp(0.0, 1.0) * 1_000.0) as i64;
    let balance_score = candidate
        .balance_variance
        .map(|b| ((1.0 - b).clamp(0.0, 1.0) * 100.0) as i64)
        .unwrap_or(0);
    (status_rank(&candidate.status) as i64) * 1_000_000_000
        + amount_score
        + description_score
        + date_score
        + confidence_score
        + balance_score
}

fn is_exact_identity_same_day_candidate(candidate: &MatchCandidate) -> bool {
    candidate.amount_variance == 0.0
        && candidate.side_variance == 0.0
        && candidate.date_days == 0.0
        && candidate.description_variance <= 0.5
}

fn has_same_day_balance_set_support(
    picks: &[SameDayCandidateOption],
    input_txns: &[Transaction],
    existing_targets: &[(usize, Uuid, Entry)],
    account_id: Uuid,
) -> bool {
    let target_by_idx: HashMap<usize, &Entry> = existing_targets
        .iter()
        .map(|(idx, _, entry)| (*idx, entry))
        .collect();

    let mut input_balances: Vec<Decimal> = Vec::with_capacity(picks.len());
    let mut target_balances: Vec<Decimal> = Vec::with_capacity(picks.len());

    for pick in picks {
        let input_balance = input_txns[pick.input_idx]
            .find_entry_by_account(&account_id)
            .and_then(|entry| entry.balance);
        let target_balance = target_by_idx
            .get(&pick.target_idx)
            .and_then(|entry| entry.balance);

        let (Some(input_balance), Some(target_balance)) = (input_balance, target_balance) else {
            return false;
        };

        input_balances.push(input_balance);
        target_balances.push(target_balance);
    }

    input_balances.sort();
    target_balances.sort();
    input_balances == target_balances
}

fn search_best_same_day_assignment(
    ordered_inputs: &[(usize, Vec<SameDayCandidateOption>)],
    pos: usize,
    used_targets: &mut HashSet<usize>,
    current_score: i64,
    current_picks: &mut Vec<SameDayCandidateOption>,
    best: &mut SameDayAssignmentBest,
) {
    if pos == ordered_inputs.len() {
        let current_count = current_picks.len();
        if current_count > best.count || (current_count == best.count && current_score > best.score) {
            best.count = current_count;
            best.score = current_score;
            best.picks = current_picks.clone();
        }
        return;
    }

    // Allow skipping this row if it cannot be confidently assigned in this set.
    search_best_same_day_assignment(
        ordered_inputs,
        pos + 1,
        used_targets,
        current_score,
        current_picks,
        best,
    );

    for option in &ordered_inputs[pos].1 {
        if used_targets.contains(&option.target_idx) {
            continue;
        }
        used_targets.insert(option.target_idx);
        current_picks.push(option.clone());
        search_best_same_day_assignment(
            ordered_inputs,
            pos + 1,
            used_targets,
            current_score + option.score,
            current_picks,
            best,
        );
        current_picks.pop();
        used_targets.remove(&option.target_idx);
    }
}

fn select_same_day_set_matches(
    input_txns: &[Transaction],
    account_id: Uuid,
    existing_targets: &[(usize, Uuid, Entry)],
    matched_indices: &mut HashSet<usize>,
) -> HashMap<usize, PreselectedSameDayMatch> {
    let mut input_by_date: std::collections::BTreeMap<NaiveDate, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (idx, txn) in input_txns.iter().enumerate() {
        let entry = txn
            .find_entry_by_account(&account_id)
            .expect("transaction involves account");
        input_by_date.entry(entry.date).or_default().push(idx);
    }

    let mut selections: HashMap<usize, PreselectedSameDayMatch> = HashMap::new();

    for (date, input_indices) in input_by_date {
        if input_indices.len() < 2 {
            continue;
        }

        let target_for_day: Vec<(usize, Uuid, &Entry)> = existing_targets
            .iter()
            .filter(|(target_idx, _, target_entry)| {
                target_entry.date == date && !matched_indices.contains(target_idx)
            })
            .map(|(target_idx, target_txn_id, target_entry)| (*target_idx, *target_txn_id, target_entry))
            .collect();

        if target_for_day.len() < 2 {
            continue;
        }

        let mut options_by_input: Vec<(usize, Vec<SameDayCandidateOption>)> = Vec::new();
        for input_idx in input_indices.iter().copied() {
            let input_entry = input_txns[input_idx]
                .find_entry_by_account(&account_id)
                .expect("transaction involves account");
            let mut options: Vec<SameDayCandidateOption> = Vec::new();
            for (target_idx, target_txn_id, target_entry) in &target_for_day {
                if let Some(candidate) = evaluate_match_candidate(input_entry, target_entry) {
                    if status_rank(&candidate.status) < status_rank(&ReconciliationMatchStatus::PartialMatch) {
                        continue;
                    }
                    options.push(SameDayCandidateOption {
                        input_idx,
                        target_idx: *target_idx,
                        target_txn_id: *target_txn_id,
                        score: score_candidate_for_set_matching(&candidate),
                        candidate,
                    });
                }
            }
            if !options.is_empty() {
                options_by_input.push((input_idx, options));
            }
        }

        if options_by_input.len() < 2 {
            continue;
        }

        options_by_input.sort_by_key(|(_, options)| options.len());
        let mut best = SameDayAssignmentBest::default();
        let mut used_targets: HashSet<usize> = HashSet::new();
        let mut current_picks: Vec<SameDayCandidateOption> = Vec::new();
        search_best_same_day_assignment(
            &options_by_input,
            0,
            &mut used_targets,
            0,
            &mut current_picks,
            &mut best,
        );

        if best.count < 2 {
            continue;
        }

        let full_bijection_size = input_indices.len().min(target_for_day.len());
        let promote_cluster = best.count == full_bijection_size
            && best
                .picks
                .iter()
                .all(|pick| is_exact_identity_same_day_candidate(&pick.candidate))
            && has_same_day_balance_set_support(
                &best.picks,
                input_txns,
                existing_targets,
                account_id,
            );

        for pick in best.picks {
            matched_indices.insert(pick.target_idx);
            selections.insert(
                pick.input_idx,
                PreselectedSameDayMatch {
                    target_idx: pick.target_idx,
                    target_txn_id: pick.target_txn_id,
                    candidate: pick.candidate,
                    promote_to_matched: promote_cluster,
                },
            );
        }
    }

    selections
}

#[derive(Clone, Copy)]
struct MatchPriority {
    status_rank: u8,
    amount_variance: f32,
    side_variance: f32,
    description_variance: f32,
    date_days: f32,
    confidence: f32,
    balance_variance: Option<f32>,
}

impl MatchPriority {
    fn from_candidate(candidate: &MatchCandidate) -> Self {
        Self {
            status_rank: status_rank(&candidate.status),
            amount_variance: candidate.amount_variance,
            side_variance: candidate.side_variance,
            description_variance: candidate.description_variance,
            date_days: candidate.date_days,
            confidence: candidate.confidence,
            balance_variance: candidate.balance_variance,
        }
    }
}

fn cmp_match_priority(a: MatchPriority, b: MatchPriority) -> Ordering {
    let cmp_f32 = |left: f32, right: f32, low_is_better: bool| -> Ordering {
        // `sort_by` requires a total order; float epsilon comparisons can violate
        // transitivity. Normalize NaN and use total_cmp for deterministic ordering.
        if low_is_better {
            let lhs = if left.is_nan() { f32::INFINITY } else { left };
            let rhs = if right.is_nan() { f32::INFINITY } else { right };
            rhs.total_cmp(&lhs)
        } else {
            let lhs = if left.is_nan() { f32::NEG_INFINITY } else { left };
            let rhs = if right.is_nan() { f32::NEG_INFINITY } else { right };
            lhs.total_cmp(&rhs)
        }
    };

    let identity_desc_threshold = 0.75;
    let same_identity_signature = a.amount_variance == 0.0
        && b.amount_variance == 0.0
        && a.side_variance == 0.0
        && b.side_variance == 0.0
        && a.description_variance <= identity_desc_threshold
        && b.description_variance <= identity_desc_threshold
        && a.date_days <= 1.0
        && b.date_days <= 1.0;
    if same_identity_signature {
        let date_cmp = cmp_f32(a.date_days, b.date_days, true);
        if date_cmp != Ordering::Equal {
            return date_cmp;
        }
    }

    a.status_rank
        .cmp(&b.status_rank)
        .then_with(|| cmp_f32(a.amount_variance, b.amount_variance, true))
        .then_with(|| cmp_f32(a.description_variance, b.description_variance, true))
        .then_with(|| cmp_f32(a.date_days, b.date_days, true))
        .then_with(|| cmp_f32(a.confidence, b.confidence, false))
        .then_with(|| match (a.balance_variance, b.balance_variance) {
            (Some(av), Some(bv)) => cmp_f32(av, bv, true),
            _ => Ordering::Equal,
        })
}

fn is_candidate_better(candidate: &MatchCandidate, best: &MatchCandidate) -> bool {
    cmp_match_priority(
        MatchPriority::from_candidate(candidate),
        MatchPriority::from_candidate(best),
    ) == Ordering::Greater
}

fn select_best_candidate(
    rec_entry: &Entry,
    existing_targets: &[(usize, Uuid, Entry)],
    matched_indices: &HashSet<usize>,
) -> Option<(usize, Uuid, MatchCandidate)> {
    let mut best_candidate: Option<(usize, Uuid, MatchCandidate)> = None;

    for (target_idx, target_txn_id, target_entry) in existing_targets.iter() {
        if matched_indices.contains(target_idx) {
            continue;
        }

        if let Some(candidate) = evaluate_match_candidate(rec_entry, target_entry) {
            match &best_candidate {
                None => {
                    best_candidate = Some((*target_idx, *target_txn_id, candidate));
                }
                Some((_, _, best_candidate_match)) => {
                    if is_candidate_better(&candidate, best_candidate_match) {
                        best_candidate = Some((*target_idx, *target_txn_id, candidate));
                    }
                }
            }
        }
    }

    best_candidate
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransactionSortOrder {
    OldestFirst,
    NewestFirst,
}

/// Sort imported transactions by account entry date while preserving original order for same-day items.
pub fn sort_transactions_by_account(
    transactions: &mut Vec<Transaction>,
    account_id: Option<Uuid>,
    order: TransactionSortOrder,
) {
    let mut indexed: Vec<(usize, Transaction)> = transactions.drain(..).enumerate().collect();
    indexed.sort_by(|(a_idx, a_txn), (b_idx, b_txn)| {
        let a_date = match account_id {
            Some(id) => a_txn.find_entry_by_account(&id).map(|e| e.date),
            None => a_txn.date(),
        };
        let b_date = match account_id {
            Some(id) => b_txn.find_entry_by_account(&id).map(|e| e.date),
            None => b_txn.date(),
        };

        let date_cmp = match (a_date, b_date) {
            (Some(a), Some(b)) => a.cmp(&b),
            _ => std::cmp::Ordering::Equal,
        };

        let date_cmp = match order {
            TransactionSortOrder::OldestFirst => date_cmp,
            TransactionSortOrder::NewestFirst => date_cmp.reverse(),
        };

        if date_cmp == std::cmp::Ordering::Equal {
            a_idx.cmp(b_idx)
        } else {
            date_cmp
        }
    });

    *transactions = indexed.into_iter().map(|(_, txn)| txn).collect();
}

#[derive(Debug)]
pub struct BooksError {
    pub error: String,
}

impl BooksError {
    pub fn from_str(name: &str) -> BooksError {
        BooksError {
            error: String::from(name),
        }
    }
}

// books tests can be found in ../tests/books_test.rs
