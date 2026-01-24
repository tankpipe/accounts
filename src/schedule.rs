use chrono::Datelike;
use chrono::Duration;
use chrono::NaiveDate;
use chronoutil::shift_months;
use chronoutil::shift_years;
use rust_decimal::prelude::*;
use serde::Serialize;
use uuid::Uuid;

use crate::serializer::*;
use serde::Deserialize;
use crate::account::{Transaction, Entry, TransactionStatus, Side};

/// Schedule models.

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ScheduleEntry {
    pub schedule_id: Uuid,
    pub description: String,
    pub account_id: Uuid,
    pub entry_type: Side,
    pub amount: Decimal,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Copy)]
pub enum ScheduleEnum {
    Days,
    Weeks,
    Months,
    Years,
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
    pub entries: Vec<ScheduleEntry>,
    pub modifier: Option<Modifier>,
}

impl Schedule {
    pub fn schedule_next(&mut self, max_date: NaiveDate) -> Option<Transaction> {
        let next_date = self.get_next_date();
        let next_modifier_date = self.get_next_modifier_date();

        if self.modifier.is_some() {
            println!(
                "next_date: {:?}, next_modifier_date: {:?}, cycles: {}",
                next_date,
                next_modifier_date.unwrap(),
                self.modifier.as_ref().unwrap().cycle_count
            );
        }

        if next_modifier_date.is_some() && next_date >= next_modifier_date.unwrap() {
            self.modifier
                .as_mut()
                .unwrap()
                .increment(next_modifier_date.unwrap());
        }

        if next_date <= max_date && (self.end_date.is_none() || next_date <= self.end_date.unwrap())
        {
            let transaction_id = Uuid::new_v4();
            let entries = self
                .entries
                .iter()
                .map(|e| self.build_entry(transaction_id, next_date, e))
                .collect();

            let transaction = Transaction {
                id: transaction_id,
                entries: entries,
                status: TransactionStatus::Projected,
                schedule_id: Some(self.id),
            };

            self.last_date = Some(next_date);
            return Some(transaction);
        }

        return None;
    }

    fn build_entry(
        &self,
        transaction_id: Uuid,
        next_date: NaiveDate,
        entry: &ScheduleEntry,
    ) -> Entry {
        let mut entry = Entry {
            id: Uuid::new_v4(),
            transaction_id: transaction_id,
            description: entry.description.clone(),
            amount: entry.amount.clone(),
            account_id: entry.account_id,
            entry_type: entry.entry_type,
            date: next_date.clone(),
            balance: None,
        };
        if self.modifier.is_some() {
            entry.amount = self.modifier.as_ref().unwrap().apply(entry.amount);
        }
        entry
    }

    pub fn get_next_date(&self) -> NaiveDate {
        match self.last_date {
            Some(d) => {
                Schedule::calculate_next_date(d, self.period, self.frequency, self.start_date)
            }
            None => self.start_date,
        }
    }

    fn calculate_next_date(
        prev_date: NaiveDate,
        period: ScheduleEnum,
        frequency: i64,
        start_date: NaiveDate,
    ) -> NaiveDate {
        let mut new_date: NaiveDate;
        match period {
            ScheduleEnum::Days => {
                new_date = prev_date
                    .checked_add_signed(Duration::days(frequency))
                    .unwrap()
            }
            ScheduleEnum::Weeks => {
                new_date = prev_date
                    .checked_add_signed(Duration::days(frequency * 7))
                    .unwrap()
            }
            ScheduleEnum::Months => {
                new_date = shift_months(prev_date, frequency.try_into().unwrap())
            }
            ScheduleEnum::Years => new_date = shift_years(prev_date, frequency.try_into().unwrap()),
        }
        println!(
            "prev_date: {:?}, new_date: {:?}, start_date: {:?}",
            prev_date, new_date, start_date
        );
        if (period == ScheduleEnum::Months || period == ScheduleEnum::Years)
            && new_date.day() < start_date.day()
        {
            let new_month = new_date.month();
            let mut result = new_date.checked_add_signed(Duration::days(1));
            while result.is_some() && result.unwrap().month() == new_month {
                new_date = result.unwrap();
                result = new_date.checked_add_signed(Duration::days(1));
            }
        }
        new_date
    }

    pub fn get_next_modifier_date(&self) -> Option<NaiveDate> {
        match &self.modifier {
            Some(modifier) => {
                let start_date = modifier.next_date.unwrap_or(modifier.start_date);
                Some(Schedule::calculate_next_date(
                    start_date,
                    modifier.period,
                    modifier.frequency,
                    modifier.start_date,
                ))
            }
            None => None,
        }
    }

