use chrono::NaiveDate;
use serde::{Serialize, Deserialize};
use uuid::Uuid;
use crate::account::{Entry, TransactionStatus};
use crate::schedule::{Modifier, Schedule, ScheduleEntry, ScheduleModifier};
use crate::serializer::*;

use crate::{account::{Transaction}, books::BooksError};

///

#[derive(Serialize, Deserialize)]
pub struct Scheduler {
    schedules: Vec<Schedule>,
    #[serde(serialize_with = "serialize_option_naivedate")]
    #[serde(deserialize_with = "deserialize_option_naivedate")]
    end_date: Option<NaiveDate>,
    modifiers: std::collections::HashMap<Uuid, Modifier>,
}

impl Scheduler {

    pub fn build_empty() -> Scheduler {
        Scheduler{schedules: Vec::new(), end_date: None, modifiers: std::collections::HashMap::new()}
    }

    pub fn with_components(schedules: Vec<Schedule>, end_date: Option<NaiveDate>, modifiers: Vec<Modifier>) -> Scheduler {
        let mut s =Scheduler { schedules, end_date, modifiers: std::collections::HashMap::new() };
        for modifier in modifiers {
            s.modifiers.insert(modifier.id, modifier);
        }
        s
    }

    pub fn add_schedule(&mut self, schedule: Schedule) {
        self.schedules.push(schedule);
    }

    pub fn get_schedule(&self, schedule_id: Uuid) -> Result<&Schedule, BooksError> {

        if let Some(index) = self.schedules.iter().position(|s| s.id == schedule_id) {
            Ok(&self.schedules[index])
        } else {
            Err(BooksError { error: "Schedule not found".to_string() })
        }

    }

    pub fn update_schedule(&mut self, schedule: Schedule) -> Result<(), BooksError> {

        if let Some(index) = self.schedules.iter().position(|s| s.id == schedule.id) {
            let _old = std::mem::replace(&mut self.schedules[index], schedule);
            Ok(())
        } else {
            Err(BooksError { error: "Schedule not found".to_string() })
        }

    }

    pub fn delete_schedule(&mut self, id: &Uuid) -> Result<(), BooksError> {
        if let Some(index) = self.schedules.iter().position(|s| s.id == *id) {
            self.schedules.remove(index);
            Ok(())
        } else {
            Err(BooksError { error: "Schedule not found".to_string() })
        }
    }

    pub fn schedules(&self) -> &[Schedule] {
        self.schedules.as_slice()
    }

    pub fn add_modifier(&mut self, modifier: Modifier) {
        self.modifiers.insert(modifier.id, modifier);
    }

    pub fn get_modifier(&self, modifier_id: Uuid) -> Result<&Modifier, BooksError> {
        if let Some(modifier) = self.modifiers.get(&modifier_id) {
            Ok(modifier)
        } else {
            Err(BooksError { error: "Modifier not found".to_string() })
        }
    }

    pub fn update_modifier(&mut self, modifier: Modifier) -> Result<(), BooksError> {
        if self.modifiers.insert(modifier.id, modifier).is_some() {
            Ok(())
        } else {
            Err(BooksError { error: "Modifier not found".to_string() })
        }        
    }

    pub fn delete_modifier(&mut self, id: &Uuid) -> Result<(), BooksError> {
        if self.modifiers.remove(id).is_some() {
            Ok(())
        } else {
            Err(BooksError { error: "Modifier not found".to_string() })
        }
    }

    pub fn modifiers(&self) -> Vec<&Modifier> {
        self.modifiers.values().collect()
    }

    pub fn end_date(&self) -> Option<NaiveDate> {
        self.end_date.and_then(|d| Some(d.clone()))
    }

