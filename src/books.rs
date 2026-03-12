use std::{collections::HashMap, cmp::Ordering};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use rust_i18n::t;

macro_rules! tr {
    ($($tt:tt)*) => {
        t!($($tt)*).to_string()
    };
}

use crate::account::{Account, Entry, ReconciledStatus, Source, Transaction, TransactionStatus};
use crate::interest::{Interest, calculate_interest_for_accounts};
use crate::schedule::{Modifier, Schedule};
use crate::scheduler::Scheduler;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct Settings {
    pub require_double_entry: bool,
}

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

/// Result for a single transaction in a reconciliation.
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
}

impl Books {
    pub fn generate(&mut self, end_date: NaiveDate) {
        let transactions = self.scheduler.generate(end_date);
        for transaction in transactions.iter() {
            let _ = self.add_transaction(transaction.clone());
        }
        sort_transactions_by_account(&mut self.transactions, None, TransactionSortOrder::OldestFirst);
    }

    pub fn generate_by_schedule(&mut self, end_date: NaiveDate, schedule_id: Uuid) -> Vec<Transaction> {
        let transactions = self.scheduler.generate_by_schedule(end_date, schedule_id);
        for transaction in transactions.iter() {
            let _ = self.add_transaction(transaction.clone());
        }
        sort_transactions_by_account(&mut self.transactions, None, TransactionSortOrder::OldestFirst);
        transactions
    }

    pub fn build_empty(name: &str) -> Books {
        Books{
            id: Uuid::new_v4(),
            name: name.to_string(),
            version: VERSION.to_string(),
            accounts: HashMap::new(),
            scheduler: Scheduler::build_empty(), transactions: Vec::new(),
            interests: HashMap::new(),
            settings: Settings{ require_double_entry: false },
        }
    }

    pub fn with_components(id: Uuid, name: String, version: String, accounts: HashMap<Uuid, Account>, scheduler: Scheduler, transactions: Vec<Transaction>, interests: HashMap<Uuid, Interest>, settings: Settings) -> Books {
        Books {
            id,
            name,
            version,
            accounts,
            scheduler,
            transactions,
            interests,
            settings,
        }
    }

    pub fn add_account(&mut self, account: Account) {
        let mut account = account;
        account.reconciliation_info = None;
        if ! self.accounts.contains_key(&account.id) {
            self.accounts.insert(account.id, account);
        }        
    }

