use chrono::{Datelike, Days, NaiveDate, Utc};
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use serde::Serialize;
use uuid::Uuid;

use crate::{account::{Account, AccountType, Entry, Side, Source, Transaction, TransactionStatus}, books::{Books, BooksError}, schedule::ScheduleEnum, serializer::*};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

pub const CALC_DECIMAL_PRECISION: u32 = 4;
pub const DECIMAL_PRECISION: u32 = 2;
pub const DAYS_PER_ANNUM: u32 = 365;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum InterestType {
    Daily,          // End of day balance
    AverageDaily,   // Average end of day balance
    MinimumMonthly  // Minimum monthly end of day balance
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct InterestTerms  {   
    pub id: Uuid, 
    #[serde(serialize_with = "serialize_naivedate")]
    #[serde(deserialize_with = "deserialize_naivedate")]
    pub start_date: NaiveDate,
    #[serde(serialize_with = "serialize_option_naivedate")]
    #[serde(deserialize_with = "deserialize_option_naivedate")]
    pub end_date: Option<NaiveDate>,
    pub rate: Decimal,
    pub calculated: InterestType,   
    pub min_balance: Option<Decimal>,   // Defaults to 0
    pub max_balance: Option<Decimal>,
    pub paid_period: ScheduleEnum,     
    pub paid_frequency: i32,
    pub paid_day: i32,
    pub description: String,
    pub interest_account_id: Option<Uuid>
}

impl InterestTerms {
    pub fn simple(start_date: NaiveDate, rate: Decimal, calculated: InterestType, paid_period: ScheduleEnum, paid_frequency: i32, paid_day: i32, description: String, interest_account_id: Option<Uuid>) -> Self {
        InterestTerms {
            id: Uuid::new_v4(),            
            start_date,
            end_date: None,
            rate,
            calculated,
            min_balance: None,
            max_balance: None,
            paid_period,
            paid_frequency,
            paid_day,
            description,
            interest_account_id
        }
    }

    pub fn from_components(start_date: NaiveDate, end_date: Option<NaiveDate>, rate: Decimal, calculated: InterestType, min_balance: Option<Decimal>, max_balance: Option<Decimal>, paid_period: ScheduleEnum, paid_frequency: i32, paid_day: i32, description: String, interest_account_id: Option<Uuid>) -> Self {
        InterestTerms {
            id: Uuid::new_v4(),            
            start_date,
            end_date,
            rate,
            calculated,
            min_balance,
            max_balance,
            paid_period,
            paid_frequency,
            paid_day,
            description,
            interest_account_id
        }
    }

    pub fn is_end_of_interest_period(&self, date: NaiveDate) -> bool {
        let paid_to_day = if self.paid_day > 1 { self.paid_day as u32 - 1 } else {31};
        date.day() == paid_to_day || (paid_to_day > date.day() && date.succ_opt().unwrap().day() == 1)
    }
    
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Interest  {
    pub id: Uuid,
    pub terms: Vec<InterestTerms>,
    pub account_id: Uuid,
}

impl Interest {
    pub fn from_components(terms: Vec<InterestTerms>, account_id: Uuid) -> Self {
        Interest {
            id: Uuid::new_v4(),
            terms,
            account_id
        }
    }

    pub fn get_terms_for_date(&self, date: NaiveDate) -> Vec<&InterestTerms> {
        self.terms.iter().filter(|t| t.start_date <= date && t.end_date.map_or(true, |end| end >= date)).collect()
    }

    pub fn get_start_date(&self) -> Option<&NaiveDate> {
        self.terms.iter().min_by(|t1, t2| t1.start_date.cmp(&t2.start_date)).map(|t| &t.start_date)
    }
}

fn build_interest_transaction(source_account: &Account, interest_account: &Option<Account>, cur_date: NaiveDate, interest_tally: Decimal) -> Transaction {
    let transaction_id = uuid::Uuid::new_v4();
    let is_interest_bearing = source_account.account_type == AccountType::Asset;
    
    let mut transaction = Transaction {
        id: transaction_id,
        entries: vec![],
        status: TransactionStatus::Projected,
        source_type: Some(Source::Interest),
        source_id: source_account.interest_id,
    };
    if is_interest_bearing || interest_account.as_ref().is_some() {
        transaction.entries.push(Entry{
            id: uuid::Uuid::new_v4(),
            transaction_id: transaction_id,
            account_id: if is_interest_bearing { source_account.id } else { interest_account.as_ref().unwrap().id },
            date: cur_date,
            entry_type: Side::Debit,
            amount: interest_tally,
            balance: None,
            description: "Interest payment".to_string(),
            reconciled_status: None,
        });
    }
    if !is_interest_bearing || interest_account.as_ref().is_some() {
        transaction.entries.push(Entry{
            id: uuid::Uuid::new_v4(),
            transaction_id: transaction_id,
            account_id: if is_interest_bearing { interest_account.as_ref().unwrap().id } else { source_account.id },
            date: cur_date,
            entry_type: Side::Credit,
            amount: interest_tally,
            balance: None,
            description: "Interest payment".to_string(),
            reconciled_status: None,
        });
    }
    transaction
}

fn is_end_of_month(date: NaiveDate) -> bool {
    let year = date.year();
    let month = date.month();
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = NaiveDate::from_ymd_opt(next_year, next_month, 1).unwrap();    
    date == first_next.pred_opt().unwrap()
}

#[derive(Debug)]
struct InterestCalculationState {
    account: Account,
    interest: Interest,
    entries: Vec<Entry>,
    entry_index: usize,
    balance: Decimal,
    interest_paid: Decimal,
    interest_tally_by_account: HashMap<Uuid, Decimal>,
    interest_tally_no_account: Decimal,
    start_date: NaiveDate,
    recorded_transaction_keys: HashSet<(NaiveDate, Option<Uuid>)>,
    recorded_transaction_amounts: HashMap<(NaiveDate, Option<Uuid>), Decimal>,
}

// Calculate interest for all provided accounts in a single pass over time.
// This function:
// - Builds a per‑account interest calculation state (terms, entries, tallies)
// - Chooses a common start date across accounts so we can walk the timeline once
// - Accumulates daily interest for each account
// - Posts month‑end interest transactions and feeds their effects back into
//   the per‑account balances on subsequent days.
pub fn calculate_interest_for_accounts(books: &mut Books, interest_accounts: Vec<Account>, to_date: NaiveDate) -> Result<(), BooksError> {       
    let mut states: Vec<InterestCalculationState> = Vec::new();
    let mut earliest_start_date: Option<NaiveDate> = None;
    let today = Utc::now().date_naive();

    // Build an InterestCalculationState for each account that has
    // interest terms and track earliest start date
    for account in interest_accounts {
        let Some(interest_id) = account.interest_id else { continue };
        let Ok(interest) = books.get_interest(&interest_id) else { continue };
        if interest.terms.is_empty() { continue };

        // Pull out existing interest transactions so we can:
        // - remember recorded ones (to help avoid duplicate posting)
        // - delete any projected ones that we’re about to recalculate.
        let existing_interest_transactions = books.transactions_by_interest(interest.id, None, None);
        let mut recorded_transaction_keys: HashSet<(NaiveDate, Option<Uuid>)> = HashSet::new();
        let mut recorded_transaction_amounts: HashMap<(NaiveDate, Option<Uuid>), Decimal> = HashMap::new();
        let mut recorded_dates: Vec<NaiveDate> = Vec::new();
        let mut projected_dates: Vec<NaiveDate> = Vec::new();

        for transaction in &existing_interest_transactions {
            if let Some(date) = transaction.date() {
                if transaction.status == TransactionStatus::Recorded {
                    let interest_account_id = transaction
                        .entries
                        .iter()
                        .find(|e| e.account_id != account.id)
                        .map(|e| e.account_id);
                    recorded_transaction_keys.insert((date, interest_account_id));
                    recorded_dates.push(date);
                    let amount = transaction
                        .entries
                        .iter()
                        .find(|e| e.account_id == account.id)
                        .map(|e| e.amount)
                        .or_else(|| transaction.entries.first().map(|e| e.amount))
                        .unwrap_or(dec!(0));
                    recorded_transaction_amounts.insert((date, interest_account_id), amount);
                } else {
                    projected_dates.push(date);
                }
            }
        }

        for transaction in existing_interest_transactions {
            if transaction.status != TransactionStatus::Recorded {
                let result = books.delete_transaction(&transaction.id);
                if let Err(e) = result {
                    println!("Error deleting transaction: {:?}, {:?}, {:?}", transaction.id, transaction.date(), e);
                }
            }
        }

        // The start date should be the the last recorded interest date before all 
        // projected interest dates. Otherwise the earliest term.start_date
        let earliest_projected_date = projected_dates.into_iter().min();
        let last_recorded_before_earliest = match earliest_projected_date {
            Some(projected_date) => recorded_dates.iter().copied().filter(|d| *d < projected_date).max(),
            None => recorded_dates.iter().copied().max(),
        };

        let Some(first_term_start) = interest.get_start_date().cloned() else {
            continue;
        };

        let mut start_date = match last_recorded_before_earliest {
            Some(date) => date.checked_add_days(Days::new(1)).unwrap(),
            None => first_term_start,
        };

        if start_date > today {
            start_date = today;
        }

        let entries = books.account_entries(account.id)?;

        // Track the earliest start date across all accounts; we’ll walk the
        // timeline from this date once and update each account’s state in lock‑step.
        earliest_start_date = Some(match earliest_start_date {
            Some(current) => current.min(start_date),
            None => start_date,
        });

        states.push(InterestCalculationState {
            account,
            interest,
            entries,
            entry_index: 0,
            balance: Decimal::ZERO,
            interest_paid: dec!(0),
            interest_tally_by_account: HashMap::new(),
            interest_tally_no_account: dec!(0),
            start_date,
            recorded_transaction_keys,
            recorded_transaction_amounts,
        });
    }

    let Some(earliest_start_date) = earliest_start_date else { return Ok(()) };

    // Initialise each account’s balance to what it was immediately before the
    // earliest start date by replaying entries up to (but not including) that day.
    for state in &mut states {
        state.balance = state.account.starting_balance;
        let normal_balance = state.account.normal_balance();
        while state.entry_index < state.entries.len() && state.entries[state.entry_index].date < earliest_start_date {
            let entry = &state.entries[state.entry_index];
            if entry.entry_type == normal_balance {
                state.balance += entry.amount;
            } else {
                state.balance -= entry.amount;
            }
            state.entry_index += 1;
        }
    }

    // When we post interest transactions they can affect multiple accounts.
    // Rather than immediately mutating balances mid‑iteration, we record the
    // per‑account per‑date deltas here and apply them when we reach that date.
    let mut pending_deltas: HashMap<Uuid, HashMap<NaiveDate, Decimal>> = HashMap::new();
    let state_index_by_account: HashMap<Uuid, usize> = states
        .iter()
        .enumerate()
        .map(|(idx, state)| (state.account.id, idx))
        .collect();

    // Walk the calendar from the earliest start date to the requested end date,
    // updating each account’s balance, accruing interest, and posting month‑end.
    let mut cur_date = earliest_start_date;
    while cur_date <= to_date {
        for state in &mut states {
            let normal_balance = state.account.normal_balance();

            while state.entry_index < state.entries.len() && state.entries[state.entry_index].date <= cur_date {
                let entry = &state.entries[state.entry_index];
                if entry.entry_type == normal_balance {
                    state.balance += entry.amount;
                } else {
                    state.balance -= entry.amount;
                }
                state.entry_index += 1;
            }

            apply_pending_delta_for_date(&mut pending_deltas, state.account.id, cur_date, &mut state.balance);

            if cur_date < state.start_date {
                continue;
            }

            let cur_terms = state.interest.get_terms_for_date(cur_date);
            for terms in cur_terms {
                let daily_rate = terms.rate / dec!(365);
                let min_balance = terms.min_balance.unwrap_or(dec!(0));

                if state.balance >= min_balance {
                    let interest_amount: Decimal;
                    if terms.max_balance.is_some() && (state.balance + state.interest_paid) >= terms.max_balance.unwrap() {
                        interest_amount = daily_rate * (terms.max_balance.unwrap() - min_balance);
                    } else {
                        interest_amount = daily_rate * (state.balance + state.interest_paid - min_balance);
                    }

                    let current_balance;
                    if let Some(interest_account_id) = terms.interest_account_id {
                        current_balance = state.interest_tally_by_account.entry(interest_account_id).or_insert(dec!(0));
                    } else {
                        current_balance = &mut state.interest_tally_no_account;
                    }

                    let new_total = *current_balance + interest_amount;
                    *current_balance = new_total;
                    //println!("{}, {}, {}, Interest amount: {}, tally {}", cur_date, state.account.name, current_balance, interest_amount, new_total);
                }
            }

            // TODO Model supports more flexibility than just month end.
            if is_end_of_month(cur_date) {
                let payment_date = cur_date.succ_opt().unwrap();
                settle_month_end_interest_by_account(
                    books,
                    state,
                    payment_date,
                    &state_index_by_account,
                    &mut pending_deltas,
                );
                settle_month_end_interest_no_account(
                    books,
                    state,
                    payment_date,
                    &state_index_by_account,
                    &mut pending_deltas,
                );
            }
        }

        cur_date = cur_date.checked_add_days(Days::new(1)).unwrap();
    }

    books.reset_interest_flag();
    Ok(())
}

fn apply_pending_delta_for_date(
    pending_deltas: &mut HashMap<Uuid, HashMap<NaiveDate, Decimal>>,
    account_id: Uuid,
    date: NaiveDate,
    balance: &mut Decimal,
) {
    if let Some(account_deltas) = pending_deltas.get_mut(&account_id) {
        if let Some(delta) = account_deltas.remove(&date) {
            *balance += delta;
        }
    }
}

fn add_transaction_and_record_deltas(
    books: &mut Books,
    transaction: Transaction,
    state_index_by_account: &HashMap<Uuid, usize>,
    pending_deltas: &mut HashMap<Uuid, HashMap<NaiveDate, Decimal>>,
) {
    if let Err(e) = books.add_transaction(transaction.clone()) {
        println!("Error adding interest transaction: {:?}", e);
        return;
    }

    for entry in &transaction.entries {
        if !state_index_by_account.contains_key(&entry.account_id) {
            continue;
        }

        let account = books.get_account(&entry.account_id).unwrap();
        let entry_delta = if entry.entry_type == account.normal_balance() {
            entry.amount
        } else {
            -entry.amount
        };

        pending_deltas
            .entry(entry.account_id)
            .or_default()
            .entry(entry.date)
            .and_modify(|delta| *delta += entry_delta)
            .or_insert(entry_delta);
    }
}

fn settle_month_end_interest_by_account(
    books: &mut Books,
    state: &mut InterestCalculationState,
    payment_date: NaiveDate,
    state_index_by_account: &HashMap<Uuid, usize>,
    pending_deltas: &mut HashMap<Uuid, HashMap<NaiveDate, Decimal>>,
) {
    let account_ids: Vec<Uuid> = state.interest_tally_by_account.keys().copied().collect();

    for account_id in account_ids {
        let balance = state.interest_tally_by_account.get(&account_id).unwrap();
        let transaction_key = (payment_date, Some(account_id));
        let rounded_balance = balance.round_dp(DECIMAL_PRECISION);

        if !state.recorded_transaction_keys.contains(&transaction_key) {
            let interest_account = books.get_account(&account_id).unwrap();
            let tx = build_interest_transaction(&state.account, &Some(interest_account.clone()), payment_date, rounded_balance);
            add_transaction_and_record_deltas(books, tx, state_index_by_account, pending_deltas);
        }

        let paid_amount = state
            .recorded_transaction_amounts
            .get(&transaction_key)
            .copied()
            .unwrap_or(rounded_balance);
        state.interest_paid += paid_amount;
        state.interest_tally_by_account.insert(account_id, dec!(0));
    }
    
    // Reset interest_paid to zero for next month's calculation
    state.interest_paid = dec!(0);
}

fn settle_month_end_interest_no_account(
    books: &mut Books,
    state: &mut InterestCalculationState,
    payment_date: NaiveDate,
    state_index_by_account: &HashMap<Uuid, usize>,
    pending_deltas: &mut HashMap<Uuid, HashMap<NaiveDate, Decimal>>,
) {
    if state.interest_tally_no_account <= dec!(0) {
        return;
    }

    let transaction_key = (payment_date, None);
    let rounded_balance = state.interest_tally_no_account.round_dp(DECIMAL_PRECISION);

    if !state.recorded_transaction_keys.contains(&transaction_key) {
        let tx = build_interest_transaction(&state.account, &None, payment_date, rounded_balance);
        add_transaction_and_record_deltas(books, tx, state_index_by_account, pending_deltas);
    }

    let paid_amount = state
        .recorded_transaction_amounts
        .get(&transaction_key)
        .copied()
        .unwrap_or(rounded_balance);
    state.interest_paid += paid_amount;
    state.interest_tally_no_account = dec!(0);
    
    // Reset interest_paid to zero for next month's calculation
    state.interest_paid = dec!(0);
}

#[cfg(test)]
mod tests {
    use chrono::{Datelike, Days, NaiveDate, Utc};
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use uuid::Uuid;

    use crate::{account::{Account, AccountType, Entry, Side, Source, Transaction, TransactionStatus}, 
        books::Books, interest::{Interest, InterestTerms, InterestType, calculate_interest_for_accounts}, schedule::ScheduleEnum};

    #[test]    
    fn calculate_loan_interest_daily() {
        let mut books = Books::build_empty("My Books");
        let mut loan_account = Account::create_new("Loan Account 1", AccountType::Liability);
        loan_account.starting_balance = dec!(9000);
        books.add_account(loan_account.clone());
        
        let mut transaction_account = Account::create_new("Transaction Account 1", AccountType::Asset);
        transaction_account.starting_balance = dec!(10000);
        books.add_account(transaction_account.clone());
        
        let _ = books.add_transaction(build_transaction(Some(transaction_account.id), Some(loan_account.id), NaiveDate::from_ymd_opt(2021, 12, 31).unwrap(), "Deposit", dec!(1000)));
        let _ = books.add_transaction(build_transaction(Some(transaction_account.id), Some(loan_account.id), NaiveDate::from_ymd_opt(2022, 1, 10).unwrap(), "Deposit", dec!(100)));
        let _ = books.add_transaction(build_transaction(Some(transaction_account.id), Some(loan_account.id), NaiveDate::from_ymd_opt(2022, 1, 10).unwrap(), "Deposit", dec!(200)));
        let _ = books.add_transaction(build_transaction(Some(loan_account.id), Some(transaction_account.id), NaiveDate::from_ymd_opt(2022, 2, 4).unwrap(), "Withdrawal", dec!(500)));

        let interest_paid = Account::create_new("Interest Earned", AccountType::Expense);        
        books.add_account(interest_paid.clone());

        let interest_terms = InterestTerms::simple(
            NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),
            dec!(0.05),
            InterestType::Daily,
            ScheduleEnum::Months,
            1,
            1,
            "Interest payment".to_string(),
            Some(interest_paid.id)
        );
        let interest = Interest::from_components(vec![interest_terms], loan_account.id);        
        calculate_interest_wrapper(&mut books, interest, NaiveDate::from_ymd_opt(2022, 2, 28).unwrap());
        let transactions = books.transactions().iter().filter(|t| t.source_type == Some(Source::Interest)).collect::<Vec<_>>();

        assert_eq!(transactions.len(), 2);
        assert_eq!(transactions[0].entries.len(), 2);
        assert_eq!(transactions[0].entries[0].account_id, interest_paid.id);
        assert_eq!(transactions[0].entries[1].account_id, loan_account.id);
        assert_eq!(transactions[0].entries[0].amount, dec!(43.37));
        assert_eq!(transactions[0].entries[0].entry_type, Side::Debit);
        assert_eq!(transactions[0].entries[1].amount, dec!(43.37));
        assert_eq!(transactions[0].entries[1].entry_type, Side::Credit);
        assert_eq!(transactions[0].entries[0].date, NaiveDate::from_ymd_opt(2022, 2, 1).unwrap());
        assert_eq!(transactions[0].entries[1].date, NaiveDate::from_ymd_opt(2022, 2, 1).unwrap());

        assert_eq!(transactions[1].entries[0].amount, dec!(37.96));
        assert_eq!(transactions[1].entries[1].amount, dec!(37.96));
        assert_eq!(transactions[1].entries[0].date, NaiveDate::from_ymd_opt(2022, 3, 1).unwrap());
        assert_eq!(transactions[1].entries[1].date, NaiveDate::from_ymd_opt(2022, 3, 1).unwrap());
    }  

    #[test]
    fn calculate_loan_interest_daily_on_starting_balance() {
        let mut books = Books::build_empty("My Books");
        let mut loan_account = Account::create_new("Loan Account 1", AccountType::Liability);
        loan_account.starting_balance = dec!(10000);
        books.add_account(loan_account.clone());
        let interest_earned = Account::create_new("Interest Paid", AccountType::Expense);        
        books.add_account(interest_earned.clone());

        let interest_terms = InterestTerms::simple(
            NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),            
            dec!(0.05),
            InterestType::Daily,
            ScheduleEnum::Months,
            1,
            1,
            "Interest payment".to_string(),
            Some(interest_earned.id)
        );
        let interest = Interest::from_components(vec![interest_terms], loan_account.id);        
        calculate_interest_wrapper(&mut books, interest, NaiveDate::from_ymd_opt(2022, 12, 31).unwrap());
let transactions = books.transactions().iter().filter(|t| t.source_type == Some(Source::Interest)).collect::<Vec<_>>();
        assert_eq!(transactions.len(), 12);
        assert_eq!(transactions[0].entries.len(), 2);        
        assert_eq!(transactions[0].entries[0].account_id, interest_earned.id);
        assert_eq!(transactions[0].entries[1].account_id, loan_account.id);
        assert_eq!(transactions[0].entries[0].amount, dec!(42.47));
        assert_eq!(transactions[0].entries[0].entry_type, Side::Debit);
        assert_eq!(transactions[0].entries[1].amount, dec!(42.47));
        assert_eq!(transactions[0].entries[1].entry_type, Side::Credit);
        assert_eq!(transactions[0].entries[0].date, NaiveDate::from_ymd_opt(2022, 2, 1).unwrap());
        assert_eq!(transactions[0].entries[1].date, NaiveDate::from_ymd_opt(2022, 2, 1).unwrap());

        assert_eq!(transactions[11].entries[0].amount, dec!(44.45));
        assert_eq!(transactions[11].entries[1].amount, dec!(44.45));
        assert_eq!(transactions[11].entries[0].date, NaiveDate::from_ymd_opt(2023, 1, 1).unwrap());
        assert_eq!(transactions[11].entries[1].date, NaiveDate::from_ymd_opt(2023, 1, 1).unwrap());
    }

    #[test]    
    fn calculate_interest_daily() {
        let mut books = Books::build_empty("My Books");
        let mut savings_account = Account::create_new("Savings Account 1", AccountType::Asset);
        savings_account.starting_balance = dec!(9000);
        books.add_account(savings_account.clone());
        
        let mut transaction_account = Account::create_new("Transaction Account 1", AccountType::Asset);
        transaction_account.starting_balance = dec!(10000);
        books.add_account(transaction_account.clone());
        
        let _ = books.add_transaction(build_transaction(Some(savings_account.id), Some(transaction_account.id), NaiveDate::from_ymd_opt(2021, 12, 31).unwrap(), "Deposit", dec!(1000)));
        let _ = books.add_transaction(build_transaction(Some(savings_account.id), Some(transaction_account.id), NaiveDate::from_ymd_opt(2022, 1, 10).unwrap(), "Deposit", dec!(100)));
        let _ = books.add_transaction(build_transaction(Some(savings_account.id), Some(transaction_account.id), NaiveDate::from_ymd_opt(2022, 1, 10).unwrap(), "Deposit", dec!(200)));
        let _ = books.add_transaction(build_transaction(Some(transaction_account.id), Some(savings_account.id), NaiveDate::from_ymd_opt(2022, 2, 4).unwrap(), "Withdrawal", dec!(500)));

        let interest_earned = Account::create_new("Interest Earned", AccountType::Revenue);        
        books.add_account(interest_earned.clone());

        let interest_terms = InterestTerms::simple(
            NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),            
            dec!(0.05),
            InterestType::Daily,
            ScheduleEnum::Months,
            1,
            1,
            "Interest payment".to_string(),
            Some(interest_earned.id)
        );
        let interest = Interest::from_components(vec![interest_terms], savings_account.id);        
        calculate_interest_wrapper(&mut books, interest, NaiveDate::from_ymd_opt(2022, 2, 28).unwrap());
        let transactions = books.transactions().iter().filter(|t| t.source_type == Some(Source::Interest)).collect::<Vec<_>>();

        assert_eq!(transactions.len(), 2);
        assert_eq!(transactions[0].entries.len(), 2);
        assert_eq!(transactions[0].entries[0].account_id, savings_account.id);
        assert_eq!(transactions[0].entries[1].account_id, interest_earned.id);
        assert_eq!(transactions[0].entries[0].amount, dec!(43.37));
        assert_eq!(transactions[0].entries[0].entry_type, Side::Debit);
        assert_eq!(transactions[0].entries[1].amount, dec!(43.37));
        assert_eq!(transactions[0].entries[1].entry_type, Side::Credit);
        assert_eq!(transactions[0].entries[0].date, NaiveDate::from_ymd_opt(2022, 2, 1).unwrap());
        assert_eq!(transactions[0].entries[1].date, NaiveDate::from_ymd_opt(2022, 2, 1).unwrap());

        assert_eq!(transactions[1].entries[0].amount, dec!(37.96));
        assert_eq!(transactions[1].entries[1].amount, dec!(37.96));
        assert_eq!(transactions[1].entries[0].date, NaiveDate::from_ymd_opt(2022, 3, 1).unwrap());
        assert_eq!(transactions[1].entries[1].date, NaiveDate::from_ymd_opt(2022, 3, 1).unwrap());
    }


    #[test]    
    fn calculate_interest_daily_starting_balance_and_transaction() {
        let mut books = Books::build_empty("My Books");
        let mut savings_account = Account::create_new("Savings Account 1", AccountType::Asset);
        savings_account.starting_balance = dec!(10000);
        books.add_account(savings_account.clone());
        
        let mut transaction_account = Account::create_new("Transaction Account 1", AccountType::Asset);
        transaction_account.starting_balance = dec!(10000);
        books.add_account(transaction_account.clone());
        

        let _ = books.add_transaction(build_transaction(Some(savings_account.id), Some(transaction_account.id), NaiveDate::from_ymd_opt(2022, 1, 10).unwrap(), "Deposit", dec!(100)));

        let interest_earned = Account::create_new("Interest Earned", AccountType::Revenue);        
        books.add_account(interest_earned.clone());

        let interest_terms = InterestTerms::simple(
            NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),
            dec!(0.05),
            InterestType::Daily,
            ScheduleEnum::Months,
            1,
            1,
            "Interest payment".to_string(),
            Some(interest_earned.id)
        );
        let interest = Interest::from_components(vec![interest_terms], savings_account.id);        
        calculate_interest_wrapper(&mut books, interest, NaiveDate::from_ymd_opt(2022, 1, 31).unwrap());
        let transactions = books.transactions().iter().filter(|t| t.source_type == Some(Source::Interest)).collect::<Vec<_>>();

        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].entries.len(), 2);
        assert_eq!(transactions[0].entries[0].account_id, savings_account.id);
        assert_eq!(transactions[0].entries[1].account_id, interest_earned.id);
        assert_eq!(transactions[0].entries[0].amount, dec!(42.77));
        assert_eq!(transactions[0].entries[0].entry_type, Side::Debit);
        assert_eq!(transactions[0].entries[1].amount, dec!(42.77));
        assert_eq!(transactions[0].entries[1].entry_type, Side::Credit);
        assert_eq!(transactions[0].entries[0].date, NaiveDate::from_ymd_opt(2022, 2, 1).unwrap());
        assert_eq!(transactions[0].entries[1].date, NaiveDate::from_ymd_opt(2022, 2, 1).unwrap());

    }



    #[test]
    fn calculate_interest_daily_on_starting_balance() {
        let mut books = Books::build_empty("My Books");
        let mut savings_account = Account::create_new("Savings Account 1", AccountType::Asset);
        savings_account.starting_balance = dec!(10000);
        books.add_account(savings_account.clone());
        let interest_earned = Account::create_new("Interest Earned", AccountType::Revenue);        
        books.add_account(interest_earned.clone());

        let interest_terms_1 = InterestTerms::from_components(
            NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),
            Some(NaiveDate::from_ymd_opt(2022, 6, 30).unwrap()),
            dec!(0.05),
            InterestType::Daily,
            None,
            None,
            ScheduleEnum::Months,
            1,
            1,
            "Interest payment".to_string(),
            Some(interest_earned.id)
        );
        let interest_terms_2 = InterestTerms::simple(
            NaiveDate::from_ymd_opt(2022, 7, 1).unwrap(),
            dec!(0.06),
            InterestType::Daily,
            ScheduleEnum::Months,
            1,
            1,
            "Interest payment".to_string(),
            Some(interest_earned.id)
        );
        let interest = Interest::from_components(vec![interest_terms_1, interest_terms_2], savings_account.id);        
        calculate_interest_wrapper(&mut books, interest, NaiveDate::from_ymd_opt(2022, 12, 31).unwrap());
        let transactions = books.transactions().iter().filter(|t| t.source_type == Some(Source::Interest)).collect::<Vec<_>>();

        assert_eq!(transactions.len(), 12);
        assert_eq!(transactions[0].entries.len(), 2);
        assert_eq!(transactions[0].entries[0].account_id, savings_account.id);
        assert_eq!(transactions[0].entries[1].account_id, interest_earned.id);
        assert_eq!(transactions[0].entries[0].amount, dec!(42.47));
        assert_eq!(transactions[0].entries[0].entry_type, Side::Debit);
        assert_eq!(transactions[0].entries[1].amount, dec!(42.47));
        assert_eq!(transactions[0].entries[1].entry_type, Side::Credit);
        assert_eq!(transactions[0].entries[0].date, NaiveDate::from_ymd_opt(2022, 2, 1).unwrap());
        assert_eq!(transactions[0].entries[1].date, NaiveDate::from_ymd_opt(2022, 2, 1).unwrap());

        assert_eq!(transactions[11].entries[0].amount, dec!(53.56));
        assert_eq!(transactions[11].entries[1].amount, dec!(53.56));
        assert_eq!(transactions[11].entries[0].date, NaiveDate::from_ymd_opt(2023, 1, 1).unwrap());
        assert_eq!(transactions[11].entries[1].date, NaiveDate::from_ymd_opt(2023, 1, 1).unwrap());

    }


    #[test]
    fn calculate_interest_daily_with_min_max_balance() {
        let mut books = Books::build_empty("My Books");
        let mut savings_account = Account::create_new("Savings Account 1", AccountType::Asset);
        savings_account.starting_balance = dec!(10000);
        books.add_account(savings_account.clone());
        
        let mut transaction_account = Account::create_new("Transaction Account 1", AccountType::Asset);
        transaction_account.starting_balance = dec!(10000);
        books.add_account(transaction_account.clone());
        
        let _ = books.add_transaction(build_transaction(Some(savings_account.id), Some(transaction_account.id), NaiveDate::from_ymd_opt(2022, 1, 10).unwrap(), "Deposit", dec!(100)));
        let _ = books.add_transaction(build_transaction(Some(savings_account.id), Some(transaction_account.id), NaiveDate::from_ymd_opt(2022, 1, 10).unwrap(), "Deposit", dec!(200)));
        let _ = books.add_transaction(build_transaction(Some(transaction_account.id), Some(savings_account.id), NaiveDate::from_ymd_opt(2022, 2, 15).unwrap(), "Withdrawal", dec!(2000)));

        let interest_earned = Account::create_new("Interest Earned", AccountType::Revenue);        
        books.add_account(interest_earned.clone());

        let interest_terms = InterestTerms::from_components(
            NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),            
            None,
            dec!(0.05),
            InterestType::Daily,
            Some(dec!(9000)),
            Some(dec!(10000)),    
            ScheduleEnum::Months,
            1,
            1,
            "Interest payment".to_string(),
            Some(interest_earned.id)
        );
        let interest = Interest::from_components(vec![interest_terms], savings_account.id);        
        calculate_interest_wrapper(&mut books, interest, NaiveDate::from_ymd_opt(2022, 2, 28).unwrap());
        let transactions = books.transactions().iter().filter(|t| t.source_type == Some(Source::Interest)).collect::<Vec<_>>();

        assert_eq!(transactions.len(), 2);
        assert_eq!(transactions[0].entries.len(), 2);
        assert_eq!(transactions[0].entries[0].account_id, savings_account.id);
        assert_eq!(transactions[0].entries[1].account_id, interest_earned.id);
        assert_eq!(transactions[0].entries[0].amount, dec!(4.25));
        assert_eq!(transactions[0].entries[0].entry_type, Side::Debit);
        assert_eq!(transactions[0].entries[1].amount, dec!(4.25));
        assert_eq!(transactions[0].entries[1].entry_type, Side::Credit);
        assert_eq!(transactions[0].entries[0].date, NaiveDate::from_ymd_opt(2022, 2, 1).unwrap());
        assert_eq!(transactions[0].entries[1].date, NaiveDate::from_ymd_opt(2022, 2, 1).unwrap());

        assert_eq!(transactions[1].entries[0].amount, dec!(1.92));
        assert_eq!(transactions[1].entries[1].amount, dec!(1.92));
        assert_eq!(transactions[1].entries[0].date, NaiveDate::from_ymd_opt(2022, 3, 1).unwrap());
        assert_eq!(transactions[1].entries[1].date, NaiveDate::from_ymd_opt(2022, 3, 1).unwrap());
    }


    #[test]
    fn calculate_tiered_interest_daily() {
        let mut books = Books::build_empty("My Books");
        let mut savings_account = Account::create_new("Savings Account 1", AccountType::Asset);
        savings_account.starting_balance = dec!(10000);
        books.add_account(savings_account.clone());
        
        let mut transaction_account = Account::create_new("Transaction Account 1", AccountType::Asset);
        transaction_account.starting_balance = dec!(10000);
        books.add_account(transaction_account.clone());
        
        let _ = books.add_transaction(build_transaction(Some(savings_account.id), Some(transaction_account.id), NaiveDate::from_ymd_opt(2022, 1, 10).unwrap(), "Deposit", dec!(100)));
        let _ = books.add_transaction(build_transaction(Some(savings_account.id), Some(transaction_account.id), NaiveDate::from_ymd_opt(2022, 1, 10).unwrap(), "Deposit", dec!(200)));
        let _ = books.add_transaction(build_transaction(Some(transaction_account.id), Some(savings_account.id), NaiveDate::from_ymd_opt(2022, 2, 15).unwrap(), "Withdrawal", dec!(2000)));

        let interest_earned = Account::create_new("Interest Earned", AccountType::Revenue);        
        books.add_account(interest_earned.clone());

        let tier_1_terms = InterestTerms::from_components(
            NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),            
            None,
            dec!(0.05),
            InterestType::Daily,
            None,
            Some(dec!(10000)),    
            ScheduleEnum::Months,
            1,
            1,
            "Interest payment".to_string(),
            Some(interest_earned.id)
        );
        let tier_2_terms = InterestTerms::from_components(
            NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),            
            None,
            dec!(0.06),
            InterestType::Daily,
            Some(dec!(10000)),
            None,    
            ScheduleEnum::Months,
            1,
            1,
            "Interest payment".to_string(),
            Some(interest_earned.id)
        );
        let interest = Interest::from_components(vec![tier_1_terms, tier_2_terms], savings_account.id);        
        calculate_interest_wrapper(&mut books, interest, NaiveDate::from_ymd_opt(2022, 2, 28).unwrap());
        let transactions = books.transactions().iter().filter(|t| t.source_type == Some(Source::Interest)).collect::<Vec<_>>();

        assert_eq!(transactions.len(), 2);
        assert_eq!(transactions[0].entries.len(), 2);
        assert_eq!(transactions[0].entries[0].account_id, savings_account.id);
        assert_eq!(transactions[0].entries[1].account_id, interest_earned.id);
        assert_eq!(transactions[0].entries[0].amount, dec!(43.55));
        assert_eq!(transactions[0].entries[0].entry_type, Side::Debit);
        assert_eq!(transactions[0].entries[1].amount, dec!(43.55));
        assert_eq!(transactions[0].entries[1].entry_type, Side::Credit);
        assert_eq!(transactions[0].entries[0].date, NaiveDate::from_ymd_opt(2022, 2, 1).unwrap());
        assert_eq!(transactions[0].entries[1].date, NaiveDate::from_ymd_opt(2022, 2, 1).unwrap());

        assert_eq!(transactions[1].entries[0].amount, dec!(35.97));
        assert_eq!(transactions[1].entries[1].amount, dec!(35.97));
        assert_eq!(transactions[1].entries[0].date, NaiveDate::from_ymd_opt(2022, 3, 1).unwrap());
        assert_eq!(transactions[1].entries[1].date, NaiveDate::from_ymd_opt(2022, 3, 1).unwrap());
    }

    // #[test]
    // fn calculate_tiered_interest_daily_paid_mid_month() {
    //     let mut books = Books::build_empty("My Books");
    //     let mut savings_account = Account::create_new("Savings Account 1", AccountType::Asset);
    //     savings_account.starting_balance = dec!(10000);
    //     books.add_account(savings_account.clone());
        
    //     let mut transaction_account = Account::create_new("Transaction Account 1", AccountType::Asset);
    //     transaction_account.starting_balance = dec!(10000);
    //     books.add_account(transaction_account.clone());
        
    //     let _ = books.add_transaction(build_transaction(Some(savings_account.id), Some(transaction_account.id), NaiveDate::from_ymd_opt(2022, 1, 10).unwrap(), "Deposit", dec!(100)));
    //     let _ = books.add_transaction(build_transaction(Some(savings_account.id), Some(transaction_account.id), NaiveDate::from_ymd_opt(2022, 1, 10).unwrap(), "Deposit", dec!(200)));
    //     let _ = books.add_transaction(build_transaction(Some(transaction_account.id), Some(savings_account.id), NaiveDate::from_ymd_opt(2022, 2, 15).unwrap(), "Withdrawal", dec!(2000)));

    //     let interest_earned = Account::create_new("Interest Earned", AccountType::Revenue);        
    //     books.add_account(interest_earned.clone());

    //     let tier_1_terms = InterestTerms::from_components(
    //         NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),            
    //         None,
    //         dec!(0.05),
    //         InterestType::Daily,
    //         None,
    //         Some(dec!(10000)),    
    //         ScheduleEnum::Months,
    //         1,
    //         7,
    //         "Interest payment".to_string(),
    //         Some(interest_earned.id)
    //     );
    //     let tier_2_terms = InterestTerms::from_components(
    //         NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),            
    //         None,
    //         dec!(0.06),
    //         InterestType::Daily,
    //         Some(dec!(10000)),
    //         None,    
    //         ScheduleEnum::Months,
    //         1,
    //         29,
    //         "Interest payment".to_string(),
    //         Some(interest_earned.id)
    //     );
    //     let interest = Interest::from_components(vec![tier_1_terms, tier_2_terms], savings_account.id);        
    //     calculate_interest_wrapper(&mut books, interest, NaiveDate::from_ymd_opt(2022, 3, 1).unwrap());
    //     let transactions = books.transactions().iter().filter(|t| t.source_type == Some(Source::Interest)).collect::<Vec<_>>();

    //     assert_eq!(transactions.len(), 4);
    //     assert_eq!(transactions[0].entries[0].amount, dec!(8.22));
    //     assert_eq!(transactions[0].entries[0].date, NaiveDate::from_ymd_opt(2022, 1, 7).unwrap());

    //     assert_eq!(transactions[1].entries[0].amount, dec!(0.80));
    //     assert_eq!(transactions[1].entries[0].date, NaiveDate::from_ymd_opt(2022, 1, 29).unwrap());

    //     assert_eq!(transactions[0].entries[0].amount, dec!(42.47));
    //     assert_eq!(transactions[0].entries[0].date, NaiveDate::from_ymd_opt(2022, 2, 7).unwrap());

    //     assert_eq!(transactions[1].entries[0].amount, dec!(0.76));
    //     assert_eq!(transactions[1].entries[0].date, NaiveDate::from_ymd_opt(2022, 3, 1).unwrap());

    // }

    #[test]
    fn calculate_interest_recalculates_from_today() {
        let mut books = Books::build_empty("My Books");

        let today = Utc::now().date_naive();
        let terms_start = today.checked_add_days(Days::new(10)).unwrap();
        let end_of_month = {
            let year = terms_start.year();
            let month = terms_start.month();
            let (next_year, next_month) = if month == 12 {
                (year + 1, 1)
            } else {
                (year, month + 1)
            };
            let first_next = NaiveDate::from_ymd_opt(next_year, next_month, 1).unwrap();
            first_next.pred_opt().unwrap()
        };

        let mut savings_account = Account::create_new("Savings", AccountType::Asset);
        savings_account.starting_balance = dec!(1000);

        let interest_account = Account::create_new("Interest Income", AccountType::Revenue);

        let interest_terms = InterestTerms::simple(
            terms_start,
            dec!(0.12),
            InterestType::Daily,
            ScheduleEnum::Months,
            1,
            1,
            "Interest payment".to_string(),
            Some(interest_account.id)
        );

        let interest = Interest::from_components(vec![interest_terms], savings_account.id);
        savings_account.interest_id = Some(interest.id);

        books.add_account(savings_account.clone());
        books.add_account(interest_account.clone());
        books.add_interest(interest.clone()).unwrap();

        let existing_date = today.checked_add_days(Days::new(5)).unwrap();
        let existing_transaction = super::build_interest_transaction(
            &savings_account,
            &Some(interest_account.clone()),
            existing_date,
            dec!(5)
        );
        let existing_id = existing_transaction.id;
        books.add_transaction(existing_transaction).unwrap();

        calculate_interest_wrapper(&mut books, interest, end_of_month);

        let interest_transactions = books.transactions_by_interest(savings_account.interest_id.unwrap(), None, None);
        assert!(interest_transactions.iter().all(|t| t.id != existing_id));
        assert!(interest_transactions.iter().any(|t| t.date() == end_of_month.succ_opt()));
    }

    #[test]
    fn calculate_interest_preserves_recorded_transactions() {
        let mut books = Books::build_empty("My Books");

        let today = Utc::now().date_naive();
        let anchor_date = today.checked_sub_days(Days::new(400)).unwrap();
        let end_of_month = {
            let year = anchor_date.year();
            let month = anchor_date.month();
            let (next_year, next_month) = if month == 12 {
                (year + 1, 1)
            } else {
                (year, month + 1)
            };
            let first_next = NaiveDate::from_ymd_opt(next_year, next_month, 1).unwrap();
            first_next.pred_opt().unwrap()
        };
        let payment_date = end_of_month.succ_opt().unwrap();

        let mut savings_account = Account::create_new("Savings", AccountType::Asset);
        savings_account.starting_balance = dec!(1000);

        let interest_account = Account::create_new("Interest Income", AccountType::Revenue);

        let interest_terms = InterestTerms::simple(
            anchor_date.checked_sub_days(Days::new(10)).unwrap(),
            dec!(0.12),
            InterestType::Daily,
            ScheduleEnum::Months,
            1,
            1,
            "Interest payment".to_string(),
            Some(interest_account.id)
        );

        let interest = Interest::from_components(vec![interest_terms], savings_account.id);
        savings_account.interest_id = Some(interest.id);

        books.add_account(savings_account.clone());
        books.add_account(interest_account.clone());
        books.add_interest(interest.clone()).unwrap();

        let mut recorded_transaction = super::build_interest_transaction(
            &savings_account,
            &Some(interest_account.clone()),
            payment_date,
            dec!(5)
        );
        recorded_transaction.status = TransactionStatus::Recorded;
        let recorded_id = recorded_transaction.id;
        books.add_transaction(recorded_transaction).unwrap();

        let projected_transaction = super::build_interest_transaction(
            &savings_account,
            &Some(interest_account.clone()),
            payment_date,
            dec!(7)
        );
        books.add_transaction(projected_transaction).unwrap();

        calculate_interest_wrapper(&mut books, interest, end_of_month);

        let interest_transactions = books.transactions_by_interest(savings_account.interest_id.unwrap(), None, None);
        assert!(interest_transactions.iter().any(|t| t.id == recorded_id));
        assert!(interest_transactions.iter().filter(|t| t.date() == Some(payment_date)).all(|t| t.status == TransactionStatus::Recorded));
    }

    #[test]
    fn calculate_interest_multi_account_different_terms() {
        let mut books = Books::build_empty("My Books");
        
        // Create first savings account with 5% interest
        let mut savings_account_1 = Account::create_new("Savings Account 1", AccountType::Asset);
        savings_account_1.starting_balance = dec!(10000);
        books.add_account(savings_account_1.clone());
        
        // Create second savings account with 3% interest
        let mut savings_account_2 = Account::create_new("Savings Account 2", AccountType::Asset);
        savings_account_2.starting_balance = dec!(5000);
        books.add_account(savings_account_2.clone());
        
        // Create interest accounts
        let interest_earned_1 = Account::create_new("Interest Earned 1", AccountType::Revenue);
        books.add_account(interest_earned_1.clone());
        let interest_earned_2 = Account::create_new("Interest Earned 2", AccountType::Revenue);
        books.add_account(interest_earned_2.clone());
        
        // Set up interest terms for first account (5% daily)
        let interest_terms_1 = InterestTerms::from_components(
            NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),
            None,
            dec!(0.05),
            InterestType::Daily,
            None,
            None,
            ScheduleEnum::Months,
            1,
            1,
            "Interest payment".to_string(),
            Some(interest_earned_1.id)
        );
        let interest_1 = Interest::from_components(vec![interest_terms_1], savings_account_1.id);
        books.add_interest(interest_1.clone()).unwrap();
        savings_account_1.interest_id = Some(interest_1.id);
        
        // Set up interest terms for second account (3% daily)
        let interest_terms_2 = InterestTerms::from_components(
            NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),
            None,
            dec!(0.03),
            InterestType::Daily,
            None,
            None,
            ScheduleEnum::Months,
            1,
            1,
            "Interest payment".to_string(),
            Some(interest_earned_2.id)
        );
        let interest_2 = Interest::from_components(vec![interest_terms_2], savings_account_2.id);
        books.add_interest(interest_2.clone()).unwrap();
        savings_account_2.interest_id = Some(interest_2.id);
        
        // Calculate interest for both accounts
        let interest_accounts = vec![savings_account_1.clone(), savings_account_2.clone()];
        calculate_interest_for_accounts(&mut books, interest_accounts, NaiveDate::from_ymd_opt(2022, 3, 31).unwrap()).unwrap();
        
        let transactions = books.transactions().iter().filter(|t| t.source_type == Some(Source::Interest)).collect::<Vec<_>>();
        
        // Should have 6 transactions total (3 months × 2 accounts)
        assert_eq!(transactions.len(), 6);
        
        // Verify transactions are split between the two interest accounts
        let account_1_transactions: Vec<_> = transactions.iter()
            .filter(|t| t.entries.iter().any(|e| e.account_id == interest_earned_1.id))
            .collect();
        let account_2_transactions: Vec<_> = transactions.iter()
            .filter(|t| t.entries.iter().any(|e| e.account_id == interest_earned_2.id))
            .collect();
        
        assert_eq!(account_1_transactions.len(), 3);
        assert_eq!(account_2_transactions.len(), 3);
        
        // Verify first account interest amounts (5% on 10000 = ~41.67 per month)
        let jan_account_1 = account_1_transactions.iter()
            .find(|t| t.date() == Some(NaiveDate::from_ymd_opt(2022, 2, 1).unwrap()))
            .unwrap();
        assert_eq!(jan_account_1.entries[0].amount, dec!(42.47));
        assert_eq!(jan_account_1.entries[1].amount, dec!(42.47));
        
        // Verify second account interest amounts (3% on 5000 = ~12.74 per month)
        let jan_account_2 = account_2_transactions.iter()
            .find(|t| t.date() == Some(NaiveDate::from_ymd_opt(2022, 2, 1).unwrap()))
            .unwrap();
        assert_eq!(jan_account_2.entries[0].amount, dec!(12.74));
        assert_eq!(jan_account_2.entries[1].amount, dec!(12.74));
        
        // Verify different interest rates produce different amounts
        assert!(jan_account_1.entries[0].amount > jan_account_2.entries[0].amount);
    }


    // This test will be more useful when the interest amounts can be paid in to the other account which in theory 
    // the interest calculation apprach should support but the model doesn't yet support it.
    #[test]
    fn calculate_interest_with_transfer() {
        let mut books = Books::build_empty("My Books");
        
        let mut savings_account_1 = Account::create_new("Savings Account 1", AccountType::Asset);
        savings_account_1.starting_balance = dec!(10000);
        books.add_account(savings_account_1.clone());
        
        let mut savings_account_2 = Account::create_new("Savings Account 2", AccountType::Asset);
        savings_account_2.starting_balance = dec!(5000);
        books.add_account(savings_account_2.clone());
                
        let interest_earned_1 = Account::create_new("Interest Earned 2", AccountType::Revenue);
        books.add_account(interest_earned_1.clone());

        let interest_earned_2 = Account::create_new("Interest Earned 1", AccountType::Revenue);
        books.add_account(interest_earned_2.clone());
        
        
        let interest_terms_1 = InterestTerms::from_components(
            NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),
            None,
            dec!(0.06),
            InterestType::Daily,
            None,
            None,
            ScheduleEnum::Months,
            1,
            1,
            "Interest payment".to_string(),
            Some(interest_earned_1.id)
        );
        let interest_1 = Interest::from_components(vec![interest_terms_1], savings_account_1.id);
        books.add_interest(interest_1.clone()).unwrap();
        savings_account_1.interest_id = Some(interest_1.id);
        
        let interest_terms_2 = InterestTerms::from_components(
            NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),
            None,
            dec!(0.04),
            InterestType::Daily,
            None,
            None,
            ScheduleEnum::Months,
            1,
            1,
            "Interest payment".to_string(),
            Some(interest_earned_2.id)
        );
        let interest_2 = Interest::from_components(vec![interest_terms_2], savings_account_2.id);
        books.add_interest(interest_2.clone()).unwrap();
        savings_account_2.interest_id = Some(interest_2.id);
        
        // Add some regular transactions to test delta timing
        let _ = books.add_transaction(build_transaction(
            Some(savings_account_1.id), 
            Some(savings_account_2.id), 
            NaiveDate::from_ymd_opt(2022, 1, 15).unwrap(), 
            "Transfer", 
            dec!(1000)
        ));
        
        // Calculate interest for both accounts for 2 months
        let interest_accounts = vec![savings_account_1.clone(), savings_account_2.clone()];
        calculate_interest_for_accounts(&mut books, interest_accounts, NaiveDate::from_ymd_opt(2022, 2, 28).unwrap()).unwrap();
        
        let transactions = books.transactions().iter().filter(|t| t.source_type == Some(Source::Interest)).collect::<Vec<_>>();
        
        // Should have 4 transactions total (2 months × 2 accounts)
        assert_eq!(transactions.len(), 4);
        
        // Verify interest transactions are created
        let jan_transactions: Vec<_> = transactions.iter()
            .filter(|t| t.date() == Some(NaiveDate::from_ymd_opt(2022, 2, 1).unwrap()))
            .collect();
        let feb_transactions: Vec<_> = transactions.iter()
            .filter(|t| t.date() == Some(NaiveDate::from_ymd_opt(2022, 3, 1).unwrap()))
            .collect();
        
        assert_eq!(jan_transactions.len(), 2);
        assert_eq!(feb_transactions.len(), 2);
        
        
        let jan_account_1_to_2 = jan_transactions.iter()
            .find(|t| t.entries.iter().any(|e| e.account_id == interest_earned_1.id))
            .unwrap();
        let jan_account_2_to_1 = jan_transactions.iter()
            .find(|t| t.entries.iter().any(|e| e.account_id == interest_earned_2.id))
            .unwrap();
        
        assert_eq!(jan_account_1_to_2.entries[1].amount, dec!(53.75)); 
        
        assert_eq!(jan_account_2_to_1.entries[1].amount, dec!(15.12)); 
        
        // Verify February calculations account for the interest payments from January
        let feb_account_1 = feb_transactions.iter()
            .find(|t| t.entries.iter().any(|e| e.account_id == savings_account_1.id))
            .unwrap();
        let feb_account_2 = feb_transactions.iter()
            .find(|t| t.entries.iter().any(|e| e.account_id == savings_account_2.id))
            .unwrap();
        
        assert_eq!(feb_account_1.entries[0].amount, dec!(50.88));
        assert_eq!(feb_account_2.entries[0].amount, dec!(12.32));        
    }

    

    #[test]
    fn test_is_end_of_month_february_non_leap() {
        let terms = InterestTerms::simple(
            NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),
            dec!(0.05),
            InterestType::Daily,
            ScheduleEnum::Months,
            1,
            31, // paid_day = 31, so paid_to_day = 30
            "Test".to_string(),
            None
        );

        // February 2022 has 28 days
        assert!(terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 2, 28).unwrap()));
        assert!(!terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 2, 27).unwrap()));
        assert!(!terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 2, 1).unwrap()));
    }

    #[test]
    fn test_is_end_of_month_february_leap() {
        let terms = InterestTerms::simple(
            NaiveDate::from_ymd_opt(2020, 1, 1).unwrap(),
            dec!(0.05),
            InterestType::Daily,
            ScheduleEnum::Months,
            1,
            31, // paid_day = 31, so paid_to_day = 30
            "Test".to_string(),
            None
        );

        // February 2020 has 29 days (leap year)
        // With paid_to_day = 30, and Feb 29 < 30, the fallback should trigger
        assert!(terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2020, 2, 29).unwrap()));
        assert!(!terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2020, 2, 28).unwrap()));
        assert!(!terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2020, 2, 27).unwrap()));
    }

    #[test]
    fn test_is_end_of_month_paid_day_1() {
        let terms = InterestTerms::simple(
            NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),
            dec!(0.05),
            InterestType::Daily,
            ScheduleEnum::Months,
            1,
            1, // paid_day = 1, so paid_to_day = 31
            "Test".to_string(),
            None
        );

        // With paid_day = 1, paid_to_day = 31, so should trigger on last day of month
        // or when next day is 1st of month
        assert!(terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 1, 31).unwrap()));
        assert!(terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 2, 28).unwrap()));
        assert!(terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 4, 30).unwrap()));
        assert!(terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2020, 2, 29).unwrap())); // leap year
        
        // Should not trigger on non-month-end days
        assert!(!terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 1, 30).unwrap()));
        assert!(!terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 2, 27).unwrap()));
        assert!(!terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 4, 29).unwrap()));
    }

    #[test]
    fn test_is_end_of_month_paid_day_15() {
        let terms = InterestTerms::simple(
            NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),
            dec!(0.05),
            InterestType::Daily,
            ScheduleEnum::Months,
            1,
            15, // paid_day = 15, so paid_to_day = 14
            "Test".to_string(),
            None
        );

        // Should trigger on 14th of month
        assert!(terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 1, 14).unwrap()));
        assert!(terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 2, 14).unwrap()));
        assert!(terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 12, 14).unwrap()));
        
        // Should not trigger on other days
        assert!(!terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 1, 13).unwrap()));
        assert!(!terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 1, 15).unwrap()));
        assert!(!terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 1, 31).unwrap()));
    }
  

    #[test]
    fn test_is_end_of_month_various_paid_days() {
        // Test different paid_day values to ensure the logic works correctly
        let test_cases = vec![
            (1, 31),  // paid_day 1 -> paid_to_day 31
            (5, 4),   // paid_day 5 -> paid_to_day 4
            (10, 9),  // paid_day 10 -> paid_to_day 9
            (20, 19), // paid_day 20 -> paid_to_day 19
            (30, 29), // paid_day 30 -> paid_to_day 29
            (31, 30), // paid_day 30 -> paid_to_day 29
            (32, 31), // paid_day 30 -> paid_to_day 29
        ];

        for (paid_day, expected_paid_to_day) in test_cases {
            let terms = InterestTerms::simple(
                NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),
                dec!(0.05),
                InterestType::Daily,
                ScheduleEnum::Months,
                1,
                paid_day,
                "Test".to_string(),
                None
            );

            // Should trigger on the expected day
            assert!(terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 1, expected_paid_to_day).unwrap()), 
                    "Failed for paid_day {} on day {}", paid_day, expected_paid_to_day);
            
            // Should not trigger on adjacent days
            if expected_paid_to_day > 1 {
                assert!(!terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 1, expected_paid_to_day - 1).unwrap()), 
                        "Should not trigger on day {} for paid_day {}", expected_paid_to_day - 1, paid_day);
            }
            if expected_paid_to_day < 31 {
                assert!(!terms.is_end_of_interest_period(NaiveDate::from_ymd_opt(2022, 1, expected_paid_to_day + 1).unwrap()), 
                        "Should not trigger on day {} for paid_day {}", expected_paid_to_day + 1, paid_day);
            }
        }
    }

    pub fn build_transaction(dr_account_id: Option<Uuid>, cr_account_id: Option<Uuid>, date: NaiveDate, description: &str, amount: Decimal) -> Transaction {
        let transaction_id = Uuid::new_v4();        
        let mut t1 = Transaction{
            id: transaction_id,
            entries: Vec::new(),
            status: TransactionStatus::Recorded,
            source_type: None,
            source_id: None,
        };

        if dr_account_id.is_some() {
            t1.entries.push(Entry{id:Uuid::new_v4(),transaction_id,date,description: description.to_string(),account_id:dr_account_id.unwrap(),
                entry_type:Side::Debit, amount,balance:None, reconciled_status: None })
        }

        if cr_account_id.is_some() {
            t1.entries.push(Entry{id:Uuid::new_v4(),transaction_id,date,description: description.to_string(),account_id:cr_account_id.unwrap(),
                entry_type:Side::Credit,amount,balance:None, reconciled_status: None })
        }
        t1
    }

    // Wraps the calculate_interest_for_accounts function for tests which used the previous per account calculator.
    fn calculate_interest_wrapper(books: &mut Books, interest: Interest, to_date: NaiveDate) {      
        let interest_id = interest.id;
        books.add_interest(interest.clone()).unwrap();
        
        let mut source_account = books.get_account(&interest.account_id).unwrap();
        source_account.interest_id = Some(interest_id);
        
        let result = calculate_interest_for_accounts(books, vec![source_account], to_date);
        if result.is_err() {
            panic!("Failed to calculate interest: {:?}", result.err().unwrap());
        }
    }

}