    pub fn set_last_date(&mut self, last_date: NaiveDate) {
        self.last_date = Some(last_date);
    }
}


#[cfg(test)]
pub(crate) fn __calculate_next_date_test(
    prev_date: NaiveDate,
    period: ScheduleEnum,
    frequency: i64,
    start_date: NaiveDate,
) -> NaiveDate {
    Schedule::calculate_next_date(prev_date, period, frequency, start_date)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Modifier {
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
    pub next_date: Option<NaiveDate>,
    pub cycle_count: i64,
    pub amount: Decimal,
    pub percentage: Decimal,
}

impl Modifier {
    pub fn apply(&self, amount: Decimal) -> Decimal {
        let mut amount = amount;
        for _ in 0..self.cycle_count {
            amount = amount + self.amount + self.percentage * amount;
        }
        amount
    }

    pub fn increment(&mut self, new_last_date: NaiveDate) {
        self.cycle_count += 1;
        self.next_date = Some(new_last_date);
    }
}

#[cfg(test)]
mod tests {

    use chrono::NaiveDate;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use uuid::Uuid;

    use super::{Schedule, ScheduleEnum, ScheduleEntry, Modifier};
    use crate::account::TransactionStatus;

    use crate::account::{Entry, Side};

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
    fn test_ambiguous_monthly() {
        let period = ScheduleEnum::Months;
        let frequency = 1;
        let expected_date = NaiveDate::from_ymd(2023, 3, 31);
        let s = Schedule {
            id: Uuid::new_v4(),
            name: "ST 1".to_string(),
            period,
            frequency,
            start_date: NaiveDate::from_ymd(2023, 1, 31),
            end_date: None,
            last_date: Some(NaiveDate::from_ymd(2023, 2, 28)),
            entries: Vec::new(),
            modifier: None,
        };

        let last_at_start = s.last_date;
        let next_date = s.get_next_date();
        assert_eq!(expected_date, next_date);
        assert_eq!(last_at_start, s.last_date);
    }

    #[test]
    fn test_yearly() {
        test_get_next(ScheduleEnum::Years, 1, NaiveDate::from_ymd(2023, 3, 11))
    }

    #[test]
    fn test_multiple_monthly() {
        let mut s = build_schedule(3, ScheduleEnum::Months, None);
        let max_date = NaiveDate::from_ymd(2022, 11, 11);
        let mut next = s.schedule_next(max_date).unwrap();
        assert_eq!(NaiveDate::from_ymd(2022, 6, 11), next.entries[0].date);
        assert_eq!(NaiveDate::from_ymd(2022, 6, 11), s.last_date.unwrap());
        assert_eq!(s.entries[0].description, next.entries[0].description);
        assert_eq!(s.entries[0].amount, next.entries[0].amount);
        assert_eq!(TransactionStatus::Projected, next.status);
        next = s.schedule_next(max_date).unwrap();
        assert_eq!(NaiveDate::from_ymd(2022, 9, 11), next.entries[0].date);
        assert_eq!(NaiveDate::from_ymd(2022, 9, 11), s.last_date.unwrap());
        let last = s.schedule_next(max_date);
        assert!(last.is_none())
    }

    #[test]
    fn test_past_max_date() {
        let mut s = build_schedule(3, ScheduleEnum::Months, None);
        let max_date = NaiveDate::from_ymd(2022, 05, 11);
        let next = s.schedule_next(max_date);
        assert_eq!(true, next.is_none());
    }

    #[test]
    fn test_past_end_date() {
        let mut s = build_schedule(3, ScheduleEnum::Months, None);
        s.end_date = Some(NaiveDate::from_ymd(2022, 05, 11));
        let next = s.schedule_next(NaiveDate::from_ymd(2023, 05, 11));
        assert_eq!(true, next.is_none());
    }

    #[test]
    fn test_first() {
        let mut s = build_schedule(3, ScheduleEnum::Months, None);
        s.last_date = None;
        let max_date = NaiveDate::from_ymd(2022, 05, 11);
        let next = s.schedule_next(max_date).unwrap();
        assert_eq!(s.start_date, next.entries[0].date);
        assert_eq!(s.id, next.schedule_id.unwrap());
    }

    fn build_schedule(
        frequency: i64,
        period: ScheduleEnum,
        modifier: Option<Modifier>,
    ) -> Schedule {
        let mut s = Schedule {
            id: Uuid::new_v4(),
            name: "ST 1".to_string(),
            period,
            frequency,
            start_date: NaiveDate::from_ymd(2022, 3, 11),
            end_date: None,
            last_date: Some(NaiveDate::from_ymd(2022, 3, 11)),
            entries: Vec::new(),
            modifier,
        };
        s.entries.push(ScheduleEntry {
            amount: dec!(100.99),
            description: "stes1".to_string(),
            account_id: Uuid::new_v4(),
            entry_type: Side::Debit,
            schedule_id: s.id,
        });
        s.entries.push(ScheduleEntry {
            amount: dec!(100.99),
            description: "stes1".to_string(),
            account_id: Uuid::new_v4(),
            entry_type: Side::Credit,
            schedule_id: s.id,
        });
        return s;
    }

    fn test_get_next(period: ScheduleEnum, frequency: i64, expected_date: NaiveDate) {
        let s = build_schedule(frequency, period, None);
        let last_at_start = s.last_date;
        let next_date = s.get_next_date();
        assert_eq!(expected_date, next_date);
        assert_eq!(last_at_start, s.last_date);
    }

    #[test]
    fn test_daily_modifier() {
        let mut modifier = build_modifier(0, dec!(0), dec!(0));
        modifier.period = ScheduleEnum::Days;
        modifier.frequency = 10;

        test_get_next_modifier_date(Some(modifier.clone()), NaiveDate::from_ymd(2022, 1, 11));
        assert_eq!(0, modifier.cycle_count);
    }

    #[test]
    fn test_yearly_modifier() {
        let mut modifier = build_modifier(0, dec!(0), dec!(0));
        modifier.period = ScheduleEnum::Years;

        test_get_next_modifier_date(Some(modifier.clone()), NaiveDate::from_ymd(2023, 1, 1));
        assert_eq!(0, modifier.cycle_count);
    }

    #[test]
    fn test_multiple_monthly_with_modifier() {
        let modifier = Modifier {
            id: Uuid::new_v4(),
            name: "Test Modifier".to_string(),
            period: ScheduleEnum::Years,
            frequency: 1,
            start_date: NaiveDate::from_ymd(2022, 1, 1),
            end_date: None,
            next_date: Some(NaiveDate::from_ymd(2022, 1, 1)),
            cycle_count: 0,
            amount: Decimal::ZERO,
            percentage: dec!(0.10),
        };

        let mut s = build_schedule(3, ScheduleEnum::Months, Some(modifier.clone()));
        let max_date = NaiveDate::from_ymd(2023, 10, 1);

        let mut next = s.schedule_next(max_date).unwrap();
        for _ in 1..6 {
            next = s.schedule_next(max_date).unwrap();
        }

        assert_eq!(NaiveDate::from_ymd(2023, 9, 11), next.entries[0].date);
        assert_eq!(NaiveDate::from_ymd(2023, 9, 11), s.last_date.unwrap());
        assert_eq!(s.entries[0].description, next.entries[0].description);
        assert_eq!(s.entries[0].amount * dec!(1.1), next.entries[0].amount);
        assert_eq!(TransactionStatus::Projected, next.status);
    }

    fn test_get_next_modifier_date(modifier: Option<Modifier>, expected_date: NaiveDate) {
        let s = build_schedule(3, ScheduleEnum::Months, modifier);
        let last_at_start = s.last_date;
        let next_date = s.get_next_modifier_date().unwrap();
        assert_eq!(expected_date, next_date);
        assert_eq!(last_at_start, s.last_date);
    }

    // Direct tests for calculate_next_date via test-only wrapper
    #[test]
    fn test_calculate_next_date_days() {
        let prev = NaiveDate::from_ymd(2022, 3, 10);
        let start = NaiveDate::from_ymd(2022, 3, 1);
        let next = super::__calculate_next_date_test(prev, ScheduleEnum::Days, 5, start);
        assert_eq!(NaiveDate::from_ymd(2022, 3, 15), next);
    }

    #[test]
    fn test_calculate_next_date_weeks() {
        let prev = NaiveDate::from_ymd(2022, 3, 10);
        let start = NaiveDate::from_ymd(2022, 3, 1);
        let next = super::__calculate_next_date_test(prev, ScheduleEnum::Weeks, 2, start);
        assert_eq!(NaiveDate::from_ymd(2022, 3, 24), next);
    }

    #[test]
    fn test_calculate_next_date_months_regular_day() {
        let prev = NaiveDate::from_ymd(2022, 1, 15);
        let start = NaiveDate::from_ymd(2022, 1, 15);
        let next = super::__calculate_next_date_test(prev, ScheduleEnum::Months, 1, start);
        assert_eq!(NaiveDate::from_ymd(2022, 2, 15), next);
    }

    #[test]
    fn test_calculate_next_date_months_eom_from_jan_31() {
        let prev = NaiveDate::from_ymd(2022, 1, 31);
        let start = NaiveDate::from_ymd(2022, 1, 31);
        // Jan 31 + 1 month => Feb 28 (non-leap year)
        let next = super::__calculate_next_date_test(prev, ScheduleEnum::Months, 1, start);
        assert_eq!(NaiveDate::from_ymd(2022, 2, 28), next);
    }

    #[test]
    fn test_calculate_next_date_months_eom_chain_to_31st() {
        // From Feb 28 with start day 31, monthly should roll to Mar 31
        let prev = NaiveDate::from_ymd(2023, 2, 28);
        let start = NaiveDate::from_ymd(2023, 1, 31);
        let next = super::__calculate_next_date_test(prev, ScheduleEnum::Months, 1, start);
        assert_eq!(NaiveDate::from_ymd(2023, 3, 31), next);
    }

    #[test]
    fn test_calculate_next_date_years_regular() {
        let prev = NaiveDate::from_ymd(2020, 6, 15);
        let start = NaiveDate::from_ymd(2020, 6, 15);
        let next = super::__calculate_next_date_test(prev, ScheduleEnum::Years, 1, start);
        assert_eq!(NaiveDate::from_ymd(2021, 6, 15), next);
    }

    #[test]
    fn test_calculate_next_date_years_from_feb_29() {
        // Leap day + 1 year => Feb 28 of non-leap year
        let prev = NaiveDate::from_ymd(2020, 2, 29);
        let start = NaiveDate::from_ymd(2020, 2, 29);
        let next = super::__calculate_next_date_test(prev, ScheduleEnum::Years, 1, start);
        assert_eq!(NaiveDate::from_ymd(2021, 2, 28), next);
    }

    // Modifier.apply tests
    #[test]
    fn test_modifier_apply_no_cycles() {
        let m = build_modifier(0, dec!(5), dec!(0.10));        
        assert_eq!(dec!(100), m.apply(dec!(100)));
    }

    #[test]
    fn test_modifier_apply_fixed_amount() {
        let m = build_modifier(3, dec!(10), Decimal::ZERO);
        // 3 cycles add 10 each => 100 + 30 = 130
        assert_eq!(dec!(130), m.apply(dec!(100)));
    }

    #[test]
    fn test_modifier_apply_percentage_only() {
        let m = build_modifier(2, dec!(0), dec!(0.10));

        // Cycle 1: 100 + 0 + 0.1*100 = 110
        // Cycle 2: 110 + 0 + 0.1*110 = 121
        assert_eq!(dec!(121), m.apply(dec!(100)));
    }

    #[test]
    fn test_modifier_apply_fixed_and_percentage() {
        let m = build_modifier(2, dec!(5), dec!(0.10));
        // Start 100
        // After 1st: 100 + 5 + 0.1*100 = 115
        // After 2nd: 115 + 5 + 0.1*115 = 131.5
        assert_eq!(dec!(131.5), m.apply(dec!(100)));
    }

    #[test]
    fn test_modifier_apply_many_cycles() {
        let m = build_modifier(5, dec!(2), dec!(0.05));        
        // iterative expected calculation
        let mut expected = dec!(100);
        for _ in 0..5 {
            expected = expected + dec!(2) + dec!(0.05) * expected;
        }
        assert_eq!(expected, m.apply(dec!(100)));
    }

    #[test]
    fn test_modifier_apply_negative_amount() {
        let m = build_modifier(3, dec!(-4), Decimal::ZERO);
        // 3 cycles subtract 4 each => 100 - 12 = 88
        assert_eq!(dec!(88), m.apply(dec!(100)));
    }

    fn build_modifier(cycle_count: i64, amount: Decimal, percentage: Decimal) -> Modifier {
        Modifier {
            id: Uuid::new_v4(),
            name: "m".into(),
            period: ScheduleEnum::Months,
            frequency: 1,
            start_date: NaiveDate::from_ymd(2022, 1, 1),
            end_date: None,
            next_date: None,
            cycle_count,
            amount,
            percentage,
        }
    }
}
