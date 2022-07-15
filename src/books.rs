use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use uuid::Uuid;

use crate::{account::{Account, ScheduledTransaction, Transaction, AccountType}};

/// Book of accounts a.k.a The Books.

#[derive(Serialize, Deserialize)]
pub struct Books {
    accounts: HashMap<Uuid, Account>,
    scheduled_transactions: Vec<ScheduledTransaction>,
    transactions: Vec<Transaction>
}

impl Books {
    pub fn build_empty() -> Books {
        Books{accounts: HashMap::new(), scheduled_transactions: Vec::new(), transactions: Vec::new()}
    }

    pub fn add_account(&mut self, account: Account) {
        self.accounts.insert (account.id, account);
    }

    pub fn accounts(&self) -> Vec<Account> {
        let mut accounts_clone: Vec<Account> = Vec::new();
        for a in self.accounts.values() {
            accounts_clone.push(a.clone());
        }
        accounts_clone
    }
    
    pub fn add_transaction(&mut self, transaction: Transaction) -> Result<(), BooksError> {

        if !self.valid_account_id(transaction.dr_account_id) {
            return Err(BooksError::from_str("Invalid DR account"))    
        }

        if !self.valid_account_id(transaction.cr_account_id) {
            return Err(BooksError::from_str("Invalid CR account"))    
        }

        if transaction.dr_account_id.is_none() && transaction.cr_account_id.is_none() {
            return Err(BooksError::from_str("A transaction must have at least one account"))    
        }
        
        self.transactions.push(transaction);
        Ok(())            
    }

    pub fn transactions(&self) -> &[Transaction] {
        self.transactions.as_slice()
    }

    /// Get a copy of the transactions with balances for a given Account.
    pub fn account_transactions(&self, account_id: Uuid) -> Result<Vec<Transaction>, BooksError> {
        if !self.accounts.contains_key(&account_id) {
            return Err(BooksError::from_str(format!("Account not found for id {}", account_id).as_str()));
        }

        let mut account_transactions: Vec<Transaction> = Vec::new();

            for t in &self.transactions {
                if !t.dr_account_id.is_none() && t.dr_account_id.unwrap() == account_id ||
                   !t.cr_account_id.is_none() && t.cr_account_id.unwrap() == account_id {
                    let new_t = t.clone();
                    account_transactions.push(new_t);
                }
            }

            account_transactions.sort_by(|a, b| a.date.cmp(&b.date));

            let account = self.accounts.get(&account_id).unwrap();
            let mut balance = account.starting_balance;
            for t in &mut account_transactions {
                if (!t.dr_account_id.is_none() && t.dr_account_id.unwrap() == account_id && account.account_type == AccountType::Debit) || 
                   (!t.cr_account_id.is_none() && t.cr_account_id.unwrap() == account_id && account.account_type == AccountType::Credit)
                {
                    balance = balance + t.amount;
                } else  {
                    balance = balance - t.amount;                    
                }
                t.balance = Some(balance.clone());
            }

            Ok(account_transactions)
    }

    pub fn add_schedule(&mut self, schedule: ScheduledTransaction) {
        self.scheduled_transactions.push(schedule);
    }

    pub fn schedules(&self) -> &[ScheduledTransaction] {
        self.scheduled_transactions.as_slice()
    }

    fn valid_account_id(&self, id: Option<Uuid>) -> bool {
        match id {
            Some(k) => return self.accounts.contains_key(&k),
            None => return true
        }
    }
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
    use uuid::Uuid;
    use chrono::{NaiveDate};
    use rust_decimal_macros::dec;
    use crate::{account::*, books::BooksError};

    use super::Books;

    #[test]
    fn test_add_account(){
        let a = Account::create_new("test account", AccountType::Credit);
        let id1 = a.id;
        let mut b = Books::build_empty();
        b.add_account(a);

        let a2 = &b.accounts()[0];
        assert_eq!(id1, a2.id);
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
        let expected: Result<(), BooksError> = Err(BooksError { error: "Invalid CR account".to_string() });
        assert!(matches!(expected, _result));
        assert_eq!(0, (&books.transactions()).len());
    }

