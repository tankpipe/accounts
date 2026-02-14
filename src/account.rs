use chrono::NaiveDate;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use serde::Serialize;
use uuid::Uuid;

use crate::serializer::*;
use serde::Deserialize;

/// Account models.

#[derive(Copy, Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum Side {
    Debit,
    Credit,
}

impl Side {
    pub fn opposite(&self) -> Side {
        match self {
            Self::Debit => Side::Credit,
            Self::Credit => Side::Debit,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum TransactionStatus {
    Projected,
    Recorded,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Transaction {
    pub id: Uuid,
    pub entries: Vec<Entry>,
    pub status: TransactionStatus,
    pub schedule_id: Option<Uuid>,
}

impl Transaction {
    pub fn account_entries(&self, account_id: Uuid) -> Vec<Entry> {
        self.entries
            .iter()
            .filter(|e| e.account_id == account_id)
            .map(|e| e.clone())
            .collect::<Vec<Entry>>()
    }

    pub fn update_balance(&mut self, prev_balance: Decimal, account: &Account) -> Decimal {
        let mut balance = prev_balance.clone();
        for i in 0..self.entries.len() {
            if self.entries[i].account_id == account.id {
                balance = self.entries[i].update_balance(prev_balance, account);
            }
        }
        balance
    }

    pub fn involves_account(&self, account_id: &Uuid) -> bool {
        self.entries.iter().any(|e| e.account_id == *account_id)
    }

    pub fn find_entry_by_account(&self, account_id: &Uuid) -> Option<&Entry> {
        self.entries.iter().find(|e| e.account_id == *account_id)
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Entry {
    pub id: Uuid,
    pub transaction_id: Uuid,
    #[serde(serialize_with = "serialize_naivedate")]
    #[serde(deserialize_with = "deserialize_naivedate")]
    pub date: NaiveDate,
    pub description: String,
    pub account_id: Uuid,
    pub entry_type: Side,
    pub amount: Decimal,
    pub balance: Option<Decimal>,
}

impl Entry {
    pub fn set_balance(&mut self, balance: Option<Decimal>) {
        self.balance = balance;
    }

    pub fn update_balance(&mut self, prev_balance: Decimal, account: &Account) -> Decimal {
        assert_eq!(
            self.account_id, account.id,
            "Attempt made to update entry balance using incorrect account"
        );
        let mut balance = prev_balance.clone();
        if self.entry_type == account.normal_balance() {
            balance = balance + self.amount;
        } else {
            balance = balance - self.amount;
        };
        self.set_balance(Some(balance.clone()));
        balance
    }
}
pub struct Transaction2 {
    pub id: Uuid,
    pub events: Vec<Entry>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AccountCategory {
    name: String,
    normal_balance: Side,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum AccountType {
    Asset,
    Liability,
    Revenue,
    Expense,
    Equity,
}

impl AccountType {
    pub fn normal_balance(&self) -> Side {
        match *self {
            Self::Asset => Side::Debit,
            Self::Expense => Side::Debit,
            Self::Liability => Side::Credit,
            Self::Revenue => Side::Credit,
            Self::Equity => Side::Credit,
        }
    }

    pub fn order(&self) -> u8 {
        match *self {
            Self::Asset => 0,
            Self::Liability => 1,
            Self::Revenue => 2,
            Self::Expense => 3,
            Self::Equity => 4,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ReconciliationInfo {    
    #[serde(serialize_with = "serialize_naivedate")]
    #[serde(deserialize_with = "deserialize_naivedate")]
    pub date: NaiveDate,
    pub balance: Decimal,
    pub transaction_id: Uuid,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: Uuid,
    pub name: String,
    pub account_type: AccountType,
    pub balance: Decimal,
    pub starting_balance: Decimal,
    pub reconciliation_info: Option<ReconciliationInfo>,    
}

impl Account {
    pub fn create_new(name: &str, account_type: AccountType) -> Account {
        return Account {
            id: Uuid::new_v4(),
            name: name.to_string(),
            account_type,
            balance: dec!(0),
            starting_balance: dec!(0),
            reconciliation_info: None,
        };
    }

    pub fn normal_balance(&self) -> Side {
        self.account_type.normal_balance()
    }
}


#[cfg(test)]
mod tests {

    use chrono::NaiveDate;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use uuid::Uuid;

    use crate::account::TransactionStatus;

    use super::Account;
    use super::Entry;
    use super::Side;
    use super::Transaction;

    #[test]
    fn test_update_entry_balance() {
        let account1 = Account::create_new("Savings Account 1", super::AccountType::Asset);
        let transaction_id = Uuid::new_v4();
        let date = NaiveDate::from_ymd(2023, 2, 14);
        let mut entry = build_entry(
            transaction_id,
            date,
            "loan payment",
            account1.id,
            Side::Credit,
            dec!(100),
        );

        assert_eq!(dec!(300), entry.update_balance(dec!(400), &account1));
        assert_eq!(dec!(300), entry.balance.unwrap());

        assert_eq!(dec!(200), entry.update_balance(dec!(300), &account1));
        assert_eq!(dec!(200), entry.balance.unwrap());
    }

    #[test]
    #[should_panic]
    fn test_update_entry_balance_using_incorrect_account() {
        let account1 = Account::create_new("Savings Account 1", super::AccountType::Asset);
        let account2 = Account::create_new("Savings Account 2", super::AccountType::Asset);
        let transaction_id = Uuid::new_v4();
        let mut entry = build_entry(
            transaction_id,
            NaiveDate::from_ymd(2023, 2, 14),
            "loan payment",
            account1.id,
            Side::Credit,
            dec!(100),
        );
        entry.update_balance(dec!(400), &account2); // Should panic
    }

    #[test]
    fn test_update_transaction_balance() {
        let account1 = Account::create_new("Savings Account 1", super::AccountType::Asset);
        let account2 = Account::create_new("Loan 1", super::AccountType::Liability);
        let transaction_id = Uuid::new_v4();
        let date = NaiveDate::from_ymd(2023, 2, 14);
        let mut t = Transaction {
            id: transaction_id,
            entries: [].to_vec(),
            status: TransactionStatus::Recorded,
            schedule_id: None,
        };
        t.entries.push(build_entry(
            transaction_id,
            date,
            "loan payment",
            account1.id,
            Side::Credit,
            dec!(100),
        ));
        t.entries.push(build_entry(
            transaction_id,
            date,
            "loan payment",
            account2.id,
            Side::Debit,
            dec!(100),
        ));

        assert_eq!(dec!(300), t.update_balance(dec!(400), &account1));
        assert_eq!(dec!(400), t.update_balance(dec!(500), &account2));
        assert_eq!(
            dec!(300),
            t.entries
                .iter()
                .find(|e| e.account_id == account1.id)
                .unwrap()
                .balance
                .unwrap()
        );
        assert_eq!(
            dec!(400),
            t.entries
                .iter()
                .find(|e| e.account_id == account2.id)
                .unwrap()
                .balance
                .unwrap()
        );

        assert_eq!(dec!(200), t.update_balance(dec!(300), &account1));
        assert_eq!(dec!(300), t.update_balance(dec!(400), &account2));
        assert_eq!(
            dec!(200),
            t.entries
                .iter()
                .find(|e| e.account_id == account1.id)
                .unwrap()
                .balance
                .unwrap()
        );
        assert_eq!(
            dec!(300),
            t.entries
                .iter()
                .find(|e| e.account_id == account2.id)
                .unwrap()
                .balance
                .unwrap()
        );
    }

    fn build_entry(
        transaction_id: Uuid,
        date: NaiveDate,
        description: &str,
        account_id: Uuid,
        entry_type: Side,
        amount: Decimal,
    ) -> Entry {
        Entry {
            id: Uuid::new_v4(),
            transaction_id: transaction_id,
            date: date,
            description: description.to_string(),
            account_id: account_id,
            entry_type,
            amount: amount,
            balance: None,
        }
    }
}
