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
use crate::account::{Side};

pub const DECIMAL_PRECISION: u32 = 4;

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
    pub schedule_modifiers: Vec<ScheduleModifier>,
}

impl Schedule {
    
    /// Calculate the next traṉsaction date for this schedule with the default being the schedule start_date.
    pub fn get_next_date(&self) -> NaiveDate {
        match self.last_date {
            Some(date) => calculate_next_date(date, self.period, self.frequency, self.start_date),
            None => self.start_date,
        }
    }

    pub fn set_last_date(&mut self, last_date: NaiveDate) {
        self.last_date = Some(last_date);
    }
}

/// Calculate the next date from the prev_date using the period and frequency.
/// For month and year, the day of the month is preserved.
/// Ambiguous month-ends are shifted backwards as necessary.
pub fn calculate_next_date(
    prev_date: NaiveDate,
    period: ScheduleEnum,
    frequency: i64,
    start_date: NaiveDate,
) -> NaiveDate {
    let mut new_date: NaiveDate;
    match period {
        ScheduleEnum::Days => new_date = prev_date.checked_add_signed(Duration::days(frequency)).unwrap(),            
        ScheduleEnum::Weeks => new_date = prev_date.checked_add_signed(Duration::days(frequency * 7)).unwrap(),
        ScheduleEnum::Months => new_date = shift_months(prev_date, frequency.try_into().unwrap()),
        ScheduleEnum::Years => new_date = shift_years(prev_date, frequency.try_into().unwrap()),
    }
    if (period == ScheduleEnum::Months || period == ScheduleEnum::Years) && new_date.day() < start_date.day() {
        let new_month = new_date.month();
        let mut result = new_date.checked_add_signed(Duration::days(1));
        while result.is_some() && result.unwrap().month() == new_month {
            new_date = result.unwrap();
            result = new_date.checked_add_signed(Duration::days(1));
        }
    }
    println!(
        "prev_date: {:?}, new_date: {:?}, start_date: {:?}",
        prev_date, new_date, start_date
    );
    new_date
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
    pub amount: Decimal,
    pub percentage: Decimal,
}

impl Modifier {
    pub fn apply(&self, amount: Decimal, cycle_count: i64) -> Decimal {
        let mut amount = amount;
        for _ in 0..cycle_count {
            amount = amount + self.amount + self.percentage * amount;
        }
        amount.round_dp(DECIMAL_PRECISION)
    }