    #[test]
    fn test_add_transaction_invalid_cr_account() {
        let (mut books, id1, _) = setup_books();
        let t1 = build_transaction(Some(id1), Some(Uuid::new_v4()));
        let _result = books.add_transaction(t1);
        let expected: Result<(), BooksError> = Err(BooksError { error: "Invalid CR account".to_string() });
        assert!(matches!(expected, _result));
        assert_eq!(0, (&books.transactions()).len());
    }

    #[test]
    fn test_add_transaction_no_account() {
        let (mut books, _id1, _id2) = setup_books();
        let t1 = build_transaction(None, None);
        let _result = books.add_transaction(t1);
        let expected: Result<(), BooksError> = Err(BooksError { error: "A transaction must have at least one account".to_string() });
        assert!(matches!(expected, _result));
        assert_eq!(0, (&books.transactions()).len());
    }

    #[test]
    fn test_account_transactions() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd(2022, 6, 4));
        let t2 = build_transaction_with_date(None, Some(id2),NaiveDate::from_ymd(2022, 6, 5));
        let t3 = build_transaction_with_date(Some(id1), None, NaiveDate::from_ymd(2022, 7, 1));
        let t4 = build_transaction_with_date(Some(id2), Some(id1), NaiveDate::from_ymd(2022, 7, 2));
        let t1_id = t1.id;    
        let t2_id = t2.id;
        let t3_id = t3.id;    
        let t4_id = t4.id;    
        books.add_transaction(t1).unwrap();
        books.add_transaction(t2).unwrap();
        books.add_transaction(t3).unwrap();
        books.add_transaction(t4).unwrap();
        let a1_transactions = books.account_transactions(id1).unwrap();        
        assert_eq!(3, a1_transactions.len());

        let entry1 = &a1_transactions[0];
        assert_eq!(t1_id, entry1.id);
        assert_eq!(dec!(10000), entry1.balance.unwrap());

        let entry2 = &a1_transactions[1];
        assert_eq!(t3_id, entry2.id);
        assert_eq!(dec!(20000), entry2.balance.unwrap());        

        let entry3 = &a1_transactions[2];
        assert_eq!(t4_id, entry3.id);
        assert_eq!(dec!(10000), entry3.balance.unwrap());        

        let a2_transactions = books.account_transactions(id2).unwrap();        
        assert_eq!(3, a2_transactions.len());

        let entry21 = &a2_transactions[0];
        assert_eq!(t1_id, entry21.id);
        assert_eq!(dec!(-10000), entry21.balance.unwrap());

        let entry22 = &a2_transactions[1];
        assert_eq!(t2_id, entry22.id);
        assert_eq!(dec!(-20000), entry22.balance.unwrap());        

        let entry23 = &a2_transactions[2];
        assert_eq!(t4_id, entry23.id);
        assert_eq!(dec!(-10000), entry23.balance.unwrap());        


    }


    fn setup_books() -> (Books, Uuid, Uuid) {
        let mut books = Books::build_empty();
        let dr_account1 = Account::create_new("Savings Account 1", AccountType::Debit);
        let id1: Uuid = dr_account1.id;
        books.add_account(dr_account1);
        let cr_account1 = Account::create_new("Savings Account 2", AccountType::Debit);
        let id2: Uuid = cr_account1.id;
        books.add_account(cr_account1);
        (books, id1, id2)
    }

    fn build_transaction(id1: Option<Uuid>, id2: Option<Uuid>) -> Transaction {
        build_transaction_with_date(id1, id2, NaiveDate::from_ymd(2022, 6, 4))        
    }

    fn build_transaction_with_date(id1: Option<Uuid>, id2: Option<Uuid>, date: NaiveDate) -> Transaction {
        let t1 = Transaction{ 
            id: Uuid::new_v4(), 
            date, 
            description: "received moneys".to_string(), 
            dr_account_id: id1, 
            cr_account_id: id2, 
            amount: dec!(10000), 
            status: TransactionStatus::Recorded,
            balance: None };
        t1
    }

}