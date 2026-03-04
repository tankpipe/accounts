use chrono::{Datelike, Days, NaiveDate};
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use serde::Serialize;
use uuid::Uuid;

use crate::{account::{Account, Entry, Side, Transaction, TransactionStatus}, books::{Books, BooksError}, serializer::*};
use serde::Deserialize;

pub const CALC_DECIMAL_PRECISION: u32 = 4;
pub const DECIMAL_PRECISION: u32 = 2;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum InterestType {
    Daily,
    Monthly
}


#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct InterestTerms  {
    #[serde(serialize_with = "serialize_naivedate")]
    #[serde(deserialize_with = "deserialize_naivedate")]
    pub date: NaiveDate,
    pub rate: Decimal,
    pub paid: InterestType,
    pub calculated: InterestType,    
    pub interest_account_id: Option<Uuid>
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct InterestInfo  {
    #[serde(serialize_with = "serialize_naivedate")]
    #[serde(deserialize_with = "deserialize_naivedate")]
    pub paid_to_date: NaiveDate,
    pub terms: InterestTerms,
    pub account_id: Uuid,
}



pub fn calculate_interest(books: &Books, interest_info: InterestInfo, to_date: NaiveDate) -> Result<Vec<Transaction>, BooksError> {    
    let mut transactions: Vec<Transaction> = Vec::new();
    let source_account = books.get_account(&interest_info.account_id)?;
    let interest_account: Option<Account> = if interest_info.terms.interest_account_id.is_some() {
        Some(books.get_account(&interest_info.terms.interest_account_id.unwrap())?) 
    } else {
        None
    };

    let daily_rate = interest_info.terms.rate / dec!(365);
    
    let start_date = interest_info.paid_to_date.checked_add_days(Days::new(1)).unwrap();
    let account_transactions = books.account_entries(source_account.id).unwrap();
    
    //iterate backwards through the account transactions until the paid_to_date + 1 day or the previous entry
    let previous_entry: Option<&Entry> = account_transactions.iter().rev().find(|t| t.date < start_date);
    let starting_balance: Decimal = if let Some(entry) = previous_entry {
        entry.balance.unwrap()
    } else {
        source_account.starting_balance
    };
    let mut cur_date = start_date;
    let mut balance = starting_balance;

    let start_index = account_transactions.iter().rev().position(|t| t.id == previous_entry.unwrap().id);

    let mut next_entry = if let Some(idx) = start_index {
        Some(&account_transactions[idx])
    } else {
        None
    };
    let mut cur_entry = next_entry;
    let mut interest_paid = dec!(0);
    let mut monthly_interest_tally = dec!(0);
    
    while cur_date <= to_date {        
        if let Some(mut cur_index) = start_index {
            while next_entry.unwrap().date <= cur_date && cur_index < account_transactions.len() - 2 {
                cur_index += 1;
                cur_entry = next_entry;
                next_entry = Some(&account_transactions[cur_index]);            
            }

            if let Some(entry) = cur_entry {
                if entry.date == cur_date {
                    balance = entry.balance.unwrap();
                }
            }
        }
        
        let daily_interest = (daily_rate * (balance + interest_paid)).round_dp(CALC_DECIMAL_PRECISION);
        monthly_interest_tally += daily_interest;
        
        if is_end_of_month(cur_date) {            
            let transaction = build_interest_transaction(&source_account, &interest_account, cur_date.succ_opt().unwrap(), monthly_interest_tally.round_dp(DECIMAL_PRECISION));
            transactions.push(transaction);
            interest_paid += monthly_interest_tally.round_dp(DECIMAL_PRECISION);
            monthly_interest_tally = dec!(0);

        }
        cur_date = cur_date.checked_add_days(Days::new(1)).unwrap();
    }
    Ok(transactions)
}

fn build_interest_transaction(source_account: &Account, interest_account: &Option<Account>, cur_date: NaiveDate, interest_tally: Decimal) -> Transaction {
    let transaction_id = uuid::Uuid::new_v4();
    
    let mut transaction = Transaction {
        id: transaction_id,
        entries: vec![Entry{
            id: uuid::Uuid::new_v4(),
            transaction_id: transaction_id,
            account_id: source_account.id,
            date: cur_date,
            entry_type: Side::Debit,
            amount: interest_tally,
            balance: None,
            description: "Interest payment".to_string(),
            reconciled_status: None,
        }],
        status: TransactionStatus::Projected,
        schedule_id: None,
    };

    if interest_account.as_ref().is_some() {
        transaction.entries.push(Entry{
            id: uuid::Uuid::new_v4(),
            transaction_id: transaction_id,
            account_id: interest_account.as_ref().unwrap().id,
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

    use crate::{account::{Account, AccountType, Entry, Side, Transaction, TransactionStatus}, books::Books, interest::{InterestInfo, InterestTerms, InterestType, calculate_interest}};

    #[test]
    fn calculate_interest_daily_on_starting_balance() {
        let mut books = Books::build_empty("My Books");
        let mut savings_account = Account::create_new("Savings Account 1", AccountType::Asset);
        savings_account.starting_balance = dec!(10000);
        books.add_account(savings_account.clone());
        let interest_earned = Account::create_new("Interest Earned", AccountType::Revenue);        
        books.add_account(interest_earned.clone());

        let interest_terms = InterestTerms { date: NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(), rate: dec!(0.05), paid: InterestType::Monthly, calculated: InterestType::Daily, interest_account_id: Some(interest_earned.id) };        
        let interest_info = InterestInfo { paid_to_date: NaiveDate::from_ymd_opt(2021, 12, 31).unwrap(), terms: interest_terms, account_id: savings_account.id };        
    let transactions = calculate_interest(&books, interest_info, NaiveDate::from_ymd_opt(2022, 12, 31).unwrap()).unwrap();

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

        assert_eq!(transactions[11].entries[0].amount, dec!(44.45));
        assert_eq!(transactions[11].entries[1].amount, dec!(44.45));
        assert_eq!(transactions[11].entries[0].date, NaiveDate::from_ymd_opt(2023, 1, 1).unwrap());
        assert_eq!(transactions[11].entries[1].date, NaiveDate::from_ymd_opt(2023, 1, 1).unwrap());

    }






    pub fn _build_transaction(dr_account_id: Option<Uuid>, cr_account_id: Option<Uuid>, date: NaiveDate, description: &str, amount: Decimal) -> Transaction {
        let transaction_id = Uuid::new_v4();        
        let mut t1 = Transaction{
            id: transaction_id,
            entries: Vec::new(),
            status: TransactionStatus::Recorded,
            schedule_id: None,            
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