    pub fn get_next_date(&self, prev_date: NaiveDate) -> NaiveDate {
        
        calculate_next_date(
            prev_date,
            self.period,
            self.frequency,
            self.start_date,
        )
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ScheduleModifier {
    pub modifier_id: Uuid,
    #[serde(serialize_with = "serialize_option_naivedate")]
    #[serde(deserialize_with = "deserialize_option_naivedate")]
    pub last_date: Option<NaiveDate>,
    pub cycle_count: i64,
}

impl ScheduleModifier {
    pub fn increment(&mut self, new_last_date: NaiveDate) {
        self.cycle_count += 1;
        self.last_date = Some(new_last_date);
    }

    pub fn apply(&self, amount: Decimal, modifier: &Modifier) -> Decimal {
        modifier.apply(amount, self.cycle_count)
    }

    pub fn get_next_date(&self, modifier: &Modifier) -> NaiveDate {
        let prev_date = self.last_date.unwrap_or(modifier.start_date);
        modifier.get_next_date(prev_date)
    }
}

#[cfg(test)]
mod tests {

    use chrono::NaiveDate;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use uuid::Uuid;

    use super::{Schedule, ScheduleEnum, ScheduleEntry, Modifier, calculate_next_date};
    
    use crate::account::{Side};
    use crate::schedule::{DECIMAL_PRECISION, ScheduleModifier};

    
    #[test]
    fn test_daily() {
        test_get_next(ScheduleEnum::Days, 3, NaiveDate::from_ymd_opt(2022, 3, 14).unwrap())
    }

    #[test]
    fn test_weekly() {
        test_get_next(ScheduleEnum::Weeks, 3, NaiveDate::from_ymd_opt(2022, 4, 1).unwrap())
    }

    #[test]
    fn test_monthly() {
        test_get_next(ScheduleEnum::Months, 3, NaiveDate::from_ymd_opt(2022, 6, 11).unwrap())
    }

    #[test]
    fn test_ambiguous_monthly() {
        let period = ScheduleEnum::Months;
        let frequency = 1;
        let expected_date = NaiveDate::from_ymd_opt(2023, 3, 31).unwrap();
        let s = Schedule {
            id: Uuid::new_v4(),
            name: "ST 1".to_string(),
            period,
            frequency,
            start_date: NaiveDate::from_ymd_opt(2023, 1, 31).unwrap(),
            end_date: None,
            last_date: Some(NaiveDate::from_ymd_opt(2023, 2, 28).unwrap()),
            entries: Vec::new(),
            schedule_modifiers: Vec::new(),
        };

        let last_at_start = s.last_date;
        let next_date = s.get_next_date();
        assert_eq!(expected_date, next_date);
        assert_eq!(last_at_start, s.last_date);
    }

    #[test]
    fn test_yearly() {
        test_get_next(ScheduleEnum::Years, 1, NaiveDate::from_ymd_opt(2023, 3, 11).unwrap())
    }

    fn build_schedule(
        frequency: i64,
        period: ScheduleEnum,
        schedule_modifiers: Vec<ScheduleModifier>,
    ) -> Schedule {
        
        let mut s = Schedule {
            id: Uuid::new_v4(),
            name: "ST 1".to_string(),
            period,
            frequency,
            start_date: NaiveDate::from_ymd_opt(2022, 3, 11).unwrap(),
            end_date: None,
            last_date: Some(NaiveDate::from_ymd_opt(2022, 3, 11).unwrap()),
            entries: Vec::new(),
            schedule_modifiers,
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
        let s = build_schedule(frequency, period, Vec::new());
        let last_at_start = s.last_date;
        let next_date = s.get_next_date();
        assert_eq!(expected_date, next_date);
        assert_eq!(last_at_start, s.last_date);
    }

    #[test]
    fn test_daily_modifier() {
        let mut modifier = build_modifier(dec!(0), dec!(0));
        modifier.period = ScheduleEnum::Days;
        modifier.frequency = 10;

        let schedule_modifier = ScheduleModifier {
            modifier_id: modifier.id,
            last_date: None,
            cycle_count: 0,
        };

        test_get_next_modifier_date(schedule_modifier.clone(), NaiveDate::from_ymd_opt(2022, 1, 11).unwrap(), modifier);
        assert_eq!(0, schedule_modifier.cycle_count);
    }

    #[test]
    fn test_yearly_modifier() {
        let (mut modifier,  schedule_modifier) = build_schedule_modifier(0, dec!(0), dec!(0));
        modifier.period = ScheduleEnum::Years;

        test_get_next_modifier_date(schedule_modifier.clone(), NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(), modifier);
        assert_eq!(0, schedule_modifier.cycle_count);
    }

    fn test_get_next_modifier_date(schedule_modifier: ScheduleModifier, expected_date: NaiveDate, modifier: Modifier) {
        let next_date = schedule_modifier.get_next_date(&modifier);
        assert_eq!(expected_date, next_date);
    }

    // Direct tests for calculate_next_date via test-only wrapper
    #[test]
    fn test_calculate_next_date_days() {
        let prev = NaiveDate::from_ymd_opt(2022, 3, 10).unwrap();
        let start = NaiveDate::from_ymd_opt(2022, 3, 1).unwrap();
        let next = calculate_next_date(prev, ScheduleEnum::Days, 5, start);
        assert_eq!(NaiveDate::from_ymd_opt(2022, 3, 15).unwrap(), next);
    }

    #[test]
    fn test_calculate_next_date_weeks() {
        let prev = NaiveDate::from_ymd_opt(2022, 3, 10).unwrap();
        let start = NaiveDate::from_ymd_opt(2022, 3, 1).unwrap();
        let next = calculate_next_date(prev, ScheduleEnum::Weeks, 2, start);
        assert_eq!(NaiveDate::from_ymd_opt(2022, 3, 24).unwrap(), next);
    }

    #[test]
    fn test_calculate_next_date_months_regular_day() {
        let prev = NaiveDate::from_ymd_opt(2022, 1, 15).unwrap();
        let start = NaiveDate::from_ymd_opt(2022, 1, 15).unwrap();
        let next = calculate_next_date(prev, ScheduleEnum::Months, 1, start);
        assert_eq!(NaiveDate::from_ymd_opt(2022, 2, 15).unwrap(), next);
    }

    #[test]
    fn test_calculate_next_date_months_eom_from_jan_31() {
        let prev = NaiveDate::from_ymd_opt(2022, 1, 31).unwrap();
        let start = NaiveDate::from_ymd_opt(2022, 1, 31).unwrap();
        // Jan 31 + 1 month => Feb 28 (non-leap year)
        let next = calculate_next_date(prev, ScheduleEnum::Months, 1, start);
        assert_eq!(NaiveDate::from_ymd_opt(2022, 2, 28).unwrap(), next);
    }

    #[test]
    fn test_calculate_next_date_months_eom_chain_to_31st() {
        // From Feb 28 with start day 31, monthly should roll to Mar 31
        let prev = NaiveDate::from_ymd_opt(2023, 2, 28).unwrap();
        let start = NaiveDate::from_ymd_opt(2023, 1, 31).unwrap();
        let next = calculate_next_date(prev, ScheduleEnum::Months, 1, start);
        assert_eq!(NaiveDate::from_ymd_opt(2023, 3, 31).unwrap(), next);
    }

    #[test]
    fn test_calculate_next_date_years_regular() {
        let prev = NaiveDate::from_ymd_opt(2020, 6, 15).unwrap();
        let start = NaiveDate::from_ymd_opt(2020, 6, 15).unwrap();
        let next = calculate_next_date(prev, ScheduleEnum::Years, 1, start);
        assert_eq!(NaiveDate::from_ymd_opt(2021, 6, 15).unwrap(), next);
    }

    #[test]
    fn test_calculate_next_date_years_from_feb_29() {
        // Leap day + 1 year => Feb 28 of non-leap year
        let prev = NaiveDate::from_ymd_opt(2020, 2, 29).unwrap();
        let start = NaiveDate::from_ymd_opt(2020, 2, 29).unwrap();
        let next = calculate_next_date(prev, ScheduleEnum::Years, 1, start);
        assert_eq!(NaiveDate::from_ymd_opt(2021, 2, 28).unwrap(), next);
    }

    // Modifier.apply tests
    #[test]
    fn test_modifier_apply_no_cycles() {
        let m = build_modifier(dec!(5), dec!(0.10));        
        assert_eq!(dec!(100), m.apply(dec!(100), 0));
    }

    #[test]
    fn test_modifier_apply_fixed_amount() {
        let m = build_modifier(dec!(10), Decimal::ZERO);
        // 3 cycles add 10 each => 100 + 30 = 130
        assert_eq!(dec!(130), m.apply(dec!(100), 3));
    }

    #[test]
    fn test_modifier_apply_percentage_only() {
        let m = build_modifier(dec!(0), dec!(0.10));

        // Cycle 1: 100 + 0 + 0.1*100 = 110
        // Cycle 2: 110 + 0 + 0.1*110 = 121
        assert_eq!(dec!(121), m.apply(dec!(100), 2));
    }

    #[test]
    fn test_modifier_apply_fixed_and_percentage() {
        let m = build_modifier(dec!(5), dec!(0.10));
        // Start 100
        // After 1st: 100 + 5 + 0.1*100 = 115
        // After 2nd: 115 + 5 + 0.1*115 = 131.5
        assert_eq!(dec!(131.5), m.apply(dec!(100), 2));
    }

    #[test]
    fn test_modifier_apply_many_cycles() {
        let m = build_modifier(dec!(2), dec!(0.05));        
        // iterative expected calculation
        let mut expected = dec!(100);
        for _ in 0..5 {
            expected = expected + dec!(2) + dec!(0.05) * expected;
        }
        assert_eq!(expected.round_dp(DECIMAL_PRECISION), m.apply(dec!(100), 5));
    }

    #[test]
    fn test_modifier_apply_negative_amount() {
        let m = build_modifier(dec!(-4), Decimal::ZERO);
        // 3 cycles subtract 4 each => 100 - 12 = 88
        assert_eq!(dec!(88), m.apply(dec!(100), 3));
    }

    fn build_schedule_modifier(cycle_count: i64, amount: Decimal, percentage: Decimal) -> (Modifier, ScheduleModifier) {
        let m = build_modifier(amount, percentage);
        let modifier_id = m.id.clone();
        (m, ScheduleModifier {
            modifier_id,
            last_date: None,
            cycle_count,
        })
    }

    fn build_modifier(amount: Decimal, percentage: Decimal) -> Modifier {
        Modifier {
            id: Uuid::new_v4(),
            name: "m".into(),
            period: ScheduleEnum::Months,
            frequency: 1,
            start_date: NaiveDate::from_ymd_opt(2022, 1, 1).unwrap(),
            end_date: None,
            amount,
            percentage,
        }
    }
}
