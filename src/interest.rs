use chrono::{Datelike, Days, NaiveDate};
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use serde::Serialize;
use uuid::Uuid;

use crate::{account::{Account, AccountType, Entry, Side, Transaction, TransactionStatus}, books::{Books, BooksError}, schedule::ScheduleEnum, serializer::*};
use serde::Deserialize;

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
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Interest  {
    pub id: Uuid,
    #[serde(serialize_with = "serialize_option_naivedate")]
    #[serde(deserialize_with = "deserialize_option_naivedate")]
    pub paid_to_date: Option<NaiveDate>,
    pub terms: Vec<InterestTerms>,
    pub account_id: Uuid,
}

impl Interest {
    pub fn from_components(paid_to_date: Option<NaiveDate>, terms: Vec<InterestTerms>, account_id: Uuid) -> Self {
        Interest {
            id: Uuid::new_v4(),
            paid_to_date,
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

pub fn calculate_interest(books: &Books, interest: Interest, to_date: NaiveDate) -> Result<Vec<Transaction>, BooksError> {    
    if interest.terms.is_empty() {
        return Ok(Vec::new());
    }
    
    let mut transactions: Vec<Transaction> = Vec::new();
    let source_account = books.get_account(&interest.account_id)?;
    
    let start_date = interest.paid_to_date.map_or_else(
        || interest.get_start_date().unwrap().clone(),
        |date| date.checked_add_days(Days::new(1)).unwrap()
    );
    let account_entries = books.account_entries(source_account.id)?;
   
    let previous_entry: Option<&Entry> = account_entries.iter().rev().find(|t| t.date < start_date);

    let starting_balance: Decimal = if let Some(entry) = previous_entry {
        entry.balance.unwrap()
    } else {
        source_account.starting_balance
    };
    
    let start_index = if let Some(prev_entry) = previous_entry {
        account_entries.iter().position(|e| e.id == prev_entry.id)
    } else if account_entries.len() > 0 {
        Some(0)
    } else {
        None
    };

    let mut next_entry = if let Some(idx) = start_index {
        Some(&account_entries[idx])
    } else {
        None
    };
    
    let mut cur_entry = next_entry;
    let mut interest_paid = dec!(0);
    let mut interest_tally_by_account: std::collections::HashMap<Uuid, Decimal> = std::collections::HashMap::new();
    let mut cur_date = start_date;
    let mut balance = starting_balance;    
    let mut cur_index  = start_index.unwrap_or(account_entries.len());
    let mut cur_terms;

    while cur_date <= to_date {        

        while next_entry.is_some() && next_entry.unwrap().date <= cur_date && cur_index < account_entries.len() {
            cur_entry = next_entry;
            cur_index += 1;                
            if cur_index < account_entries.len() {
                next_entry = Some(&account_entries[cur_index]);
            }
        }

        if let Some(entry) = cur_entry {                
            if entry.date == cur_date {
                balance = entry.balance.unwrap();
            }
        }

        cur_terms = interest.get_terms_for_date(cur_date);

        for terms in cur_terms {            
            let daily_rate = terms.rate / dec!(365);
            let min_balance = terms.min_balance.unwrap_or(dec!(0));
                        
            if balance >= min_balance {
                let interest_amount: Decimal;
                if terms.max_balance.is_some() && (balance + interest_paid) >= terms.max_balance.unwrap() {
                    interest_amount = daily_rate * (terms.max_balance.unwrap() - min_balance);
                } else {
                    interest_amount = daily_rate * (balance + interest_paid - min_balance);
                }

                let current_balance = interest_tally_by_account.entry(terms.interest_account_id.unwrap()).or_insert(dec!(0));
                let new_total = *current_balance + interest_amount;
                *current_balance = new_total;

                //println!("{}, {}, Interest amount: {}, tally {}", cur_date, balance, interest_amount, new_total);
            } 
            
        }

        if is_end_of_month(cur_date) {      

            let account_ids: Vec<Uuid> = interest_tally_by_account.keys().copied().collect();
            
            for account_id in account_ids {
                let balance = interest_tally_by_account.get(&account_id).unwrap();
                let interest_account = books.get_account(&account_id)?;
                let transaction = build_interest_transaction(&source_account, &Some(interest_account.clone()), cur_date.succ_opt().unwrap(), balance.round_dp(DECIMAL_PRECISION));
                transactions.push(transaction);
                interest_paid += balance.round_dp(DECIMAL_PRECISION);
                interest_tally_by_account.insert(account_id, dec!(0));
            }
            
        }
                
        cur_date = cur_date.checked_add_days(Days::new(1)).unwrap();
    }
    Ok(transactions)
}

fn build_interest_transaction(source_account: &Account, interest_account: &Option<Account>, cur_date: NaiveDate, interest_tally: Decimal) -> Transaction {
    let transaction_id = uuid::Uuid::new_v4();
    let is_interest_bearing = source_account.account_type == AccountType::Asset;
    
    let mut transaction = Transaction {
        id: transaction_id,
        entries: vec![],
        status: TransactionStatus::Projected,
        source_type: None,
        source_id: None,
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

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use uuid::Uuid;

    use crate::{account::{Account, AccountType, Entry, Side, Transaction, TransactionStatus}, books::Books, interest::{Interest, InterestTerms, InterestType, calculate_interest}, schedule::ScheduleEnum};

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
        let interest = Interest::from_components(Some(NaiveDate::from_ymd_opt(2021, 12, 31).unwrap()), vec![interest_terms], loan_account.id);        
        let transactions = calculate_interest(&books, interest, NaiveDate::from_ymd_opt(2022, 2, 28).unwrap()).unwrap();

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
        let interest = Interest::from_components(Some(NaiveDate::from_ymd_opt(2021, 12, 31).unwrap()), vec![interest_terms], loan_account.id);        
        let transactions = calculate_interest(&books, interest, NaiveDate::from_ymd_opt(2022, 12, 31).unwrap()).unwrap();

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
        let interest = Interest::from_components(Some(NaiveDate::from_ymd_opt(2021, 12, 31).unwrap()), vec![interest_terms], savings_account.id);        
        let transactions = calculate_interest(&books, interest, NaiveDate::from_ymd_opt(2022, 2, 28).unwrap()).unwrap();

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
        let interest = Interest::from_components(Some(NaiveDate::from_ymd_opt(2021, 12, 31).unwrap()), vec![interest_terms], savings_account.id);        
        let transactions = calculate_interest(&books, interest, NaiveDate::from_ymd_opt(2022, 1, 31).unwrap()).unwrap();

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
        let interest = Interest::from_components(Some(NaiveDate::from_ymd_opt(2021, 12, 31).unwrap()), vec![interest_terms_1, interest_terms_2], savings_account.id);        
        let transactions = calculate_interest(&books, interest, NaiveDate::from_ymd_opt(2022, 12, 31).unwrap()).unwrap();

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
        let interest = Interest::from_components(Some(NaiveDate::from_ymd_opt(2021, 12, 31).unwrap()), vec![interest_terms], savings_account.id);        
        let transactions = calculate_interest(&books, interest, NaiveDate::from_ymd_opt(2022, 2, 28).unwrap()).unwrap();

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
        let interest = Interest::from_components(Some(NaiveDate::from_ymd_opt(2021, 12, 31).unwrap()), vec![tier_1_terms, tier_2_terms], savings_account.id);        
        let transactions = calculate_interest(&books, interest, NaiveDate::from_ymd_opt(2022, 2, 28).unwrap()).unwrap();

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

}
