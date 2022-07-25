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
pub enum AccountType {
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
    #[serde(serialize_with = "serialize_naivedate")]
    #[serde(deserialize_with = "deserialize_naivedate")]
	pub date: NaiveDate,
	pub description: String,
	pub dr_account_id:   Option<Uuid>,
    pub cr_account_id:   Option<Uuid>,
	pub amount:      Decimal,
	pub status:      TransactionStatus,
    pub balance:     Option<Decimal>
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
            starting_balance: dec!(0)            
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum ScheduleEnum{
	Undefined,
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
    pub last_date: Option<NaiveDate>,
	pub amount: Decimal,
	pub description: String,
	pub dr_account_id: Option<Uuid>,
	pub cr_account_id: Option<Uuid>
}

impl Schedule {
    pub fn schedule_next(&mut self, max_date : NaiveDate) -> Option<Transaction> {
        let next_date = self.get_next_date();

        if next_date <= max_date {
            let transaction = Transaction{
                id: Uuid::new_v4(),
                description: self.description.clone(),
                amount: self.amount.clone(),
                dr_account_id: self.dr_account_id.clone(),
                cr_account_id: self.cr_account_id.clone(),
                date:        next_date.clone(),
                status:      TransactionStatus::Predicted,
                balance: None
            };

            self.last_date = Some(transaction.date);
            return Some(transaction)
        }

        return None
    }

    pub fn get_next_date(&self) -> NaiveDate {
        match self.last_date {
           Some(d) => {
                let last_date = d;                
                let new_date: NaiveDate;
                match self.period {
                    ScheduleEnum::Days => new_date = last_date.checked_add_signed(Duration::days(self.frequency)).unwrap(),
                    ScheduleEnum::Months => new_date = shift_months(last_date, self.frequency.try_into().unwrap()),
                    ScheduleEnum::Years => new_date = shift_years(last_date, self.frequency.try_into().unwrap()),
                    _ => new_date = last_date
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
        assert_eq!(NaiveDate::from_ymd(2022, 6, 11), next.date);
        assert_eq!(NaiveDate::from_ymd(2022, 6, 11), s.last_date.unwrap());
        assert_eq!(s.description, next.description);
        assert_eq!(s.amount, next.amount);
        assert_eq!(TransactionStatus::Predicted, next.status);
        next = s.schedule_next(max_date).unwrap();
        assert_eq!(NaiveDate::from_ymd(2022, 9, 11), next.date);
        assert_eq!(NaiveDate::from_ymd(2022, 9, 11), s.last_date.unwrap());
    }

    #[test]
    fn test_past_max_date() {
        let mut s= build_schedule(3, ScheduleEnum::Months);
        let max_date = NaiveDate::from_ymd(2022, 05, 11);
        let next = s.schedule_next(max_date);
        assert_eq!(true, next.is_none());
    }

    #[test]
    fn test_first() {
        let mut s= build_schedule(3, ScheduleEnum::Months);
        s.last_date = None;
        let max_date = NaiveDate::from_ymd(2022, 05, 11);
        let next = s.schedule_next(max_date).unwrap();
        assert_eq!(s.start_date, next.date);
    }
    
    fn build_schedule(frequency: i64, period: ScheduleEnum) -> Schedule {
        let s= Schedule{
            id: Uuid::new_v4(),
            name: "ST 1".to_string(),
            period,
            frequency,
            start_date:   NaiveDate::from_ymd(2022, 3, 11),
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