use chrono::Duration;
use chronoutil::shift_months;
use chronoutil::shift_years;
use rust_decimal::prelude::*;
use chrono::{NaiveDate};
use serde::Serialize;
use uuid::Uuid;
use rust_decimal_macros::dec;

use serde::Deserialize;
use crate::serializer::*;

/// Account models.

#[derive(Copy, Clone, Serialize, Deserialize, Debug, PartialEq, Eq  )]
pub enum Side {
	Debit,
	Credit
}

#[derive(Copy, Clone, PartialEq, Debug,Serialize, Deserialize)]
pub enum TransactionStatus {
	Recorded,
	Predicted,
	Reconsiled
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Transaction {
    pub id: Uuid,
    pub entries: Vec<Entry>,
}

impl Transaction {
    pub fn account_entries(&self, account_id: Uuid) -> Vec<Entry> {
        self.entries.iter()
                .filter(|e| e.account_id == account_id)
                .map(|e| e.clone())
                .collect::<Vec<Entry>>()
    }

    pub fn involves_account(&self, account_id: Uuid) -> bool {
        self.entries.iter()
                .any(|e| e.account_id == account_id)
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
    pub transaction_type: Side,
    pub amount: Decimal,
    pub status: TransactionStatus,
    pub balance:     Option<Decimal>,
    pub schedule_id: Option<Uuid>
}

impl Entry {
    pub fn set_balance(&mut self, balance: Option<Decimal>) {
        self.balance = balance;
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
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: Uuid,
	pub name: String,
	pub account_type: AccountType,
	pub balance: Decimal,
	pub starting_balance: Decimal
}

impl Account {
    pub fn create_new(name: &str, account_type: AccountType) -> Account {
        return Account{
            id: Uuid::new_v4(),
            name: name.to_string(),
            account_type,
            balance: dec!(0),
            starting_balance: dec!(0),
        }
    }

