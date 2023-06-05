use std::{collections::HashMap, cmp::Ordering};
use chrono::{NaiveDate};
use serde::{Serialize, Deserialize};
use uuid::Uuid;

use crate::{account::{Account, Schedule, Transaction, Entry}, scheduler::{Scheduler}};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq  )]
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
    pub settings: Settings,
}

impl Books {
    pub fn generate(&mut self, end_date: NaiveDate) {
        self.transactions.append(&mut self.scheduler.generate(end_date));
        self.transactions.sort_by(|a, b| a.entries[0].date.cmp(&b.entries[0].date));
    }
}

impl Books {
    pub fn build_empty(name: &str) -> Books {
        Books{
            id: Uuid::new_v4(),
            name: name.to_string(),
            version: VERSION.to_string(),
            accounts: HashMap::new(),
            scheduler: Scheduler::build_empty(), transactions: Vec::new(),
            settings: Settings{ require_double_entry: false },
        }
    }

    pub fn add_account(&mut self, account: Account) {
        self.accounts.insert (account.id, account);
    }

    pub fn delete_account(&mut self, id: &Uuid) -> Result<(), BooksError> {
        if !self.accounts.contains_key(id) {
            return Err(BooksError::from_str(format!("Account {} not found.", id).as_str()));
        }

        if self.transactions.iter().any(|t|t.involves_account(id)) {
            return Err(BooksError::from_str(format!("Account {} can not be deleted as it has transactions.", id).as_str()));
        }

        self.accounts.remove(id);
        Ok(())
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
                return Some(Err(BooksError{ error: format!("Account not found for id: {}", e.account_id) }))
            }
        }

        if self.settings.require_double_entry && transaction.entries.len() < 2 {
            return Some(Err(BooksError::from_str("A transaction needs at least two entries (double entry required is on).")))
        } else if transaction.entries.len() < 1 {
            return Some(Err(BooksError::from_str("A transaction must have at least one entry")))
        }

        if !self.valid_account_id(Some(transaction.entries[0].account_id)) {
            return Some(Err(BooksError::from_str("Invalid Account")))
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
            Err(BooksError { error: "Transaction not found".to_string() })
        }

    }

    pub fn delete_transaction(&mut self, id: &Uuid) -> Result<(), BooksError> {
        if let Some(index) = self.transactions.iter().position(|t| t.id == *id) {
            println!("remove: {:?}", index);
            self.transactions.remove(index);
            Ok(())
        } else {
            return Err(BooksError::from_str(format!("Transaction {} not found.", id).as_str()));
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

    /// Get a copy of the transactions with balances for a given Account.
    pub fn account_entries(&self, account_id: Uuid) -> Result<Vec<Entry>, BooksError> {
        if !self.accounts.contains_key(&account_id) {
            return Err(BooksError::from_str(format!("Account not found for id {}", account_id).as_str()));
        }

        let mut account_transactions: Vec<Transaction> =
            self.transactions
                .iter()
                .filter(|t|t.involves_account(&account_id))
                .map(|t| t.clone())
                .collect();

        account_transactions.sort_by(|a, b| a.entries[0].date.cmp(&b.entries[0].date));
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
            return Err(BooksError::from_str(format!("Account not found for id {}", account_id).as_str()));
        }

        let mut account_transactions: Vec<Transaction> =
            self.transactions
                .iter()
                .filter(|t|t.involves_account(&account_id))
                .map(|t| t.clone())
                .collect();

        account_transactions.sort_by(
            |a, b| a.find_entry_by_account(&account_id).unwrap().date.cmp(&b.find_entry_by_account(&account_id).unwrap().date));
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
            return Some(Err(BooksError::from_str("A schedule must have at least one transaction entry")))
        }

        for e in schedule.entries.iter() {
            if !self.valid_account_id(Some(e.account_id)) {
                return Some(Err(BooksError::from_str(format!("Invalid account: {}", e.account_id).as_str())))
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

    pub fn schedules(&self) -> &[Schedule] {
        self.scheduler.schedules()
    }

    pub fn end_date(&self) -> Option<NaiveDate> {
        self.scheduler.end_date()
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
    use rust_decimal::Decimal;
    use uuid::Uuid;
    use chrono::{NaiveDate};
    use rust_decimal_macros::dec;
    use crate::{account::*, books::BooksError};

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
    fn test_delete_account(){
        let (mut books, id1, id2) = setup_books();
        let _result = books.delete_account(&id1);
        assert!(matches!((), _result));
        assert!(books.accounts.get(&id1).is_none());
        assert!(books.accounts.get(&id2).is_some());
    }

    #[test]
    fn test_cannot_delete_account_with_transactions(){
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction(None, Some(id1));
        books.add_transaction(t1).unwrap();
        let result = books.delete_account(&id1);
        assert_eq!(format!("Account {} can not be deleted as it has transactions.", id1).as_str(), result.err().unwrap().error);
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
        assert_eq!(format!("Account {} not found.", id  ).as_str(), result.err().unwrap().error);
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
        assert_eq!("A transaction needs at least two entries (double entry required is on).".to_string(), result.err().unwrap().error);
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
        assert_eq!("A transaction must have at least one entry".to_string(), result.err().unwrap().error);
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
        assert_eq!(format!("Transaction {} not found.", id  ).as_str(), result.err().unwrap().error);
        assert_eq!(1, books.transactions.len());
    }


    #[test]
    fn test_account_entries() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd(2022, 6, 4));
        let t2 = build_transaction_with_date(None, Some(id2),NaiveDate::from_ymd(2022, 6, 5));
        let t3 = build_transaction_with_date(Some(id1), None, NaiveDate::from_ymd(2022, 7, 1));
        let t4 = build_transaction_with_date(Some(id2), Some(id1), NaiveDate::from_ymd(2022, 7, 2));
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
        println!("{:?}", entry3);
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
    fn test_account_transaction() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd(2022, 6, 4));
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
    fn test_account_transactions() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), NaiveDate::from_ymd(2022, 6, 4));
        let t2 = build_transaction_with_date(None, Some(id2),NaiveDate::from_ymd(2022, 6, 5));
        let t3 = build_transaction_with_date(Some(id1), None, NaiveDate::from_ymd(2022, 7, 1));
        let t4 = build_transaction_with_date(Some(id2), Some(id1), NaiveDate::from_ymd(2022, 7, 2));
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
        println!("{:?}", entry3);
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
        let st1 = build_schedule_std(id1, id2, NaiveDate::from_ymd(2022, 6, 4));
        let _result = books.add_schedule(st1);
        let expected: Result<(), BooksError> = Err(BooksError { error: "Invalid CR account".to_string() });
        assert!(matches!(expected, _result));
        assert_eq!(1, (&books.schedules()).len());
    }

    #[test]
    fn test_update_schedule() {
        let (mut books, id1, id2) = setup_books();
        let st1 = build_schedule_std(id1, id2, NaiveDate::from_ymd(2022, 6, 4));
        let mut st1_copy = st1.clone();
        let _result = books.add_schedule(st1);

        st1_copy.entries[0].description = "test changed".to_string();
        let _result = books.update_schedule(st1_copy);
        assert_eq!(1, (books.schedules()).len());
        assert_eq!("test changed", books.schedules()[0].entries[0].description);
    }


    #[test]
    fn test_add_schedule_invalid_dr_account() {
        let (mut books, id1, id2) = setup_books();
        let st1 = build_schedule_std(id1, id2, NaiveDate::from_ymd(2022, 6, 4));
        let _result = books.add_schedule(st1);
        let expected: Result<(), BooksError> = Ok(());
        assert!(matches!(expected, _result));
        assert_eq!(1, (&books.schedules()).len());
    }

    #[test]
    fn test_add_schedule_invalid_cr_account() {
        let (mut books, id1, _) = setup_books();
        let st1 = build_schedule_std(id1, Uuid::new_v4(), NaiveDate::from_ymd(2022, 6, 4));
        let _result = books.add_schedule(st1);
        let expected: Result<(), BooksError> = Err(BooksError { error: "Invalid CR account".to_string() });
        assert!(matches!(expected, _result));
        assert_eq!(0, (&books.schedules()).len());
    }

    #[test]
    fn test_generate() {
        let (mut books, id1, id2) = setup_books();
        let _result = books.add_schedule(
            build_schedule(id1, id2, NaiveDate::from_ymd(2022, 3, 11), "S_1", "st test 1", dec!(100.99), 3, ScheduleEnum::Months)
        );

        let _result = books.add_schedule(
            build_schedule(id2, id1, NaiveDate::from_ymd(2022, 3, 11), "S_2", "st test 2", dec!(20.23), 45, ScheduleEnum::Days)
        );

        assert_eq!(0, books.transactions.len());
        books.generate(NaiveDate::from_ymd(2023, 3, 11));

        assert_eq!(14, books.transactions.len());
        assert_eq!("st test 2", books.transactions[2].entries[0].description);
        assert_eq!("st test 1", books.transactions[4].entries[0].description);
    }

    fn setup_books() -> (Books, Uuid, Uuid) {
        let mut books = Books::build_empty("My Books");
        let dr_account1 = Account::create_new("Savings Account 1", AccountType::Asset);
        let id1: Uuid = dr_account1.id;
        books.add_account(dr_account1);
        let cr_account1 = Account::create_new("Savings Account 2", AccountType::Asset);
        let id2: Uuid = cr_account1.id;
        books.add_account(cr_account1);
        (books, id1, id2)
    }

    fn build_transaction(id1: Option<Uuid>, id2: Option<Uuid>) -> Transaction {
        build_transaction_with_date(id1, id2, NaiveDate::from_ymd(2022, 6, 4))
    }

    fn build_transaction_with_date(dr_account_id: Option<Uuid>, cr_account_id: Option<Uuid>, date: NaiveDate) -> Transaction {
        let transaction_id = Uuid::new_v4();
        let description_str = "received moneys";
        let amount = dec!(10000);
        let mut t1 = Transaction{
            id: transaction_id,
            entries: Vec::new(),
            status: TransactionStatus::Recorded,
            schedule_id: None
        };

        if dr_account_id.is_some() {
            t1.entries.push(Entry{id:Uuid::new_v4(),transaction_id,date,description: description_str.to_string(),account_id:dr_account_id.unwrap(),
                entry_type:Side::Debit, amount,balance:None })
        }

        if cr_account_id.is_some() {
            t1.entries.push(Entry{id:Uuid::new_v4(),transaction_id,date,description: description_str.to_string(),account_id:cr_account_id.unwrap(),
                entry_type:Side::Credit,amount,balance:None })
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
                ]
        }
    }

}