    fn generate_transactions_for_schedules(&mut self, schedule_indices: &[usize], last_date: NaiveDate) -> Vec<Transaction> {
        let mut transactions : Vec<Transaction> = Vec::new();
        let end_date_check = self.end_date;

        for &index in schedule_indices {
            let schedule = &mut self.schedules[index];
            
            loop {
                let next_date = schedule.get_next_date();
                
                // Check if we need to increment any modifiers based on next_date.
                for schedule_modifier in &mut schedule.schedule_modifiers {
                    if let Some(modifier) = self.modifiers.get(&schedule_modifier.modifier_id) {
                        let next_modifier_date = schedule_modifier.get_next_date(modifier);
                        
                        if next_date >= next_modifier_date {
                            schedule_modifier.increment(next_modifier_date);
                        }
                    }
                }
                
                // If we aren't past the last date, create a transaction
                if next_date <= last_date && (end_date_check.is_none() || next_date <= end_date_check.unwrap()) {
                    let transaction_id = Uuid::new_v4();
                    
                    // Build entries inline to avoid borrowing self
                    let mut entries = Vec::new();
                    for entry in &schedule.entries {
                        let mut built_entry = Entry {
                            id: Uuid::new_v4(),
                            transaction_id: transaction_id,
                            description: entry.description.clone(),
                            amount: entry.amount.clone(),
                            account_id: entry.account_id,
                            entry_type: entry.entry_type,
                            date: next_date.clone(),
                            balance: None,
                        };
                        
                        // Apply all modifiers in sequence
                        for schedule_modifier in &schedule.schedule_modifiers {
                            if let Some(modifier) = self.modifiers.get(&schedule_modifier.modifier_id) {
                                built_entry.amount = schedule_modifier.apply(built_entry.amount, modifier);
                            }
                        }
                        
                        entries.push(built_entry);
                    }

                    let transaction = Transaction {
                        id: transaction_id,
                        entries: entries,
                        status: TransactionStatus::Projected,
                        schedule_id: Some(schedule.id),
                    };

                    schedule.last_date = Some(next_date);
                    transactions.push(transaction);
                } else {
                    break;
                }
            }
        }
        transactions.sort_by(|a, b| a.entries[0].date.cmp(&b.entries[0].date));        
        transactions
    }

    pub fn generate(&mut self, end_date: NaiveDate) -> Vec<Transaction> {
        self.end_date = Some(end_date);
        let indices: Vec<usize> = (0..self.schedules.len()).collect();
        self.generate_transactions_for_schedules(&indices, end_date)
    }

    /// Generate transactions for a specific schedule. Does not update the scheduler (overall) end_date.
    pub fn generate_by_schedule(&mut self, end_date: NaiveDate, schedule_id: Uuid) -> Vec<Transaction> {
        let mut transactions : Vec<Transaction> = Vec::new();

        if let Some(index) = self.schedules.iter().position(|s| s.id == schedule_id) {
            transactions = self.generate_transactions_for_schedules(&[index], end_date);
        }
        transactions
    }

     pub fn schedule_next(&mut self, schedule: &mut Schedule, max_date: NaiveDate) -> Option<Transaction> {
        let next_date = schedule.get_next_date();        

        for schedule_modifier in &mut schedule.schedule_modifiers {
            let modifier = self.modifiers.get(&schedule_modifier.modifier_id);
            let next_modifier_date = schedule_modifier.get_next_date(modifier.unwrap());

            if next_date >= next_modifier_date {
                schedule_modifier.increment(next_modifier_date);
            }
        }

        if next_date <= max_date && (self.end_date.is_none() || next_date <= self.end_date.unwrap()) {
            let transaction_id = Uuid::new_v4();
            let entries = schedule
                .entries
                .iter()
                .map(|e| self.build_entry(transaction_id, next_date, e, &schedule.schedule_modifiers))
                .collect();

            let transaction = Transaction {
                id: transaction_id,
                entries: entries,
                status: TransactionStatus::Projected,
                schedule_id: Some(schedule.id),
            };

            schedule.last_date = Some(next_date);
            return Some(transaction);
        }

        return None;
    }
    
    fn build_entry(
        &self,
        transaction_id: Uuid,
        next_date: NaiveDate,
        entry: &ScheduleEntry,
        schedule_modifiers: &[ScheduleModifier]        
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
        
        // Apply all modifiers in sequence
        for schedule_modifier in schedule_modifiers {
            let modifier = self.get_modifier(schedule_modifier.modifier_id).ok()
            .expect("Modifier not found");
            entry.amount = schedule_modifier.apply(entry.amount, modifier);
        }
        
        entry
    }

}




#[cfg(test)]

mod tests {
    use rust_decimal::Decimal;
    use uuid::Uuid;
    use chrono::{NaiveDate};
    use rust_decimal_macros::dec;
    use crate::{account::*, schedule::{Modifier, Schedule, ScheduleEntry, ScheduleEnum, ScheduleModifier}, scheduler::{Scheduler}};
    use std::fs;
    use std::path::Path;
    use std::str::FromStr;