    pub fn normal_balance(&self) -> Side {
        self.account_type.normal_balance()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum ScheduleEnum{
	Days,
	Weeks,
	Months,
	Years
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub id: Uuid,
	pub name: String,
	pub period: ScheduleEnum,
	pub frequency: i64,
    #[serde(serialize_with = "serialize_naivedate")]
    #[serde(deserialize_with = "deserialize_naivedate")]
	pub start_date: NaiveDate,
    #[serde(serialize_with = "serialize_option_naivedate")]
    #[serde(deserialize_with = "deserialize_option_naivedate")]
    pub end_date: Option<NaiveDate>,
    #[serde(serialize_with = "serialize_option_naivedate")]
    #[serde(deserialize_with = "deserialize_option_naivedate")]
    pub last_date: Option<NaiveDate>,
	pub amount: Decimal,
	pub description: String,
	pub dr_account_id: Option<Uuid>,
	pub cr_account_id: Option<Uuid>
}

impl Schedule {
    pub fn schedule_next(&mut self, max_date : NaiveDate) -> Option<Transaction> {
        let next_date = self.get_next_date();

        if next_date <= max_date && (self.end_date.is_none() || next_date <= self.end_date.unwrap()) {
            let transaction_id = Uuid::new_v4();
            let mut entries: Vec<Entry> = Vec::new();
            if self.dr_account_id.is_some() {
                entries.push(self.build_entry(transaction_id, next_date, self.dr_account_id.unwrap(), Side::Debit))
            }

            if self.cr_account_id.is_some() {
                entries.push(self.build_entry(transaction_id, next_date, self.cr_account_id.unwrap(), Side::Credit))
            }

            let transaction = Transaction{
                id: transaction_id,
                entries: entries,
            };

            self.last_date = Some(next_date);
            return Some(transaction)
        }

        return None
    }

    fn build_entry(&mut self, transaction_id: Uuid, next_date: NaiveDate, account_id: Uuid, transaction_type: Side) -> Entry {
        Entry{
            id: Uuid::new_v4(),
            transaction_id: transaction_id,
            description: self.description.clone(),
            amount: self.amount.clone(),
            account_id: account_id,
            transaction_type: transaction_type,
            date:        next_date.clone(),
            status:      TransactionStatus::Predicted,
            balance:     None,
            schedule_id: Some(self.id),
        }
    }

    pub fn get_next_date(&self) -> NaiveDate {
        match self.last_date {
           Some(d) => {
                let last_date = d;
                let new_date: NaiveDate;
                match self.period {
                    ScheduleEnum::Days => new_date = last_date.checked_add_signed(Duration::days(self.frequency)).unwrap(),
                    ScheduleEnum::Weeks => new_date = last_date.checked_add_signed(Duration::days(self.frequency * 7)).unwrap(),
                    ScheduleEnum::Months => new_date = shift_months(last_date, self.frequency.try_into().unwrap()),
                    ScheduleEnum::Years => new_date = shift_years(last_date, self.frequency.try_into().unwrap()),
                }

                new_date
            },
            None => self.start_date
        }
    }
}



#[cfg(test)]
mod tests {

    use chrono::{NaiveDate};
    use rust_decimal_macros::dec;
    use uuid::Uuid;

    use crate::account::ScheduleEnum;
    use crate::account::Schedule;
    use crate::account::TransactionStatus;


    #[test]
    fn test_daily() {
        test_get_next(ScheduleEnum::Days, 3, NaiveDate::from_ymd(2022, 3, 14))
    }

    #[test]
    fn test_weekly() {
        test_get_next(ScheduleEnum::Weeks, 3, NaiveDate::from_ymd(2022, 4, 1))
    }

    #[test]
    fn test_monthly() {
        test_get_next(ScheduleEnum::Months, 3, NaiveDate::from_ymd(2022, 6, 11))
    }

    #[test]
    fn test_yearly() {
        test_get_next(ScheduleEnum::Years, 1, NaiveDate::from_ymd(2023, 3, 11))
    }

    #[test]
    fn test_multiple_monthly() {
        let mut s= build_schedule(3, ScheduleEnum::Months);
        let max_date = NaiveDate::from_ymd(2022, 11, 11);
        let mut next = s.schedule_next(max_date).unwrap();
        assert_eq!(NaiveDate::from_ymd(2022, 6, 11), next.entries[0].date);
        assert_eq!(NaiveDate::from_ymd(2022, 6, 11), s.last_date.unwrap());
        assert_eq!(s.description, next.entries[0].description);
        assert_eq!(s.amount, next.entries[0].amount);
        assert_eq!(TransactionStatus::Predicted, next.entries[0].status);
        next = s.schedule_next(max_date).unwrap();
        assert_eq!(NaiveDate::from_ymd(2022, 9, 11), next.entries[0].date);
        assert_eq!(NaiveDate::from_ymd(2022, 9, 11), s.last_date.unwrap());
        let last = s.schedule_next(max_date);
        assert!(last.is_none())
    }

    #[test]
    fn test_past_max_date() {
        let mut s= build_schedule(3, ScheduleEnum::Months);
        let max_date = NaiveDate::from_ymd(2022, 05, 11);
        let next = s.schedule_next(max_date);
        assert_eq!(true, next.is_none());
    }

    #[test]
    fn test_past_end_date() {
        let mut s= build_schedule(3, ScheduleEnum::Months);
        s.end_date = Some(NaiveDate::from_ymd(2022, 05, 11));
        let next = s.schedule_next(NaiveDate::from_ymd(2023, 05, 11));
        assert_eq!(true, next.is_none());
    }

    #[test]
    fn test_first() {
        let mut s= build_schedule(3, ScheduleEnum::Months);
        s.last_date = None;
        let max_date = NaiveDate::from_ymd(2022, 05, 11);
        let next = s.schedule_next(max_date).unwrap();
        assert_eq!(s.start_date, next.entries[0].date);
        assert_eq!(s.id, next.entries[0].schedule_id.unwrap());
    }

    fn build_schedule(frequency: i64, period: ScheduleEnum) -> Schedule {
        let s= Schedule{
            id: Uuid::new_v4(),
            name: "ST 1".to_string(),
            period,
            frequency,
            start_date:   NaiveDate::from_ymd(2022, 3, 11),
            end_date:   None,
            last_date:   Some(NaiveDate::from_ymd(2022, 3, 11)),
            amount:      dec!(100.99),
            description: "stes1".to_string(),
            dr_account_id: Some(Uuid::new_v4()),
            cr_account_id: Some(Uuid::new_v4())
        };
        return s
    }

    fn test_get_next(period: ScheduleEnum, frequency: i64, expected_date: NaiveDate) {
        let s= build_schedule(frequency, period);
        let last_at_start = s.last_date;
        let next_date = s.get_next_date();
        assert_eq!(expected_date, next_date);
        assert_eq!(last_at_start, s.last_date);
    }

}