    pub fn update_account(&mut self, account: Account) -> Result<(), BooksError> {
        let existing = self
            .accounts
            .get(&account.id)
            .ok_or_else(|| BooksError { error: tr!("errors.account_not_found", id => account.id) })?;

        let same_reconciliation = match (&account.reconciliation_info, &existing.reconciliation_info) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.date == b.date && a.balance == b.balance && a.transaction_id == b.transaction_id
            }
            _ => false,
        };

        if !same_reconciliation {
            return Err(BooksError { error: tr!("errors.account_reconciliation_info_immutable") });
        }

        if account.account_type != existing.account_type
            && self.transactions.iter().any(|t| t.involves_account(&account.id))
        {
            return Err(BooksError { error: tr!("errors.account_type_immutable_with_transactions") });
        }

        if account.starting_balance != existing.starting_balance
            && existing.reconciliation_info.is_some()
        {
            return Err(BooksError { error: tr!("errors.account_starting_balance_immutable_after_reconciliation") });
        }

        self.accounts.insert(account.id, account);
        Ok(())
    }

    pub fn delete_account(&mut self, id: &Uuid) -> Result<(), BooksError> {
        if !self.accounts.contains_key(id) {
            return Err(BooksError { error: tr!("errors.account_not_found", id => id) });
        }

        if self.transactions.iter().any(|t|t.involves_account(id)) {
            return Err(BooksError { error: tr!("errors.account_cannot_delete_with_transactions", id => id) });
        }

        self.accounts.remove(id);
        Ok(())
    }

    pub fn get_account(&self, id: &Uuid) -> Result<Account, BooksError> {
        self.accounts.get(id).cloned().ok_or(BooksError { error: tr!("errors.account_not_found", id => id) })
    }

    pub fn accounts(&self) -> Vec<Account> {
        let mut accounts_clone: Vec<Account> = Vec::new();
        for a in self.accounts.values() {
            accounts_clone.push(a.clone());
        }

        accounts_clone.sort_by(|a, b| {
            let result = a.account_type.order().cmp(&b.account_type.order());
            if result == Ordering::Equal {
                return a.name.cmp(&b.name)
            }
            return result
        });
        accounts_clone
    }

    pub fn add_transaction(&mut self, transaction: Transaction) -> Result<(), BooksError> {

        if let Some(value) = self.validate_transaction(&transaction) {
            return value;
        }

        self.transactions.push(transaction);
        Ok(())
    }

    fn validate_transaction(&mut self, transaction: &Transaction) -> Option<Result<(), BooksError>> {

        for e in transaction.entries.as_slice() {
            if !self.valid_account_id(Some(e.account_id)) {
                return Some(Err(BooksError{ error: tr!("errors.account_not_found_for_id_colon", id => e.account_id) }))
            }
        }

        if self.settings.require_double_entry && transaction.entries.len() < 2 {
            return Some(Err(BooksError { error: tr!("errors.transaction_requires_two_entries") }))
        } else if transaction.entries.len() < 1 {
            return Some(Err(BooksError { error: tr!("errors.transaction_requires_one_entry") }))
        }

        if !self.valid_account_id(Some(transaction.entries[0].account_id)) {
            return Some(Err(BooksError { error: tr!("errors.invalid_account") }))
        }

        // Check that no account has more than one entry in the transaction.
        let mut account_ids = std::collections::HashSet::new();
        for entry in &transaction.entries {
            if !account_ids.insert(entry.account_id) {
                return Some(Err(BooksError { error: tr!("errors.transaction_single_entry_per_account") }));
            }
        }
        
        // for each original transaction entry that is reconciled or outstanding find the matching transaction entry 
        // if the transaction entry is not the same return error
        if let Some(orginal_transaction) = self.transactions.iter().find(|t| t.id == transaction.id) {
            for original_entry in orginal_transaction.entries.iter() {
                if original_entry.is_reconciled_or_outstanding() {
                    let matching_entry = transaction.entries.iter().find(|e| e.account_id == original_entry.account_id);
                    if matching_entry.is_none() || matching_entry.unwrap() != original_entry {
                        return Some(Err(BooksError { error: tr!("errors.reconciled_entry_immutable") }))
                    }
                }
            }
        }

        // If the trandaction is net new,
        // or the original_transaction has entries that are not flaged as reconciled or outstanding,
        // check that their dates are after their account's reconciliation date
        
        let original_transaction = self.transactions.iter().find(|t| t.id == transaction.id);

        for entry in &transaction.entries {
            // Check if this entry exists in original transaction as reconciled or outstanding
            
            if original_transaction.is_none_or(
                |original_transaction| original_transaction.entries.iter()
                    .any(|e| e.account_id == entry.account_id && ! e.is_reconciled_or_outstanding())) { 
                
                // Check if account exists and has reconciliation info
                if let Some(account) = self.accounts.get(&entry.account_id) {
                    if let Some(reconciliation_info) = &account.reconciliation_info {
                        if reconciliation_info.date > entry.date {
                            return Some(Err(BooksError { error: tr!("errors.transaction_before_reconciliation_date") }));
                        }
                    }
                }
            }
        }
            
        None
    }

    pub fn update_transaction(&mut self, transaction: Transaction) -> Result<(), BooksError> {

        if let Some(value) = self.validate_transaction(&transaction) {
            return value;
        }

        if let Some(index) = self.transactions.iter().position(|t| t.id == transaction.id) {
            let _old = std::mem::replace(&mut self.transactions[index], transaction);
            Ok(())
        } else {
            Err(BooksError { error: tr!("errors.transaction_not_found") })
        }

    }

    fn reconcile_transaction(&mut self, mut transaction: Transaction, account_id: Uuid, status: ReconciledStatus) -> Result<(), BooksError> {

        if let Some(value) = self.validate_transaction(&transaction) {
            return value;
        }
        
        let transaction_id = transaction.id;

        if let Some(entry) = transaction.entries.iter_mut().find(|e| e.account_id == account_id) {
            if entry.reconciled_status.is_none_or(|rs|rs != status) { 
                entry.reconciled_status = Some(status);
                transaction.status = TransactionStatus::Recorded;
                if let Some(index) = self.transactions.iter().position(|t| t.id == transaction_id) {
                    let _old = std::mem::replace(&mut self.transactions[index], transaction);
                } else {
                    return Err(BooksError { error: tr!("errors.transaction_not_found_no_period", id => transaction_id) })
                }
            }
        }

        Ok(())
    }

    pub fn delete_transaction(&mut self, id: &Uuid) -> Result<(), BooksError> {
        if let Some(index) = self.transactions.iter().position(|t| t.id == *id) {
            println!("remove: {:?}", index);

            if let Some(transaction) = self.transactions.get(index) {
                if transaction.entries.iter().any(|e| e.is_reconciled_or_outstanding()) {
                    return Err(BooksError { error: tr!("errors.cannot_delete_reconciled_transaction") });
                }
            }

            self.transactions.remove(index);
            Ok(())
        } else {
            return Err(BooksError { error: tr!("errors.transaction_not_found_with_period", id => id) });
        }
    }

    pub fn transactions(&self) -> &[Transaction] {
        self.transactions.as_slice()
    }

    pub fn transaction(&self, transaction_id: Uuid) ->  Option<Transaction> {
        let matches:Vec<Transaction> = self.transactions.iter()
            .filter(|t|t.id == transaction_id)
            .map(|t| t.clone())
            .collect();

        if matches.len() > 0 {
            return Some(matches[0].clone())
        }

        None
    }

    /// Get a copy of the entries with balances for a given Account.
    pub fn account_entries(&self, account_id: Uuid) -> Result<Vec<Entry>, BooksError> {
        if !self.accounts.contains_key(&account_id) {
            return Err(BooksError { error: tr!("errors.account_not_found_for_id", id => account_id) });
        }

        let mut account_transactions: Vec<Transaction> =
            self.transactions
                .iter()
                .filter(|t|t.involves_account(&account_id))
                .map(|t| t.clone())
                .collect();

        sort_transactions_by_account(&mut account_transactions, Some(account_id), TransactionSortOrder::OldestFirst);
        let account = self.accounts.get(&account_id).unwrap();
        let mut balance = account.starting_balance;
        let mut account_entries: Vec<Entry> = Vec::new();
        account_transactions
            .iter()
            .for_each(|t| t.account_entries(account_id)
                .iter()
                .for_each(|e|{
                    if e.entry_type == account.normal_balance() {
                        balance = balance + e.amount;
                    } else {
                        balance = balance - e.amount;
                    };
                    let mut new_e = e.clone();
                    new_e.set_balance(Some(balance.clone()));
                    account_entries.push(new_e);
                }
                )
            );

        account_entries.sort_by(|a, b| a.date.cmp(&b.date));
        Ok(account_entries)
    }

    /// Get a copy of the transactions with balances for a given Account.
    pub fn account_transactions(&self, account_id: Uuid) -> Result<Vec<Transaction>, BooksError> {
        if !self.accounts.contains_key(&account_id) {
            return Err(BooksError { error: tr!("errors.account_not_found_for_id", id => account_id) });
        }

        let account_transactions: Vec<(usize, Transaction)> =
            self.transactions
                .iter()
                .filter(|t| t.involves_account(&account_id))
                .enumerate()
                .map(|(idx, t)| (idx, t.clone()))
                .collect();
        
        let mut account_transactions: Vec<Transaction> =
            account_transactions.into_iter().map(|(_, t)| t).collect();

        sort_transactions_by_account(&mut account_transactions, Some(account_id), TransactionSortOrder::OldestFirst);        
        let account = self.accounts.get(&account_id).unwrap();
        let mut balance = account.starting_balance;

        for i in 0..account_transactions.len() {
            balance = account_transactions[i].update_balance(balance, account);
        }
        Ok(account_transactions)
    }


    pub fn add_schedule(&mut self, schedule: Schedule) -> Result<(), BooksError> {
        if let Some(value) = self.validate_schedule(&schedule) {
            return value;
        }

        self.scheduler.add_schedule(schedule);
        Ok(())
    }

    fn validate_schedule(&mut self, schedule: &Schedule) -> Option<Result<(), BooksError>> {

        if schedule.entries.len() < 1 {
            return Some(Err(BooksError { error: tr!("errors.schedule_requires_entry") }))
        }

        for e in schedule.entries.iter() {
            if !self.valid_account_id(Some(e.account_id)) {
                return Some(Err(BooksError { error: tr!("errors.invalid_account_with_id", id => e.account_id) }))
            }
        }

        None
    }

    pub fn update_schedule(&mut self, schedule: Schedule) -> Result<(), BooksError> {
        if let Some(value) = self.validate_schedule(&schedule) {
            return value;
        }

        self.scheduler.update_schedule(schedule)
    }

    pub fn delete_schedule(&mut self, id: &Uuid) -> Result<(), BooksError> {
        // Check if schedule exists
        if !self.scheduler.schedules().iter().any(|s| s.id == *id) {
            return Err(BooksError { error: tr!("errors.schedule_not_found", id => id) });
        }

        // Check if any transactions reference this schedule
        if self.transactions.iter().any(|t| {t.source_type == Some(Source::Schedule) && t.source_id == Some(*id)}) {
            return Err(BooksError { error: tr!("errors.schedule_cannot_delete_with_transactions", id => id) });
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

    pub fn transactions_by_schedule(&self, schedule_id: Uuid, status: Option<TransactionStatus>) -> Vec<Transaction> {
        self.transactions
            .iter()
            .filter(|t| {t.source_type == Some(Source::Schedule) && t.source_id == Some(schedule_id)})
            .filter(|t| {
                match status {
                    Some(filter_status) => t.status == filter_status,
                    None => true,
                }
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
            return Err(BooksError { error: tr!("errors.modifier_not_found", id => id) });
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
        if !self.accounts.contains_key(&interest.account_id) {
            return Err(BooksError { error: tr!("errors.account_not_found", id => interest.account_id) });
        }
        
        // Update the account to reference this interest info
        if let Some(account) = self.accounts.get_mut(&interest.account_id) {
            account.interest_id = Some(interest.id);
        }
        
        self.interests.insert(interest.id, interest);
        Ok(())
    }

    pub fn get_interest(&self, interest_id: &Uuid) -> Result<Interest, BooksError> {
        self.interests.get(interest_id)
            .cloned()
            .ok_or(BooksError { error: tr!("errors.interest_info_not_found", id => interest_id) })
    }

    pub fn update_interest(&mut self, interest: Interest) -> Result<(), BooksError> {
        if !self.accounts.contains_key(&interest.account_id) {
            return Err(BooksError { error: tr!("errors.account_not_found", id => interest.account_id) });
        }
        
        // Get the account to check if it has interest info
        let account = self.accounts.get(&interest.account_id)
            .ok_or(BooksError { error: tr!("errors.account_not_found", id => interest.account_id) })?;
        
        if let Some(interest_id) = account.interest_id {
            // Update existing interest info
            self.interests.insert(interest_id, interest);
        } else {
            // Account doesn't have interest info yet, add it
            self.add_interest(interest)?;
        }
        
        Ok(())
    }

    pub fn interests(&self) -> Vec<&Interest> {
        self.interests.values().collect()
    }

    pub fn reset_schedule_last_date(&mut self, schedule_id: Uuid) -> Option<NaiveDate> {
        let mut transactions: Vec<Transaction> = self.transactions
            .iter()
            .filter(|t| {t.source_type == Some(Source::Schedule) && t.source_id == Some(schedule_id)})
            .map(|t| t.clone())
            .collect();
        
        // Sort transactions by date to find the latest one
        sort_transactions_by_account(&mut transactions, None, TransactionSortOrder::OldestFirst);
        
        let new_last = transactions.last().and_then(|t| t.date());
        println!("New last date: {:?}", new_last);
        self.scheduler.update_schedule(Schedule {
            id: schedule_id,
            last_date: new_last,
            ..self.scheduler.get_schedule(schedule_id).unwrap().clone()
        }).unwrap();
        new_last
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
        sort_transactions_by_account(&mut input_txns, Some(account_id), TransactionSortOrder::OldestFirst);

        // 2) Load existing account transactions (with balances) and track matched indices.
        let existing_txns = self.account_transactions(account_id)?;
        let mut matched_indices: Vec<usize> = Vec::new();

        let mut results: Vec<ReconciliationItem> = Vec::with_capacity(input_txns.len() + existing_txns.len());

        for input in input_txns.iter() {
            // 3) Extract the account entry details from the input transaction.
            let entry = input
                .find_entry_by_account(&account_id)
                .expect("transaction involves account");

            let amount = entry.amount;
            let entry_type = entry.entry_type;
            let date = entry.date;
            let description = &entry.description;
            let expected_balance = entry.balance;

            // 4) Try exact match first; if none, try partial/mismatch rules.
            let (status, matched_id) = existing_txns
                .iter()
                .enumerate()
                .find(|(i, existing)| {
                    !matched_indices.contains(i)
                        && existing
                            .find_entry_by_account(&account_id)
                            .map(|e| {
                                (e.date - date).num_days().abs() <= 14
                                    && e.date == date
                                    && e.amount == amount
                                    && e.entry_type == entry_type
                                    && e.balance == expected_balance
                            })
                            .unwrap_or(false)
                })
                .map(|(i, existing)| {
                    matched_indices.push(i);
                    (ReconciliationMatchStatus::Matched, Some(existing.id))
                })
                .or_else(|| {
                    existing_txns.iter().enumerate().find_map(|(i, existing)| {
                        if matched_indices.contains(&i) {
                            return None;
                        }
                        existing.find_entry_by_account(&account_id).and_then(|e| {
                            let within_14_days = (e.date - date).num_days().abs() <= 14;
                            if !within_14_days {
                                return None;
                            }
                            let date_match = (e.date - date).num_days().abs() <= 1;
                            let amount_match = e.amount == amount;
                            let description_match = e.description == *description;
                            let balance_match = e.balance == expected_balance;
                            let other_match_count = [date_match, amount_match, description_match]
                                .into_iter()
                                .filter(|&b| b)
                                .count();
                            if other_match_count >= 2 {
                                matched_indices.push(i);
                                if balance_match {
                                    Some((ReconciliationMatchStatus::PartialMatch, Some(existing.id)))
                                } else {
                                    Some((ReconciliationMatchStatus::Mismatch, Some(existing.id)))
                                }
                            } else {
                                None
                            }
                        })
                    })
                })
                .unwrap_or((ReconciliationMatchStatus::Unmatched, None));

            // 5) Record this input transaction's reconciliation outcome.
            results.push(ReconciliationItem::Reconciliation(ReconciliationResult {
                transaction: input.clone(),
                status,
                balance: expected_balance,
                matched_transaction_id: matched_id,
            }));
        }

        // 6) Add existing transactions to results, splicing matched reconciliation transactions immediately after their targets
        let mut final_results: Vec<ReconciliationItem> = Vec::with_capacity(existing_txns.len() + results.len());
        let mut reconciliation_lookup: std::collections::HashMap<Uuid, Vec<&ReconciliationItem>> = std::collections::HashMap::new();
        
        // Group reconciliation transactions by their matched target ID
        for reconciliation_item in &results {
            if let ReconciliationItem::Reconciliation(recon_result) = reconciliation_item {
                if let Some(matched_id) = recon_result.matched_transaction_id {
                    reconciliation_lookup.entry(matched_id).or_insert_with(Vec::new).push(reconciliation_item);
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
            });
            
            // If there are matches, set the matched_reconciliation_id to the first match's transaction ID
            if let Some(matches) = matched_reconciliations {
                if !matches.is_empty() {
                    if let ReconciliationItem::Reconciliation(recon_result) = matches[0] {
                        if let Some(_target_id) = recon_result.matched_transaction_id {
                            if let ReconciliationItem::Original(ref mut target_result) = original_item {
                                target_result.matched_reconciliation_id = Some(recon_result.transaction.id);
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
        let mut unmatched_by_date: std::collections::HashMap<chrono::NaiveDate, Vec<&ReconciliationItem>> = std::collections::HashMap::new();
        
        for reconciliation_item in &results {
            if let ReconciliationItem::Reconciliation(recon_result) = reconciliation_item {
                if recon_result.matched_transaction_id.is_none() {
                    let entry = recon_result.transaction.find_entry_by_account(&account_id)
                        .expect("reconciliation transaction involves account");
                    unmatched_by_date.entry(entry.date).or_insert_with(Vec::new).push(reconciliation_item);
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
                        recon.transaction.find_entry_by_account(&account_id)
                            .expect("reconciliation transaction involves account").date
                    }
                    ReconciliationItem::Original(target) => {
                        target.transaction.find_entry_by_account(&account_id)
                            .expect("target transaction involves account").date
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

        // 7) If balances realign later (and no Unmatched in between), treat earlier Mismatch as PartialMatch.
        let mut mismatched_indices: Vec<usize> = Vec::new();
        for i in 0..final_results.len() {
            match final_results[i].status() {
                ReconciliationMatchStatus::Unmatched => {
                    mismatched_indices.clear();
                }
                ReconciliationMatchStatus::Mismatch => {
                    mismatched_indices.push(i);
                }
                ReconciliationMatchStatus::Matched | ReconciliationMatchStatus::PartialMatch => {
                    for idx in mismatched_indices.drain(..) {
                        final_results[idx].set_status(ReconciliationMatchStatus::PartialMatch);
                    }
                }
            }
        }

        Ok(final_results)
    }

    
    pub fn reconcile_account_transactions(&mut self, account_id: Uuid, transaction_ids: Vec<Uuid>) -> Result<(), BooksError> {
        println!("Reconciling account transactions for account {} transactions: {:?}", account_id, transaction_ids);
        if !self.accounts.contains_key(&account_id) {
            return Err(BooksError { error: tr!("errors.account_not_found_for_id", id => account_id) });
        }

        let mut account_transactions = self.account_transactions(account_id)?;
        let mut new_recon_transaction: Option<Transaction> = None;
        // set the last index to the account reconciliation_info transaction_id index   
        let mut last_index: Option<usize> = self.accounts.get(&account_id).unwrap().reconciliation_info.as_ref().map(|info| info.transaction_id).map(|id| account_transactions.iter().position(|t| t.id == id).unwrap());
        let mut first_index: Option<usize> = None;
        
        // Reconcile each transaction.
        for transaction_id in transaction_ids {

            let idx = account_transactions.iter().position(|t| t.id == transaction_id).ok_or_else(|| {
                BooksError { error: tr!("errors.transaction_not_found_for_account", transaction_id => transaction_id, account_id => account_id) }
            })?;
            
            let transaction = account_transactions.iter_mut().find(|t| t.id == transaction_id).ok_or_else(|| {
                BooksError { error: tr!("errors.transaction_not_found_for_account", transaction_id => transaction_id, account_id => account_id) }
            })?;
            
            self.reconcile_transaction(transaction.clone(), account_id, ReconciledStatus::Reconciled)?;
            
            if last_index.is_none_or(|li|li < idx) {
                last_index = Some(idx);
                new_recon_transaction = Some(transaction.clone())
            }

            if first_index.is_none_or(|fi|fi > idx) {
                first_index = Some(idx);
            }
        }
        
        // Flag any now outstanding transactions before the first transaction.
        if let Some(first_index) = first_index {
            for earlier_transaction in account_transactions.iter_mut().take(first_index)
                    .filter(|t|t.find_entry_by_account(&account_id)
                    .is_some_and(|e|e.reconciled_status.is_none())) {                    
                self.reconcile_transaction(earlier_transaction.clone(), account_id, ReconciledStatus::Outstanding)?;
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


    pub fn rollback_reconciliation(&mut self, account_id: Uuid, to_date: NaiveDate) -> Result<(), BooksError> {
        if !self.accounts.contains_key(&account_id) {
            return Err(BooksError { error: tr!("errors.account_not_found_for_id", id => account_id) });
        }

        let account_transactions = self.account_transactions(account_id)?;
        let mut last_reconciled_index: Option<usize> = None;
        let mut last_reconciled_info: Option<crate::account::ReconciliationInfo> = None;

        for (idx, transaction) in account_transactions.iter().enumerate() {
            if let Some(entry) = transaction.find_entry_by_account(&account_id) {
                if entry.is_reconciled() && entry.date <= to_date {
                    last_reconciled_index = Some(idx);
                    let balance = entry.balance.ok_or_else(|| {
                        BooksError { error: tr!("errors.reconciliation_rollback_requires_balances") }
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
                if let Some(existing) = self.transactions.iter_mut().find(|t| t.id == transaction.id) {
                    if let Some(entry) = existing.entries.iter_mut().find(|e| e.account_id == account_id) {
                        entry.reconciled_status = None;
                    }
                }
            }
        }

        Ok(())
    }
  
    fn valid_account_id(&self, id: Option<Uuid>) -> bool {
        match id {
            Some(k) => return self.accounts.contains_key(&k),
            None => return true
        }
    }

    pub fn run_checks_and_update(&mut self, projection_date: NaiveDate) -> Result<(), BooksError>{
        println!("Running checks - projecting to: {}", projection_date);
        let interest_accounts = self.accounts.values().filter(|a| a.interest_id.is_some()).cloned().collect();
        calculate_interest_for_accounts(self, interest_accounts, projection_date);        
        println!("Checks completed ✅");
        Ok(())
    }
    
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
        BooksError { error: String::from(name) }
    }
}

#[cfg(test)]

mod tests {
    use rust_decimal::Decimal;
    use uuid::Uuid;
    use chrono::{NaiveDate};
    use rust_decimal_macros::dec;
    use rust_i18n::t;
    use crate::account::*;
    use crate::books::{BooksError, ReconciliationItem, ReconciliationMatchStatus};
    use crate::schedule::{Schedule, ScheduleEnum, ScheduleEntry};
    use super::{sort_transactions_by_account, TransactionSortOrder};

    use super::Books;

    #[test]
    fn test_add_account(){
        let a = Account::create_new("test account", AccountType::Liability);
        let id1 = a.id;
        let mut b = Books::build_empty("My Books");
        b.add_account(a);

        let a2 = &b.accounts()[0];
        assert_eq!(id1, a2.id);
    }

    #[test]
    fn test_add_account_resets_reconciliation_info() {
        let mut a = Account::create_new("test account", AccountType::Liability);
        a.reconciliation_info = Some(ReconciliationInfo {
            date: NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
            balance: dec!(100),
            transaction_id: Uuid::new_v4(),
        });
        let mut b = Books::build_empty("My Books");
        b.add_account(a);

        let a2 = &b.accounts()[0];
        assert!(a2.reconciliation_info.is_none());
    }

    #[test]
    fn test_delete_account(){
        let (mut books, id1, id2) = setup_books();
        let _result = books.delete_account(&id1);
        assert!(matches!((), _result));
        assert!(books.accounts.get(&id1).is_none());
        assert!(books.accounts.get(&id2).is_some());
    }

    #[test]
    fn test_update_account_allows_non_reconciliation_changes() {
        let (mut books, id1, _id2) = setup_books();
        let mut account = books.accounts.get(&id1).unwrap().clone();
        account.name = "updated name".to_string();

        let result = books.update_account(account);
        assert!(result.is_ok());

        let updated = books.accounts.get(&id1).unwrap();
        assert_eq!("updated name", updated.name);
    }

    #[test]
    fn test_update_account_rejects_reconciliation_changes() {
        let (mut books, id1, _id2) = setup_books();
        let mut account = books.accounts.get(&id1).unwrap().clone();
        account.reconciliation_info = Some(ReconciliationInfo {
            date: NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
            balance: dec!(100),
            transaction_id: Uuid::new_v4(),
        });

        let result = books.update_account(account);
        assert!(result.is_err());
    }

    #[test]
    fn test_update_account_rejects_account_type_change_with_transactions() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        books.add_transaction(t1).unwrap();

        let mut account = books.accounts.get(&id1).unwrap().clone();
        account.account_type = AccountType::Expense;

        let result = books.update_account(account);
        assert!(result.is_err());
    }

    #[test]
    fn test_update_account_rejects_starting_balance_change_when_reconciled() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        books.add_transaction(t1.clone()).unwrap();
        books.reconcile_account_transactions(id1, vec![t1.id]).unwrap();

        let mut account = books.accounts.get(&id1).unwrap().clone();
        account.starting_balance = dec!(500);

        let result = books.update_account(account);
        assert!(result.is_err());
    }

    #[test]
    fn test_cannot_delete_account_with_transactions(){
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction(None, Some(id1));
        books.add_transaction(t1).unwrap();
        let result = books.delete_account(&id1);
        assert_eq!(tr!("errors.account_cannot_delete_with_transactions", id => id1), result.err().unwrap().error);
        assert!(books.accounts.get(&id1).is_some());
        assert!(books.accounts.get(&id2).is_some());
    }

    #[test]
    fn test_cannot_delete_with_invalid_account_id(){
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction(None, Some(id1));
        books.add_transaction(t1).unwrap();
        let id = &Uuid::new_v4();
        let result = books.delete_account(id);
        assert_eq!(tr!("errors.account_not_found", id => id), result.err().unwrap().error);
        assert!(books.accounts.get(&id1).is_some());
        assert!(books.accounts.get(&id2).is_some());
    }

    #[test]
    fn test_add_transaction() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction(Some(id1), Some(id2));
        let t1_id = t1.id;
        books.add_transaction(t1).unwrap();
        let t1_2 = &books.transactions()[0];
        assert_eq!(t1_id, t1_2.id);
    }

    #[test]
    fn test_double_entry_required() {
        let (mut books, id1, id2) = setup_books();
        books.settings.require_double_entry = true;
        assert_eq!(0, books.transactions.len());
        let mut t1 = build_transaction(Some(id1), Some(id2));
        t1.entries.pop();
        let result = books.add_transaction(t1);
        assert_eq!(tr!("errors.transaction_requires_two_entries"), result.err().unwrap().error);
        assert_eq!(0, books.transactions.len());
    }

    #[test]
    fn test_at_least_one_entry_required() {
        let (mut books, id1, id2) = setup_books();
        assert_eq!(0, books.transactions.len());
        let mut t1 = build_transaction(Some(id1), Some(id2));
        t1.entries.pop();
        t1.entries.pop();
        let result = books.add_transaction(t1);
        assert_eq!(tr!("errors.transaction_requires_one_entry"), result.err().unwrap().error);
        assert_eq!(0, books.transactions.len());
    }


    #[test]
    fn test_add_transaction_no_cr_account() {
        let (mut books, id1, _) = setup_books();
        let t1 = build_transaction(Some(id1), None);
        let t1_id = t1.id;
        books.add_transaction(t1).unwrap();
        let t1_2 = &books.transactions()[0];
        assert_eq!(t1_id, t1_2.id);
    }

    #[test]
    fn test_add_transaction_no_dr_account() {
        let (mut books, _, id2) = setup_books();
        let t1 = build_transaction(None, Some(id2));
        let t1_id = t1.id;
        books.add_transaction(t1).unwrap();
        let t1_2 = &books.transactions()[0];
        assert_eq!(t1_id, t1_2.id);
    }

    #[test]
    fn test_add_transaction_invalid_dr_account() {
        let (mut books, _, id2) = setup_books();
        let t1 = build_transaction(Some(Uuid::new_v4()), Some(id2));
        let _result = books.add_transaction(t1);
        let expected: Result<(), BooksError> = Err(BooksError { error: tr!("errors.invalid_cr_account") });
        assert!(matches!(expected, _result));
        assert_eq!(0, (&books.transactions()).len());
    }

    #[test]
    fn test_add_transaction_invalid_cr_account() {
        let (mut books, id1, _) = setup_books();
        let t1 = build_transaction(Some(id1), Some(Uuid::new_v4()));
        let _result = books.add_transaction(t1);
        let expected: Result<(), BooksError> = Err(BooksError { error: tr!("errors.invalid_cr_account") });
        assert!(matches!(expected, _result));
        assert_eq!(0, (&books.transactions()).len());
    }

    #[test]
    fn test_add_transaction_before_reconciliation_date_rejected() {
        let (mut books, account1_id, account2_id) = setup_books();
        
        // Add a transaction and reconcile the account
        let reconciliation_date = NaiveDate::from_ymd_opt(2022, 6, 4).unwrap();
        let t1 = build_transaction_with_date(Some(account1_id), Some(account2_id), reconciliation_date);
        books.add_transaction(t1.clone()).unwrap();
        books.reconcile_account_transactions(account1_id, vec![t1.id]).unwrap();

        println!("Reconciled account {:?}", books.accounts.get(&account1_id).unwrap());
        
        // Try to add a transaction before the reconciliation date - should be rejected
        let early_date = NaiveDate::from_ymd_opt(2022, 6, 1).unwrap();
        let t2 = build_transaction_with_date(Some(account1_id), Some(account2_id), early_date);
        let result = books.add_transaction(t2);
        assert!(result.is_err());
        assert_eq!(tr!("errors.transaction_before_reconciliation_date"), result.err().unwrap().error);
    }

    #[test]
    fn test_add_transaction_after_reconciliation_date_allowed() {
        let (mut books, id1, id2) = setup_books();
        
        // Add a transaction and reconcile the account
        let reconciliation_date = NaiveDate::from_ymd_opt(2022, 6, 4).unwrap();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), reconciliation_date);
        books.add_transaction(t1.clone()).unwrap();
        books.reconcile_account_transactions(id1, vec![t1.id]).unwrap();
        
        // Add a transaction after the reconciliation date - should be allowed
        let later_date = NaiveDate::from_ymd_opt(2022, 6, 10).unwrap();
        let t2 = build_transaction_with_date(Some(id1), Some(id2), later_date);
        let result = books.add_transaction(t2);
        assert!(result.is_ok());
        assert_eq!(2, books.transactions.len());
    }

    #[test]
    fn test_add_transaction_no_account() {
        let (mut books, _id1, _id2) = setup_books();
        let t1 = build_transaction(None, None);
        let _result = books.add_transaction(t1);
        let expected: Result<(), BooksError> = Err(BooksError { error: tr!("errors.transaction_requires_one_account") });
        assert!(matches!(expected, _result));
        assert_eq!(0, (&books.transactions()).len());
    }

    #[test]
    fn test_delete_transaction() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction(Some(id1), Some(id2));
        let t1_id = t1.id;
        books.add_transaction(t1).unwrap();

        let _result = books.delete_transaction(&t1_id);
        let expected: Result<(), BooksError> = Ok(());
        assert!(matches!(expected, _result));
        assert_eq!(0, books.transactions.len());
    }

    #[test]
    fn test_delete_invalid_transaction() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction(Some(id1), Some(id2));
        books.add_transaction(t1).unwrap();

        let id = &Uuid::new_v4();
        let result = books.delete_transaction(&id);
        assert_eq!(tr!("errors.transaction_not_found_with_period", id => id), result.err().unwrap().error);
        assert_eq!(1, books.transactions.len());
    }


    #[test]
    fn test_account_entries() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let t2 = build_transaction_with_date(None, Some(id2),NaiveDate::from_ymd_opt(2022, 6, 5).unwrap());
        let t3 = build_transaction_with_date(Some(id1), None, NaiveDate::from_ymd_opt(2022, 7, 1).unwrap());
        let t4 = build_transaction_with_date(Some(id2), Some(id1), NaiveDate::from_ymd_opt(2022, 7, 2).unwrap());
        let t1a1e1 = &t1.account_entries(id1)[0];
        let t3a1e3 = &t3.account_entries(id1)[0];
        let t4a1e4 = &t4.account_entries(id1)[0];
        let t1a2e1 = &t1.account_entries(id2)[0];
        let t2a2e1 = &t2.account_entries(id2)[0];
        let t4a2e1 = &t4.account_entries(id2)[0];
        books.add_transaction(t1).unwrap();
        books.add_transaction(t2).unwrap();
        books.add_transaction(t3).unwrap();
        books.add_transaction(t4).unwrap();
        let a1_entries = books.account_entries(id1).unwrap();
        assert_eq!(3, a1_entries.len());

        let entry1 = &a1_entries[0];
        assert_eq!(t1a1e1.id, entry1.id);
        assert_eq!(dec!(10000), entry1.balance.unwrap());

        let entry2 = &a1_entries[1];
        assert_eq!(t3a1e3.id, entry2.id);
        assert_eq!(dec!(20000), entry2.balance.unwrap());

        let entry3 = &a1_entries[2];
        assert_eq!(t4a1e4.id, entry3.id);
        assert_eq!(dec!(10000), entry3.balance.unwrap());

        let a2_entries = books.account_entries(id2).unwrap();
        assert_eq!(3, a2_entries.len());

        let entry21 = &a2_entries[0];
        assert_eq!(t1a2e1.id, entry21.id);
        assert_eq!(dec!(-10000), entry21.balance.unwrap());

        let entry22 = &a2_entries[1];
        assert_eq!(t2a2e1.id, entry22.id);
        assert_eq!(dec!(-20000), entry22.balance.unwrap());

        let entry23 = &a2_entries[2];
        assert_eq!(t4a2e1.id, entry23.id);
        assert_eq!(dec!(-10000), entry23.balance.unwrap());


    }

    #[test]
    fn test_account_transactions_same_day_preserves_order() {
        let (mut books, id1, id2) = setup_books();
        let date = NaiveDate::from_ymd_opt(2022, 6, 4).unwrap();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), date);
        let t2 = build_transaction_with_date(Some(id1), Some(id2), date);
        let t3 = build_transaction_with_date(Some(id1), Some(id2), date);

        let t1_id = t1.id;
        let t2_id = t2.id;
        let t3_id = t3.id;

        books.add_transaction(t1).unwrap();
        books.add_transaction(t2).unwrap();
        books.add_transaction(t3).unwrap();

        let a2_transactions = books.account_transactions(id2).unwrap();
        assert_eq!(3, a2_transactions.len());
        assert_eq!(t1_id, a2_transactions[0].id);
        assert_eq!(t2_id, a2_transactions[1].id);
        assert_eq!(t3_id, a2_transactions[2].id);
    }

    #[test]
    fn test_sort_transactions_global_uses_transaction_date() {
        let (_books, id1, id2) = setup_books();
        let d1 = NaiveDate::from_ymd_opt(2022, 6, 4).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2022, 6, 5).unwrap();

        let t1 = build_transaction_with_date(Some(id1), Some(id2), d2);
        let t2 = build_transaction_with_date(Some(id1), Some(id2), d1);
        let t3 = build_transaction_with_date(Some(id1), Some(id2), d1);

        let t1_id = t1.id;
        let t2_id = t2.id;
        let t3_id = t3.id;

        let mut transactions = vec![t1, t2, t3];
        sort_transactions_by_account(&mut transactions, None, TransactionSortOrder::OldestFirst);

        assert_eq!(t2_id, transactions[0].id);
        assert_eq!(t3_id, transactions[1].id);
        assert_eq!(t1_id, transactions[2].id);
    }

    #[test]
    fn test_sort_transactions_global_uses_transaction_date_newest_first() {
        let (_books, id1, id2) = setup_books();
        let d1 = NaiveDate::from_ymd_opt(2022, 6, 4).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2022, 6, 5).unwrap();

        let t1 = build_transaction_with_date(Some(id1), Some(id2), d2);
        let t2 = build_transaction_with_date(Some(id1), Some(id2), d1);
        let t3 = build_transaction_with_date(Some(id1), Some(id2), d1);

        let t1_id = t1.id;
        let t2_id = t2.id;
        let t3_id = t3.id;

        let mut transactions = vec![t1, t2, t3];
        sort_transactions_by_account(&mut transactions, None, TransactionSortOrder::NewestFirst);

        assert_eq!(t1_id, transactions[0].id);
        assert_eq!(t2_id, transactions[1].id);
        assert_eq!(t3_id, transactions[2].id);
    }
    
    #[test]
    fn test_account_transaction() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let t1a1e1 = &t1.account_entries(id1)[0];
        let _t1a2e1 = &t1.account_entries(id2)[0];
        books.add_transaction(t1).unwrap();
        let a1_entries = books.account_transactions(id1).unwrap();
        assert_eq!(1, a1_entries.len());

        let entry1 = &a1_entries[0].account_entries(id1)[0];
        assert_eq!(t1a1e1.id, entry1.id);
        assert_eq!(dec!(10000), entry1.balance.unwrap());
    }

    #[test]
    fn test_reconcile_matched_and_unmatched() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let t2 = build_transaction_with_date(None, Some(id2), NaiveDate::from_ymd_opt(2022, 6, 5).unwrap());
        let t3 = build_transaction_with_date(Some(id1), None, NaiveDate::from_ymd_opt(2022, 7, 1).unwrap());
        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();
        books.add_transaction(t3.clone()).unwrap();

        // Simulate statement import: same date/amount/accounts as books, but different IDs
        let mut statement_t1 = clone_transaction_for_reconcile(&t1);
        for e in &mut statement_t1.entries {
            if e.account_id == id2 {
                e.balance = Some(dec!(-10000));
                break;
            }
        }
        let mut statement_t2 = clone_transaction_for_reconcile(&t2);
        for e in &mut statement_t2.entries {
            if e.account_id == id2 {
                e.balance = Some(dec!(-20000));
                break;
            }
        }
        // Different date, amount, description -> no match (not even partial)
        let mut statement_t3_unmatched = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 7, 15).unwrap(),
        );
        for e in &mut statement_t3_unmatched.entries {
            if e.account_id == id2 {
                e.amount = dec!(5000);
                e.description = "unmatched tx".to_string();
                break;
            }
        }

        let statement_t1_id = statement_t1.id;
        let statement_t2_id = statement_t2.id;
        let statement_t3_unmatched_id = statement_t3_unmatched.id;
        let to_reconcile = vec![statement_t1, statement_t2, statement_t3_unmatched];
        let results = books.prepare_reconciliation(id2, to_reconcile).unwrap();

        assert_eq!(5, results.len());

        // Existing transactions with matched reconciliations spliced after.
        match &results[0] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.transaction.id, t1.id);
                assert_eq!(target.status, ReconciliationMatchStatus::Matched);
                assert_eq!(target.matched_reconciliation_id, Some(statement_t1_id));
            }
            _ => panic!("expected original transaction"),
        }
        match &results[1] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.transaction.id, statement_t1_id);
                assert_eq!(recon.status, ReconciliationMatchStatus::Matched);
                assert_eq!(recon.matched_transaction_id, Some(t1.id));
                assert_eq!(recon.balance, Some(dec!(-10000)));
            }
            _ => panic!("expected reconciliation transaction"),
        }

        match &results[2] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.transaction.id, t2.id);
                assert_eq!(target.status, ReconciliationMatchStatus::Matched);
                assert_eq!(target.matched_reconciliation_id, Some(statement_t2_id));
            }
            _ => panic!("expected original transaction"),
        }
        match &results[3] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.transaction.id, statement_t2_id);
                assert_eq!(recon.status, ReconciliationMatchStatus::Matched);
                assert_eq!(recon.matched_transaction_id, Some(t2.id));
                assert_eq!(recon.balance, Some(dec!(-20000)));
            }
            _ => panic!("expected reconciliation transaction"),
        }

        match &results[4] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.transaction.id, statement_t3_unmatched_id);
                assert_eq!(recon.status, ReconciliationMatchStatus::Unmatched);
                assert_eq!(recon.matched_transaction_id, None);
                assert_eq!(recon.balance, None);
            }
            _ => panic!("expected reconciliation transaction"),
        }
    }

    #[test]
    fn test_reconcile_account_sets_info_and_marks_entries_correctly() {
        let (mut books, account_id1, account_id2) = setup_books();
        let t0 = build_transaction_with_date(Some(account_id1), Some(account_id2), NaiveDate::from_ymd_opt(2022, 6, 3).unwrap());
        let t1 = build_transaction_with_date(Some(account_id1), Some(account_id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let t2 = build_transaction_with_date(Some(account_id1), Some(account_id2), NaiveDate::from_ymd_opt(2022, 6, 5).unwrap());
        let t3 = build_transaction_with_date(Some(account_id1), Some(account_id2), NaiveDate::from_ymd_opt(2022, 6, 6).unwrap());

        books.add_transaction(t0.clone()).unwrap();
        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();
        books.add_transaction(t3.clone()).unwrap();

        // reconcile earliest transaction first to check it does not change with later reconciliations
        books.reconcile_account_transactions(account_id1, vec![t0.id]).unwrap();
        books.reconcile_account_transactions(account_id1, vec![t2.id]).unwrap();

        let account = books.accounts.get(&account_id1).unwrap();
        let info = account.reconciliation_info.as_ref().unwrap();
        assert_eq!(t2.id, info.transaction_id);
        assert_eq!(t2.entries[0].date, info.date);

        let t0_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t0.id)

            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();

        let t1_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t1.id)
            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();
        let t2_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t2.id)
            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();
        let t3_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t3.id)
            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();

        assert_eq!( Some(ReconciledStatus::Reconciled), t0_entry.reconciled_status);
        assert_eq!( Some(ReconciledStatus::Outstanding), t1_entry.reconciled_status);
        assert_eq!( Some(ReconciledStatus::Reconciled), t2_entry.reconciled_status);
        assert_eq!( None, t3_entry.reconciled_status);
    }

    #[test]
    fn test_reconcile_account_no_op_when_earlier_or_already_reconciled() {
        let (mut books, account_id1, account_id2) = setup_books();
        let t1 = build_transaction_with_date(Some(account_id1), Some(account_id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let t2 = build_transaction_with_date(Some(account_id1), Some(account_id2), NaiveDate::from_ymd_opt(2022, 6, 5).unwrap());
        let t3 = build_transaction_with_date(Some(account_id1), Some(account_id2), NaiveDate::from_ymd_opt(2022, 6, 6).unwrap());

        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();
        books.add_transaction(t3.clone()).unwrap();

        books.reconcile_account_transactions(account_id1, vec![t2.id]).unwrap();
        let info = books.accounts.get(&account_id1).unwrap().reconciliation_info.as_ref().unwrap();
        assert_eq!(t2.id, info.transaction_id);

        books.reconcile_account_transactions(account_id1, vec![t1.id]).unwrap();
        let info_after_earlier = books.accounts.get(&account_id1).unwrap().reconciliation_info.as_ref().unwrap();
        assert_eq!(t2.id, info_after_earlier.transaction_id);

        books.reconcile_account_transactions(account_id1, vec![t2.id]).unwrap();
        let info_after_reconcile_again = books.accounts.get(&account_id1).unwrap().reconciliation_info.as_ref().unwrap();
        assert_eq!(t2.id, info_after_reconcile_again.transaction_id);
    }

    #[test]
    fn test_reconcile_account_no_op_when_same_day_earlier_in_order() {
        let (mut books, id1, id2) = setup_books();
        let date = NaiveDate::from_ymd_opt(2022, 6, 4).unwrap();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), date);
        let t2 = build_transaction_with_date(Some(id1), Some(id2), date);
        let t3 = build_transaction_with_date(Some(id1), Some(id2), date);

        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();
        books.add_transaction(t3.clone()).unwrap();

        books.reconcile_account_transactions(id1, vec![t2.id]).unwrap();
        let info = books.accounts.get(&id1).unwrap().reconciliation_info.as_ref().unwrap();
        assert_eq!(t2.id, info.transaction_id);

        books.reconcile_account_transactions(id1, vec![t1.id]).unwrap();
        let info_after_earlier = books.accounts.get(&id1).unwrap().reconciliation_info.as_ref().unwrap();
        assert_eq!(t2.id, info_after_earlier.transaction_id);
    }

    #[test]
    fn test_reconcile_account_advances_when_same_day_later_in_order() {
        let (mut books, id1, id2) = setup_books();
        let date = NaiveDate::from_ymd_opt(2022, 6, 4).unwrap();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), date);
        let t2 = build_transaction_with_date(Some(id1), Some(id2), date);
        let t3 = build_transaction_with_date(Some(id1), Some(id2), date);

        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();
        books.add_transaction(t3.clone()).unwrap();

        books.reconcile_account_transactions(id1, vec![t2.id]).unwrap();
        let info = books.accounts.get(&id1).unwrap().reconciliation_info.as_ref().unwrap();
        assert_eq!(t2.id, info.transaction_id);

        books.reconcile_account_transactions(id1, vec![t3.id]).unwrap();
        let info_after_later = books.accounts.get(&id1).unwrap().reconciliation_info.as_ref().unwrap();
        assert_eq!(t3.id, info_after_later.transaction_id);
    }

    #[test]
    fn test_rollback_reconciliation_resets_to_last_reconciled_before_date() {
        let (mut books, account_id1, account_id2) = setup_books();
        let t1 = build_transaction_with_date(Some(account_id1), Some(account_id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let t2 = build_transaction_with_date(Some(account_id1), Some(account_id2), NaiveDate::from_ymd_opt(2022, 6, 5).unwrap());
        let t3 = build_transaction_with_date(Some(account_id1), Some(account_id2), NaiveDate::from_ymd_opt(2022, 6, 6).unwrap());

        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();
        books.add_transaction(t3.clone()).unwrap();

        books.reconcile_account_transactions(account_id1, vec![t1.id, t2.id]).unwrap();
        books.rollback_reconciliation(account_id1, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap()).unwrap();

        let account = books.accounts.get(&account_id1).unwrap();
        let info = account.reconciliation_info.as_ref().unwrap();
        assert_eq!(t1.id, info.transaction_id);
        assert_eq!(t1.entries[0].date, info.date);

        let t1_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t1.id)
            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();
        let t2_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t2.id)
            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();
        let t3_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t3.id)
            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();

        assert_eq!(Some(ReconciledStatus::Reconciled), t1_entry.reconciled_status);
        assert_eq!(None, t2_entry.reconciled_status);
        assert_eq!(None, t3_entry.reconciled_status);
    }

    #[test]
    fn test_rollback_reconciliation_clears_all_when_no_reconciled_before_date() {
        let (mut books, account_id1, account_id2) = setup_books();
        let t1 = build_transaction_with_date(Some(account_id1), Some(account_id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let t2 = build_transaction_with_date(Some(account_id1), Some(account_id2), NaiveDate::from_ymd_opt(2022, 6, 5).unwrap());

        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();

        books.reconcile_account_transactions(account_id1, vec![t1.id, t2.id]).unwrap();
        books.rollback_reconciliation(account_id1, NaiveDate::from_ymd_opt(2022, 6, 3).unwrap()).unwrap();

        let account = books.accounts.get(&account_id1).unwrap();
        assert!(account.reconciliation_info.is_none());

        let t1_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t1.id)
            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();
        let t2_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t2.id)
            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();

        assert_eq!(None, t1_entry.reconciled_status);
        assert_eq!(None, t2_entry.reconciled_status);
    }

    #[test]
    fn test_reconcile_balances() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        books.add_transaction(t1.clone()).unwrap();

        let mut statement_t1 = clone_transaction_for_reconcile(&t1);
        for e in &mut statement_t1.entries {
            if e.account_id == id2 {
                e.balance = Some(dec!(-10000));
                break;
            }
        }
        let results = books.prepare_reconciliation(id2, vec![statement_t1]).unwrap();

        assert_eq!(2, results.len());
        match &results[1] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.balance, Some(dec!(-10000)));
            }
            _ => panic!("expected reconciliation transaction"),
        }
    }

    #[test]
    fn test_reconcile_partial_match() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        books.add_transaction(t1.clone()).unwrap();

        // Date within ±1 day + same amount + same balance, but different description -> PartialMatch
        let mut partial_t1 = clone_transaction_for_reconcile(&t1);
        for e in &mut partial_t1.entries {
            if e.account_id == id2 {
                e.date = NaiveDate::from_ymd_opt(2022, 6, 5).unwrap();
                e.description = "adjusted description".to_string();
                e.balance = Some(dec!(-10000));
                break;
            }
        }

        let results = books.prepare_reconciliation(id2, vec![partial_t1]).unwrap();
        assert_eq!(2, results.len());
        match &results[0] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.status, ReconciliationMatchStatus::PartialMatch);
            }
            _ => panic!("expected original transaction"),
        }
        match &results[1] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.status, ReconciliationMatchStatus::PartialMatch);
                assert_eq!(recon.matched_transaction_id, Some(t1.id));
            }
            _ => panic!("expected reconciliation transaction"),
        }
    }

    #[test]
    fn test_reconcile_partial_match_date_variance() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        books.add_transaction(t1.clone()).unwrap();

        // One day after book entry, same amount and description -> date within ±1 day, so 2 of 3 = PartialMatch
        let mut next_day = clone_transaction_for_reconcile(&t1);
        for e in &mut next_day.entries {
            if e.account_id == id2 {
                e.date = NaiveDate::from_ymd_opt(2022, 6, 5).unwrap();
                e.balance = Some(dec!(-10000));
                break;
            }
        }

        let results = books.prepare_reconciliation(id2, vec![next_day]).unwrap();
        assert_eq!(2, results.len());
        match &results[0] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.status, ReconciliationMatchStatus::PartialMatch);
            }
            _ => panic!("expected original transaction"),
        }
        match &results[1] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.status, ReconciliationMatchStatus::PartialMatch);
                assert_eq!(recon.matched_transaction_id, Some(t1.id));
            }
            _ => panic!("expected reconciliation transaction"),
        }
    }

    #[test]
    fn test_reconcile_unmatched_when_more_than_14_days_apart() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        books.add_transaction(t1.clone()).unwrap();

        let mut statement_t1 = clone_transaction_for_reconcile(&t1);
        for e in &mut statement_t1.entries {
            if e.account_id == id2 {
                e.date = NaiveDate::from_ymd_opt(2022, 6, 20).unwrap();
                e.balance = Some(dec!(-10000));
                break;
            }
        }

        let results = books.prepare_reconciliation(id2, vec![statement_t1]).unwrap();
        assert_eq!(2, results.len());
        match &results[0] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.status, ReconciliationMatchStatus::Unmatched);
                assert_eq!(target.matched_reconciliation_id, None);
            }
            _ => panic!("expected original transaction"),
        }
        match &results[1] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.status, ReconciliationMatchStatus::Unmatched);
                assert_eq!(recon.matched_transaction_id, None);
            }
            _ => panic!("expected reconciliation transaction"),
        }
    }

    #[test]
    fn test_reconcile_mismatch_balance() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        books.add_transaction(t1.clone()).unwrap();

        let mut statement_t1 = clone_transaction_for_reconcile(&t1);
        for e in &mut statement_t1.entries {
            if e.account_id == id2 {
                e.balance = Some(dec!(-9000));
                break;
            }
        }

        let statement_t1_id = statement_t1.id;
        let results = books.prepare_reconciliation(id2, vec![statement_t1]).unwrap();

        assert_eq!(2, results.len());
        match &results[0] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.status, ReconciliationMatchStatus::Mismatch);
                assert_eq!(target.matched_reconciliation_id, Some(statement_t1_id));
            }
            _ => panic!("expected original transaction"),
        }
        match &results[1] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.status, ReconciliationMatchStatus::Mismatch);
                assert_eq!(recon.matched_transaction_id, Some(t1.id));
            }
            _ => panic!("expected reconciliation transaction"),
        }
    }

    #[test]
    fn test_reconcile_mismatch_promoted_when_balances_realign() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let t2 = build_transaction_with_date(None, Some(id2), NaiveDate::from_ymd_opt(2022, 6, 5).unwrap());
        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();

        let mut statement_t1 = clone_transaction_for_reconcile(&t1);
        for e in &mut statement_t1.entries {
            if e.account_id == id2 {
                e.balance = Some(dec!(-9000));
                break;
            }
        }
        let mut statement_t2 = clone_transaction_for_reconcile(&t2);
        for e in &mut statement_t2.entries {
            if e.account_id == id2 {
                e.balance = Some(dec!(-20000));
                break;
            }
        }

        let results = books
            .prepare_reconciliation(id2, vec![statement_t1, statement_t2])
            .unwrap();

        assert_eq!(4, results.len());
        match &results[0] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.status, ReconciliationMatchStatus::PartialMatch);
            }
            _ => panic!("expected original transaction"),
        }
        match &results[1] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.status, ReconciliationMatchStatus::PartialMatch);
            }
            _ => panic!("expected reconciliation transaction"),
        }
        match &results[2] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.status, ReconciliationMatchStatus::Matched);
            }
            _ => panic!("expected original transaction"),
        }
        match &results[3] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.status, ReconciliationMatchStatus::Matched);
            }
            _ => panic!("expected reconciliation transaction"),
        }
    }

    #[test]
    fn test_reconcile_invalid_account() {
        let (books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let result = books.prepare_reconciliation(Uuid::new_v4(), vec![t1]);
        assert!(result.is_err());
    }

     #[test]
    fn test_reconcile_match_after_unreconciled_entry() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let t2 = build_transaction_with_date(None, Some(id2), NaiveDate::from_ymd_opt(2022, 6, 5).unwrap());
        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();

        let mut statement_t2 = clone_transaction_for_reconcile(&t2);
        for e in &mut statement_t2.entries {
            if e.account_id == id2 {
                e.balance = Some(dec!(-20000));
                break;
            }
        }

        let statement_t2_id = statement_t2.id;
        let results = books.prepare_reconciliation(id2, vec![statement_t2]).unwrap();
        assert_eq!(3, results.len());
        match &results[0] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.transaction.id, t1.id);
                assert_eq!(target.status, ReconciliationMatchStatus::Unmatched);
                assert_eq!(target.matched_reconciliation_id, None);
            }
            _ => panic!("expected original transaction"),
        }
        match &results[1] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.transaction.id, t2.id);
                assert_eq!(target.status, ReconciliationMatchStatus::Matched);
                assert_eq!(target.matched_reconciliation_id, Some(statement_t2_id));
            }
            _ => panic!("expected original transaction"),
        }
        match &results[2] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.status, ReconciliationMatchStatus::Matched);
                assert_eq!(recon.matched_transaction_id, Some(t2.id));
            }
            _ => panic!("expected reconciliation transaction"),
        }
    }

    fn clone_transaction_for_reconcile(t: &Transaction) -> Transaction {
        let new_id = Uuid::new_v4();
        Transaction {
            id: new_id,
            entries: t
                .entries
                .iter()
                .map(|e| Entry {
                    id: Uuid::new_v4(),
                    transaction_id: new_id,
                    date: e.date,
                    description: e.description.clone(),
                    account_id: e.account_id,
                    entry_type: e.entry_type,
                    amount: e.amount,
                    balance: None,
                    reconciled_status: None,
                })
                .collect(),
            status: t.status,
            source_type: None,
            source_id: None,
        }
    }

    #[test]
    fn test_account_transactions() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let t2 = build_transaction_with_date(None, Some(id2),NaiveDate::from_ymd_opt(2022, 6, 5).unwrap());
        let t3 = build_transaction_with_date(Some(id1), None, NaiveDate::from_ymd_opt(2022, 7, 1).unwrap());
        let t4 = build_transaction_with_date(Some(id2), Some(id1), NaiveDate::from_ymd_opt(2022, 7, 2).unwrap());
        let t1a1e1 = &t1.account_entries(id1)[0];
        let t3a1e3 = &t3.account_entries(id1)[0];
        let t4a1e4 = &t4.account_entries(id1)[0];
        let t1a2e1 = &t1.account_entries(id2)[0];
        let t2a2e1 = &t2.account_entries(id2)[0];
        let t4a2e1 = &t4.account_entries(id2)[0];
        books.add_transaction(t1).unwrap();
        books.add_transaction(t2).unwrap();
        books.add_transaction(t3).unwrap();
        books.add_transaction(t4).unwrap();
        let a1_entries = books.account_transactions(id1).unwrap();
        assert_eq!(3, a1_entries.len());

        let entry1 = &a1_entries[0].account_entries(id1)[0];
        assert_eq!(t1a1e1.id, entry1.id);
        assert_eq!(dec!(10000), entry1.balance.unwrap());

        let entry2 = &a1_entries[1].account_entries(id1)[0];
        assert_eq!(t3a1e3.id, entry2.id);
        assert_eq!(dec!(20000), entry2.balance.unwrap());

        let entry3 = &a1_entries[2].account_entries(id1)[0];
        assert_eq!(t4a1e4.id, entry3.id);
        assert_eq!(dec!(10000), entry3.balance.unwrap());

        let a2_entries = books.account_transactions(id2).unwrap();
        assert_eq!(3, a2_entries.len());

        let entry21 = &a2_entries[0].account_entries(id2)[0];
        assert_eq!(t1a2e1.id, entry21.id);
        assert_eq!(dec!(-10000), entry21.balance.unwrap());

        let entry22 = &a2_entries[1].account_entries(id2)[0];
        assert_eq!(t2a2e1.id, entry22.id);
        assert_eq!(dec!(-20000), entry22.balance.unwrap());

        let entry23 = &a2_entries[2].account_entries(id2)[0];
        assert_eq!(t4a2e1.id, entry23.id);
        assert_eq!(dec!(-10000), entry23.balance.unwrap());


    }

    #[test]
    fn test_add_schedule() {
        let (mut books, id1, id2) = setup_books();
        let st1 = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let _result = books.add_schedule(st1);
        let expected: Result<(), BooksError> = Err(BooksError { error: tr!("errors.invalid_cr_account") });
        assert!(matches!(expected, _result));
        assert_eq!(1, (&books.schedules()).len());
    }

    #[test]
    fn test_update_schedule() {
        let (mut books, id1, id2) = setup_books();
        let st1 = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let mut st1_copy = st1.clone();
        let _result = books.add_schedule(st1);

        st1_copy.entries[0].description = "test changed".to_string();
        let _result = books.update_schedule(st1_copy);
        assert_eq!(1, (books.schedules()).len());
        assert_eq!("test changed", books.schedules()[0].entries[0].description);
    }

    #[test]
    fn test_delete_schedule() {
        let (mut books, id1, id2) = setup_books();
        let st1 = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let st1_id = st1.id;
        books.add_schedule(st1).unwrap();
        assert_eq!(1, books.schedules().len());

        let result = books.delete_schedule(&st1_id);
        assert!(result.is_ok());
        assert_eq!(0, books.schedules().len());
    }

    #[test]
    fn test_cannot_delete_schedule_with_transactions() {
        let (mut books, id1, id2) = setup_books();
        let st1 = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let st1_id = st1.id;
        books.add_schedule(st1).unwrap();

        // Generate transactions from the schedule
        books.generate(NaiveDate::from_ymd_opt(2023, 6, 4).unwrap());
        
        // Verify transactions were created with schedule_id
        assert!(books.transactions().len() > 0);
        assert!(books.transactions().iter().any(|t| {t.source_type == Some(Source::Schedule) && t.source_id == Some(st1_id)}));

        // Try to delete the schedule - should fail
        let result = books.delete_schedule(&st1_id);
        assert_eq!(
            tr!("errors.schedule_cannot_delete_with_transactions", id => st1_id),
            result.err().unwrap().error
        );
        assert_eq!(1, books.schedules().len());
    }

    #[test]
    fn test_cannot_delete_schedule_with_invalid_id() {
        let (mut books, id1, id2) = setup_books();
        let st1 = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        books.add_schedule(st1).unwrap();
        
        let invalid_id = Uuid::new_v4();
        let result = books.delete_schedule(&invalid_id);
        assert_eq!(
            tr!("errors.schedule_not_found", id => invalid_id),
            result.err().unwrap().error
        );
        assert_eq!(1, books.schedules().len());
    }


    #[test]
    fn test_add_schedule_invalid_dr_account() {
        let (mut books, id1, id2) = setup_books();
        let st1 = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let _result = books.add_schedule(st1);
        let expected: Result<(), BooksError> = Ok(());
        assert!(matches!(expected, _result));
        assert_eq!(1, (&books.schedules()).len());
    }

    #[test]
    fn test_add_schedule_invalid_cr_account() {
        let (mut books, id1, _) = setup_books();
        let st1 = build_schedule_std(id1, Uuid::new_v4(), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let _result = books.add_schedule(st1);
        let expected: Result<(), BooksError> = Err(BooksError { error: tr!("errors.invalid_cr_account") });
        assert!(matches!(expected, _result));
        assert_eq!(0, (&books.schedules()).len());
    }

    #[test]
    fn test_generate() {
        let (mut books, id1, id2) = setup_books();
        let _result = books.add_schedule(
            build_schedule(id1, id2, NaiveDate::from_ymd_opt(2022, 3, 11).unwrap(), "S_1", "st test 1", dec!(100.99), 3, ScheduleEnum::Months)
        );

        let _result = books.add_schedule(
            build_schedule(id2, id1, NaiveDate::from_ymd_opt(2022, 3, 11).unwrap(), "S_2", "st test 2", dec!(20.23), 45, ScheduleEnum::Days)
        );

        assert_eq!(0, books.transactions.len());
        books.generate(NaiveDate::from_ymd_opt(2023, 3, 11).unwrap());

        assert_eq!(14, books.transactions.len());
        assert_eq!("st test 2", books.transactions[2].entries[0].description);
        assert_eq!("st test 1", books.transactions[4].entries[0].description);
    }

    pub fn setup_books() -> (Books, Uuid, Uuid) {
        let mut books = Books::build_empty("My Books");
        let dr_account1 = Account::create_new("Savings Account 1", AccountType::Asset);
        let dr_account_id: Uuid = dr_account1.id;
        books.add_account(dr_account1);
        let cr_account1 = Account::create_new("Savings Account 2", AccountType::Asset);
        let cr_account_id: Uuid = cr_account1.id;
        books.add_account(cr_account1);
        (books, dr_account_id, cr_account_id)
    }

    fn build_transaction(id1: Option<Uuid>, id2: Option<Uuid>) -> Transaction {
        build_transaction_with_date(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap())
    }

    pub fn build_transaction_with_date(dr_account_id: Option<Uuid>, cr_account_id: Option<Uuid>, date: NaiveDate) -> Transaction {
        let transaction_id = Uuid::new_v4();
        let description_str = "received moneys";
        let amount = dec!(10000);
        let mut t1 = Transaction{
            id: transaction_id,
            entries: Vec::new(),
            status: TransactionStatus::Recorded,
            source_type: None,
            source_id: None,
        };

        if dr_account_id.is_some() {
            t1.entries.push(Entry{id:Uuid::new_v4(),transaction_id,date,description: description_str.to_string(),account_id:dr_account_id.unwrap(),
                entry_type:Side::Debit, amount,balance:None, reconciled_status: None })
        }

        if cr_account_id.is_some() {
            t1.entries.push(Entry{id:Uuid::new_v4(),transaction_id,date,description: description_str.to_string(),account_id:cr_account_id.unwrap(),
                entry_type:Side::Credit,amount,balance:None, reconciled_status: None })
        }
        t1
    }

    fn build_schedule_std(id1: Uuid, id2: Uuid, start_date: NaiveDate) -> Schedule {
        build_schedule(id1, id2, start_date, "Reocurring transaction", "Reocurring transaction", dec!(100), 1, ScheduleEnum::Months)
    }

    fn build_schedule(id1: Uuid, id2: Uuid, start_date: NaiveDate, name: &str, description: &str, amount: Decimal, frequency: i64, period: ScheduleEnum) -> Schedule {
        let s_id_1 = Uuid::new_v4();
        Schedule {
            id: s_id_1,
            name: name.to_string(),
            start_date,
            end_date: None,
            last_date: None,
            frequency,
            period,
            entries: vec![
                    ScheduleEntry {
                        amount,
                        description: description.to_string(),
                        account_id: id1,
                        entry_type: Side::Debit,
                        schedule_id: s_id_1,
                    },
                    ScheduleEntry {
                        amount,
                        description: description.to_string(),
                        account_id: id2,
                        entry_type: Side::Credit,
                        schedule_id: s_id_1,
                    }
                ],
            schedule_modifiers: vec![],
        }
    }

    #[test]
    fn test_reset_schedule_last_date_with_transactions() {
        let (mut books, id1, id2) = setup_books();
        let schedule = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let schedule_id = schedule.id;
        
        // Add the schedule
        books.add_schedule(schedule).unwrap();
        
        // Create some transactions for this schedule
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let mut t1_with_schedule = t1;        
        t1_with_schedule.set_source_schedule(schedule_id);
        
        let t2 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 7, 4).unwrap());
        let mut t2_with_schedule = t2;
        t2_with_schedule.set_source_schedule(schedule_id);
        
        let t3 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 8, 4).unwrap());
        let mut t3_with_schedule = t3;
        t3_with_schedule.set_source_schedule(schedule_id);
        
        // Add transactions out of order to test that sorting finds the latest date
        books.add_transaction(t3_with_schedule).unwrap(); // August 4
        books.add_transaction(t1_with_schedule).unwrap(); // June 4
        books.add_transaction(t2_with_schedule).unwrap(); // July 4
        
        // Reset the schedule last date
        let result = books.reset_schedule_last_date(schedule_id);
        
        // Should return the date of the latest transaction (August 4, 2022)
        // Now that transactions are sorted by date, it should find August 4th regardless of addition order
        assert_eq!(result, Some(NaiveDate::from_ymd_opt(2022, 8, 4).unwrap()));
        
        // Verify the schedule was updated
        let updated_schedule = books.get_schedule(schedule_id).unwrap();
        assert_eq!(updated_schedule.last_date, Some(NaiveDate::from_ymd_opt(2022, 8, 4).unwrap()));
    }

    #[test]
    fn test_reset_schedule_last_date_no_transactions() {
        let (mut books, id1, id2) = setup_books();
        let schedule = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let schedule_id = schedule.id;
        
        // Add the schedule but no transactions
        books.add_schedule(schedule).unwrap();
        
        // Reset the schedule last date
        let result = books.reset_schedule_last_date(schedule_id);
        
        // Should return None since there are no transactions
        assert_eq!(result, None);
        
        // Verify the schedule was updated with None
        let updated_schedule = books.get_schedule(schedule_id).unwrap();
        assert_eq!(updated_schedule.last_date, None);
    }

    #[test]
    fn test_reset_schedule_last_date_transactions_for_other_schedules() {
        let (mut books, id1, id2) = setup_books();
        let schedule1 = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let schedule1_id = schedule1.id;
        
        let schedule2 = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let schedule2_id = schedule2.id;
        
        // Add both schedules
        books.add_schedule(schedule1).unwrap();
        books.add_schedule(schedule2).unwrap();
        
        // Create transactions for schedule1
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let mut t1_with_schedule = t1;
        t1_with_schedule.set_source_schedule(schedule1_id);
        
        // Create transactions for schedule2 (later date)
        let t2 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 8, 4).unwrap());
        let mut t2_with_schedule = t2;
        t2_with_schedule.set_source_schedule(schedule2_id);
        
        // Add transactions
        books.add_transaction(t1_with_schedule).unwrap();
        books.add_transaction(t2_with_schedule).unwrap();
        
        // Reset schedule1's last date
        let result = books.reset_schedule_last_date(schedule1_id);
        
        // Should return the date of schedule1's last transaction (June 4, 2022)
        assert_eq!(result, Some(NaiveDate::from_ymd_opt(2022, 6, 4).unwrap()));
        
        // Verify schedule1 was updated correctly
        let updated_schedule1 = books.get_schedule(schedule1_id).unwrap();
        assert_eq!(updated_schedule1.last_date, Some(NaiveDate::from_ymd_opt(2022, 6, 4).unwrap()));
        
        // Verify schedule2 was not affected
        let updated_schedule2 = books.get_schedule(schedule2_id).unwrap();
        assert_eq!(updated_schedule2.last_date, None);
    }

    #[test]
    #[should_panic(expected = "Schedule not found")]
    fn test_reset_schedule_last_date_nonexistent_schedule() {
        let (mut books, _id1, _id2) = setup_books();
        let fake_schedule_id = Uuid::new_v4();
        
        // Try to reset last date for a schedule that doesn't exist - should panic
        books.reset_schedule_last_date(fake_schedule_id);
    }

    #[test]
    fn test_reset_schedule_last_date_with_existing_last_date() {
        let (mut books, id1, id2) = setup_books();
        let schedule = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let schedule_id = schedule.id;
        
        // Add the schedule with an existing last_date
        let mut schedule_with_last_date = schedule;
        schedule_with_last_date.last_date = Some(NaiveDate::from_ymd_opt(2022, 5, 4).unwrap());
        books.add_schedule(schedule_with_last_date).unwrap();
        
        // Create a transaction after the existing last_date
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let mut t1_with_schedule = t1;
        t1_with_schedule.set_source_schedule(schedule_id);
        
        books.add_transaction(t1_with_schedule).unwrap();
        
        // Reset the schedule last date
        let result = books.reset_schedule_last_date(schedule_id);
        
        // Should return the date of the last transaction (June 4, 2022), overwriting the old date
        assert_eq!(result, Some(NaiveDate::from_ymd_opt(2022, 6, 4).unwrap()));
        
        // Verify the schedule was updated with the new date
        let updated_schedule = books.get_schedule(schedule_id).unwrap();
        assert_eq!(updated_schedule.last_date, Some(NaiveDate::from_ymd_opt(2022, 6, 4).unwrap()));
    }

}