    #[test]
    fn test_generate() {
        let mut scheduler  = Scheduler::build_empty();
        
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let s_id_1 = Uuid::new_v4();
        scheduler.schedules.push(
            Schedule {
                id: s_id_1,
                name: "S_1".to_string(),
                period: ScheduleEnum::Months,
                frequency: 3,
                start_date: NaiveDate::from_ymd(2022, 3, 11),
                end_date: None,
                last_date: None,
                entries: vec![
                    ScheduleEntry {
                        amount: dec!(100.99),
                        description: "st test 1".to_string(),
                        account_id: id1,
                        entry_type: Side::Debit,
                        schedule_id: s_id_1,
                    },
                    ScheduleEntry {
                        amount: dec!(100.99),
                        description: "st test 1".to_string(),
                        account_id: id2,
                        entry_type: Side::Credit,
                        schedule_id: s_id_1,
                    }
                ],
                schedule_modifiers: vec![]
            });

        let s_id_2 = Uuid::new_v4();
        scheduler.schedules.push(
            Schedule {
                id: s_id_2,
                name: "S_2".to_string(),
                period: ScheduleEnum::Days,
                frequency: 45,
                start_date: NaiveDate::from_ymd(2022, 3, 11),
                end_date: Some(NaiveDate::from_ymd(2023, 1, 20)),
                last_date: None,
                entries: vec![
                    ScheduleEntry {
                        amount: dec!(20.23),
                        description: "st test 2".to_string(),
                        account_id: id2,
                        entry_type: Side::Debit,
                        schedule_id: s_id_2,
                    },
                    ScheduleEntry {
                        amount: dec!(100.99),
                        description: "st test 2".to_string(),
                        account_id: id1,
                        entry_type: Side::Credit,
                        schedule_id: s_id_2,
                    }
                ],
                schedule_modifiers: vec![]
            });

        let transactions = scheduler.generate(NaiveDate::from_ymd(2023, 3, 11));

        assert_eq!(14, transactions.len());
        assert_eq!("st test 2", transactions[2].entries[0].description);
        assert_eq!("st test 1", transactions[4].entries[0].description);
    }


        #[test]
    fn test_multiple_monthly() {
        let mut s = build_schedule(3, ScheduleEnum::Months, Vec::new());
        let max_date = NaiveDate::from_ymd(2022, 11, 11);
        let mut scheduler = Scheduler::build_empty();
        let mut next = scheduler.schedule_next(&mut s, max_date).unwrap();
        assert_eq!(NaiveDate::from_ymd(2022, 6, 11), next.entries[0].date);
        assert_eq!(NaiveDate::from_ymd(2022, 6, 11), s.last_date.unwrap());
        assert_eq!(s.entries[0].description, next.entries[0].description);
        assert_eq!(s.entries[0].amount, next.entries[0].amount);
        assert_eq!(TransactionStatus::Projected, next.status);
        next = scheduler.schedule_next(&mut s, max_date).unwrap();
        assert_eq!(NaiveDate::from_ymd(2022, 9, 11), next.entries[0].date);
        assert_eq!(NaiveDate::from_ymd(2022, 9, 11), s.last_date.unwrap());
        let last = scheduler.schedule_next(&mut s, max_date);
        assert!(last.is_none())
    }

    #[test]
    fn test_past_max_date() {
        let mut s = build_schedule(3, ScheduleEnum::Months, Vec::new());
        let max_date = NaiveDate::from_ymd(2022, 5, 11);
        let mut scheduler = Scheduler::with_components(Vec::new(), Some(max_date),  Vec::new());
        let next = scheduler.schedule_next(&mut s, max_date);
        assert_eq!(true, next.is_none());
    }

    #[test]
    fn test_past_end_date() {
        let mut s = build_schedule(3, ScheduleEnum::Months, Vec::new());
        s.end_date = Some(NaiveDate::from_ymd(2022, 5, 11));
        let mut scheduler = Scheduler::with_components(Vec::new(), Some(NaiveDate::from_ymd(2022, 5, 11)), Vec::new());
        let next = scheduler.schedule_next(&mut s, NaiveDate::from_ymd(2023, 5, 11));
        assert_eq!(true, next.is_none());
    }

