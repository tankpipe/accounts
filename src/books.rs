use std::collections::HashSet;
use std::{collections::HashMap, cmp::Ordering};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::books_error;

use crate::account::{Account, AccountType, Entry, ReconciledStatus, Source, Transaction, TransactionStatus};
use crate::interest::{Interest, calculate_interest_for_accounts};
use crate::reconcile::{ReconciliationItem, ReconciliationMatchStatus, ReconciliationResult, TargetResult};
use crate::schedule::{Modifier, Schedule};
use crate::scheduler::Scheduler;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct Settings {
    pub require_double_entry: bool,
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
            recalculate_interest: HashSet::new(),
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
            recalculate_interest: HashSet::new(),
        }
    }

    pub fn add_account(&mut self, account: Account) {
        let mut account = account;
        account.reconciliation_info = None;
        self.flag_interest_outdated_by_account(&account);
        if ! self.accounts.contains_key(&account.id) {
            self.accounts.insert(account.id, account);
        }        
    }

    pub fn update_account(&mut self, account: Account) -> Result<(), BooksError> {
        let existing = self
            .accounts
            .get(&account.id)
            .ok_or_else(|| books_error!("errors.account_not_found", id => account.id))?;

        let same_reconciliation = match (&account.reconciliation_info, &existing.reconciliation_info) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.date == b.date && a.balance == b.balance && a.transaction_id == b.transaction_id
            }
            _ => false,
        };

        if !same_reconciliation {
            return Err(books_error!("errors.account_reconciliation_info_immutable"));
        }

        if account.account_type != existing.account_type
            && self.transactions.iter().any(|t| t.involves_account(&account.id))
        {
            return Err(books_error!("errors.account_type_immutable_with_transactions"));
        }

        if account.starting_balance != existing.starting_balance
            && existing.reconciliation_info.is_some()
        {
            return Err(books_error!("errors.account_starting_balance_immutable_after_reconciliation"));
        }

        self.flag_interest_outdated_by_account(&account);
        self.accounts.insert(account.id, account);
        Ok(())
    }

    pub fn delete_account(&mut self, id: &Uuid) -> Result<(), BooksError> {
        if !self.accounts.contains_key(id) {
            return Err(books_error!("errors.account_not_found", id => id));
        }

        if self.transactions.iter().any(|t|t.involves_account(id)) {
            return Err(books_error!("errors.account_cannot_delete_with_transactions", id => id));
        }
        
        if let Some(account) = self.accounts.remove(id) {
            self.flag_interest_outdated_by_account(&account);
        }
        
        Ok(())
    }

    pub fn get_account(&self, id: &Uuid) -> Result<Account, BooksError> {
        self.accounts.get(id).cloned().ok_or(books_error!("errors.account_not_found", id => id))
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
            return Err(books_error!("errors.transaction_requires_two_entries"))
        } else if transaction.entries.len() < 1 {
            return Err(books_error!("errors.transaction_requires_one_entry"))
        }

        // Using transaction date to avoid potential reconciliation edge case for split date entries.
        if transaction.status == TransactionStatus::Recorded && transaction.date() > Some(chrono::Utc::now().date_naive()) {
            return Err(books_error!("errors.future_transaction_set_as_recorded"))
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
        if let Some(orginal_transaction) = self.transactions.iter().find(|t| t.id == transaction.id) {
            for original_entry in orginal_transaction.entries.iter() {
                if original_entry.is_reconciled_or_outstanding() {
                    let matching_entry = transaction.entries.iter().find(|e| e.account_id == original_entry.account_id);
                    if matching_entry.is_none() || matching_entry.unwrap() != original_entry {
                        return Err(books_error!("errors.reconciled_entry_immutable"))
                    }
                }
            }
        }

        // If the transaction is net new,
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
                            return Err(books_error!("errors.transaction_before_reconciliation_date", transaction_date = entry.date, reconciliation_date = reconciliation_info.date));
                        }
                    }
                }
            }
        }
            
        Ok(())
    }

    pub fn update_transaction(&mut self, transaction: Transaction) -> Result<(), BooksError> {

        self.validate_transaction(&transaction)?;

        if let Some(index) = self.transactions.iter().position(|t| t.id == transaction.id) {
            self.flag_interest_outdated(&transaction);
            let old = std::mem::replace(&mut self.transactions[index], transaction);
            self.flag_interest_outdated(&old);            
            Ok(())
        } else {
            Err(books_error!("errors.transaction_not_found", id => transaction.id))
        }

    }

    fn reconcile_transaction(&mut self, mut transaction: Transaction, account_id: Uuid, status: ReconciledStatus) -> Result<(), BooksError> {

        self.validate_transaction(&transaction)?;
        
        let transaction_id = transaction.id;

        if let Some(entry) = transaction.entries.iter_mut().find(|e| e.account_id == account_id) {
            if entry.reconciled_status.is_none_or(|rs|rs != status) { 
                entry.reconciled_status = Some(status);
                transaction.status = TransactionStatus::Recorded;
                if let Some(index) = self.transactions.iter().position(|t| t.id == transaction_id) {
                    let _old = std::mem::replace(&mut self.transactions[index], transaction);
                } else {
                    return Err(books_error!("errors.transaction_not_found", id => transaction_id))
                }
            }
        }

        Ok(())
    }

    pub fn delete_transaction(&mut self, id: &Uuid) -> Result<(), BooksError> {
        if let Some(index) = self.transactions.iter().position(|t| t.id == *id) {
            // Check reconciled status first
            if self.transactions[index].entries.iter().any(|e| e.is_reconciled_or_outstanding()) {
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

    fn flag_interest_outdated(&mut self, transaction: &Transaction) -> bool {
        if self.interest_outdated() { return true }
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
        if self.interest_outdated() { return true }
        
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
            return Err(books_error!("errors.account_not_found", id => account_id));
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
        self.validate_schedule(&schedule)?;
        self.scheduler.add_schedule(schedule);
        Ok(())
    }

    fn validate_schedule(&mut self, schedule: &Schedule) -> Result<(), BooksError> {

        if schedule.entries.len() < 1 {
            return Err(books_error!("errors.schedule_requires_entry"))
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
        if self.transactions.iter().any(|t| {t.source_type == Some(Source::Schedule) && t.source_id == Some(*id)}) {
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

    pub fn transactions_by_interest(&self, interest_id: Uuid, status: Option<TransactionStatus>, from: Option<NaiveDate>) -> Vec<Transaction> {
        print!("transactions_by_interest called with interest_id: {}, status: {:?}, from: {:?}", interest_id, status, from);
        self.transactions
            .iter()
            .filter(|t| {t.source_type == Some(Source::Interest) && t.source_id == Some(interest_id)})
            .filter(|t| {
                match from {
                    Some(filter_from) => t.date() >= Some(filter_from),
                    None => true,
                }
            })
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
        self.interests.get(interest_id)
            .cloned()
            .ok_or(books_error!("errors.interest_info_not_found", id => interest_id))
    }

    pub fn update_interest(&mut self, interest: Interest) -> Result<(), BooksError> {
        self.validate_interest(&interest)?;        
        self.check_recalculate_interest(&interest);

        let account = self.accounts.get(&interest.account_id)
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
                if account.account_type != AccountType::Asset && account.account_type != AccountType::Liability {
                    return Err(books_error!("errors.invalid_account_type", name => account.name))
                }
            }
        
            if let Some(income_account_id) = t.income_account_id {
                self.valid_account_id(income_account_id)?;
                let account = self.get_account(&income_account_id)?;
                if account.account_type != AccountType::Revenue && account.account_type != AccountType::Expense {
                    return Err(books_error!("errors.invalid_account_type", name => account.name))
                }
            }
        }
        
        Ok(())
    }

    pub fn reset_schedule_last_date(&mut self, schedule_id: Uuid) -> Result<Option<NaiveDate>, BooksError> {
        let mut transactions: Vec<Transaction> = self.transactions
            .iter()
            .filter(|t| {t.source_type == Some(Source::Schedule) && t.source_id == Some(schedule_id)})
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
                confidence: 0.0,
                signals: Vec::new(),
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
            return Err(books_error!("errors.account_not_found", id => account_id));
        }

        let mut account_transactions = self.account_transactions(account_id)?;
        let mut new_recon_transaction: Option<Transaction> = None;
        // set the last index to the account reconciliation_info transaction_id index   
        let mut last_index: Option<usize> = self.accounts.get(&account_id).unwrap().reconciliation_info.as_ref().map(|info| info.transaction_id).map(|id| account_transactions.iter().position(|t| t.id == id).unwrap());
        let mut first_index: Option<usize> = None;
        
        // Reconcile each transaction.
        for transaction_id in transaction_ids {

            let idx = account_transactions.iter().position(|t| t.id == transaction_id).ok_or_else(|| {
                books_error!("errors.transaction_not_found_for_account", transaction_id => transaction_id, account_id => account_id)
            })?;
            
            let transaction = account_transactions.iter_mut().find(|t| t.id == transaction_id).ok_or_else(|| {
                books_error!("errors.transaction_not_found_for_account", transaction_id => transaction_id, account_id => account_id)
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
                if let Some(existing) = self.transactions.iter_mut().find(|t| t.id == transaction.id) {
                    if let Some(entry) = existing.entries.iter_mut().find(|e| e.account_id == account_id) {
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

    pub fn recalculate_interest(&mut self, projection_date: NaiveDate) -> Result<(), BooksError>{
        println!("Calculating interest...");
        let interest_accounts = self.accounts().into_iter().filter(|a| {
            self.recalculate_interest.contains(&a.id) && a.interest_id.is_some()
        }).collect();
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

    pub fn run_checks_and_update(&mut self, projection_date: NaiveDate) -> Result<(), BooksError>{
        println!("Running checks 📋  Projection date: {}", projection_date);
        println!("Generating schedules...");
        self.generate(projection_date);
        let interest_accounts = self.accounts.values().filter(|a| a.interest_id.is_some()).cloned().collect();
        println!("Calculating interest...");
        calculate_interest_for_accounts(self, interest_accounts, projection_date)?;
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

// books tests can be found in ../tests/books_test.rs