    #[test]
    fn test_first() {
        let mut s = build_schedule(3, ScheduleEnum::Months, Vec::new());
        s.last_date = None;
        let max_date = NaiveDate::from_ymd(2022, 5, 11);
        
        // Store the properties we need to test
        let start_date = s.start_date;
        let schedule_id = s.id;
        
        // Create scheduler with empty schedules vector
        let mut scheduler = Scheduler::with_components(Vec::new(), Some(max_date), Vec::new());
        
        // Call schedule_next with the separate schedule
        let next = scheduler.schedule_next(&mut s, max_date).unwrap();
        
        // Now move the schedule into the scheduler
        scheduler.schedules.push(s);
        
        assert_eq!(start_date, next.entries[0].date);
        assert_eq!(schedule_id, next.schedule_id.unwrap());
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
            start_date: NaiveDate::from_ymd(2022, 3, 11),
            end_date: None,
            last_date: Some(NaiveDate::from_ymd(2022, 3, 11)),
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

    #[test]
    fn test_tri_monthly_with_annual_modifier() {
        let modifier = Modifier {
            id: Uuid::new_v4(),
            name: "Test Modifier".to_string(),
            period: ScheduleEnum::Years,
            frequency: 1,
            start_date: NaiveDate::from_ymd(2022, 1, 1),
            end_date: None,
            amount: Decimal::ZERO,
            percentage: dec!(0.10),
        };
        let schedule_modifier = ScheduleModifier {
            modifier_id: modifier.id,
            next_date: None,
            cycle_count: 0,
        };

        let schedule = build_schedule(3, ScheduleEnum::Months, vec![schedule_modifier]);
        let schedule_id = schedule.id;
        let mut scheduler = Scheduler::with_components(vec![schedule], None, vec![modifier]);

        assert_schedule_csv(
            &mut scheduler,
            schedule_id,
            "tri_monthly_with_annual_modifier.csv",
        );
    }

#[test]
    fn test_monthly_with_multiple_modifiers() {
        let modifier1 = Modifier {
            id: Uuid::new_v4(),
            name: "Test Modifier 1".to_string(),
            period: ScheduleEnum::Months,
            frequency: 3,
            start_date: NaiveDate::from_ymd(2022, 1, 1),
            end_date: None,
            amount: Decimal::ZERO,
            percentage: dec!(0.05),
        };
        let schedule_modifier1 = ScheduleModifier {
            modifier_id: modifier1.id,
            next_date: None,
            cycle_count: 0,
        };

        let modifier2 = Modifier {
            id: Uuid::new_v4(),
            name: "Test Modifier 2".to_string(),
            period: ScheduleEnum::Years,
            frequency: 1,
            start_date: NaiveDate::from_ymd(2022, 2, 10),
            end_date: None,
            amount: Decimal::ZERO,
            percentage: dec!(0.10),
        };
        let schedule_modifier2 = ScheduleModifier {
            modifier_id: modifier2.id,
            next_date: None,
            cycle_count: 0,
        };

        let schedule = build_schedule(
            1, ScheduleEnum::Months, vec![schedule_modifier1, schedule_modifier2]);
        let schedule_id = schedule.id;
        let mut scheduler = Scheduler::with_components(vec![schedule], None, vec![modifier1, modifier2]);

        assert_schedule_csv(
            &mut scheduler,
            schedule_id,
            "monthly_with_multiple_modifiers.csv",
        );
    }


    /// Assert the scheduler generates the expected transactions from the fixture csv.
    fn assert_schedule_csv(
        scheduler: &mut Scheduler,
        schedule_id: Uuid,
        fixture_name: &str,
    ) {
        let csv_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(fixture_name);
        let csv = fs::read_to_string(&csv_path)
            .unwrap_or_else(|err| panic!("failed to read {}: {}", csv_path.display(), err));

        for (line_index, line) in csv.lines().enumerate() {
            if line_index == 0 || line.trim().is_empty() {
                continue;
            }

            let parts: Vec<&str> = line.split(',').map(|part| part.trim()).collect();
            assert_eq!(5, parts.len(), "invalid csv row at line {}", line_index + 1);

            let to_date = NaiveDate::parse_from_str(parts[0], "%Y-%m-%d")
                .unwrap_or_else(|err| panic!("invalid to_date at line {}: {}", line_index + 1, err));
            
            
            let expected_last_tx_date: NaiveDate = NaiveDate::parse_from_str(parts[1], "%Y-%m-%d")
                .unwrap_or_else(|err| panic!("invalid last_tx_date at line {}: {}", line_index + 1, err));            
            
            let expected_last_amount: Option<Decimal> = if parts[2].is_empty() { None } else {
                Some(Decimal::from_str(parts[2])
                    .unwrap_or_else(|err| panic!("invalid last_amount at line {}: {}", line_index + 1, err)))
            };

            let expected_modifier_next_date = NaiveDate::parse_from_str(parts[3], "%Y-%m-%d")
                .unwrap_or_else(|err| panic!("invalid modifier_next_date at line {}: {}", line_index + 1, err));

            let expected_tx_count: usize = parts[4]
                .parse()
                .unwrap_or_else(|err| panic!("invalid txn_count at line {}: {}", line_index + 1, err));

            let transactions = scheduler.generate_by_schedule(to_date, schedule_id);

            assert_eq!(expected_tx_count, transactions.len());
            assert_eq!(expected_last_tx_date, scheduler.schedules[0].last_date.unwrap());
            assert_eq!(
                expected_modifier_next_date,
                scheduler.schedules[0].schedule_modifiers[0].next_date.unwrap()
            );
            if transactions.len() > 0 {
                assert_eq!(expected_last_tx_date, transactions.last().unwrap().entries[0].date);                
                assert_eq!(expected_last_amount.unwrap(), transactions.last().unwrap().entries[0].amount);
                assert_eq!(TransactionStatus::Projected, transactions.last().unwrap().status);                
            }
            
        }
    }

}
