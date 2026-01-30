use chrono::NaiveDate;
use serde::{Serialize, Deserialize};
use uuid::Uuid;
use crate::schedule::{Modifier, Schedule};
use crate::serializer::*;

use crate::{account::{Transaction}, books::BooksError};

///

#[derive(Serialize, Deserialize)]
pub struct Scheduler {
    schedules: Vec<Schedule>,
    #[serde(serialize_with = "serialize_option_naivedate")]
    #[serde(deserialize_with = "deserialize_option_naivedate")]
    end_date: Option<NaiveDate>,
    modifiers: Vec<Modifier>,
}

impl Scheduler {

    pub fn build_empty() -> Scheduler {
        Scheduler{schedules: Vec::new(), end_date: None, modifiers: Vec::new()}
    }

    pub fn with_components(schedules: Vec<Schedule>, end_date: Option<NaiveDate>, modifiers: Vec<Modifier>) -> Scheduler {
        Scheduler { schedules, end_date, modifiers }
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
        self.modifiers.push(modifier);
    }

    pub fn get_modifier(&self, modifier_id: Uuid) -> Result<&Modifier, BooksError> {
        if let Some(index) = self.modifiers.iter().position(|m| m.id == modifier_id) {
            Ok(&self.modifiers[index])
        } else {
            Err(BooksError { error: "Modifier not found".to_string() })
        }
    }

    pub fn update_modifier(&mut self, modifier: Modifier) -> Result<(), BooksError> {
        if let Some(index) = self.modifiers.iter().position(|m| m.id == modifier.id) {
            let _old = std::mem::replace(&mut self.modifiers[index], modifier);
            Ok(())
        } else {
            Err(BooksError { error: "Modifier not found".to_string() })
        }
    }

    pub fn delete_modifier(&mut self, id: &Uuid) -> Result<(), BooksError> {
        if let Some(index) = self.modifiers.iter().position(|m| m.id == *id) {
            self.modifiers.remove(index);
            Ok(())
        } else {
            Err(BooksError { error: "Modifier not found".to_string() })
        }
    }

    pub fn modifiers(&self) -> &[Modifier] {
        self.modifiers.as_slice()
    }

    pub fn end_date(&self) -> Option<NaiveDate> {
        self.end_date.and_then(|d| Some(d.clone()))
    }

    fn generate_transactions_for_schedules(schedules: &mut [Schedule], end_date: NaiveDate) -> Vec<Transaction> {
        let mut transactions : Vec<Transaction> = Vec::new();

        for schedule in schedules.iter_mut() {
            let mut next = schedule.schedule_next(end_date);
            while next.is_some() {
                transactions.push(next.unwrap());
                next = schedule.schedule_next(end_date);
            }
        }
        transactions.sort_by(|a, b| a.entries[0].date.cmp(&b.entries[0].date));
        print!("{:?}", transactions);
        transactions
    }

    pub fn generate(&mut self, end_date: NaiveDate) -> Vec<Transaction> {
        self.end_date = Some(end_date);
        Self::generate_transactions_for_schedules(&mut self.schedules, end_date)
    }

    /// Generate transactions for a specific schedule. Does not update the scheduler (overall) end_date.
    pub fn generate_by_schedule(&mut self, end_date: NaiveDate, schedule_id: Uuid) -> Vec<Transaction> {
        let mut transactions : Vec<Transaction> = Vec::new();

        if let Some(index) = self.schedules.iter().position(|s| s.id == schedule_id) {
            let schedule_slice = &mut self.schedules[index..=index];
            transactions = Self::generate_transactions_for_schedules(schedule_slice, end_date);
        }
        transactions
    }

}




#[cfg(test)]

mod tests {
    use uuid::Uuid;
    use chrono::{NaiveDate};
    use rust_decimal_macros::dec;
    use crate::{account::*, schedule::{Schedule, ScheduleEnum, ScheduleEntry}, scheduler::Scheduler};


    #[test]
    fn test_generate() {
        let mut scheduler  = Scheduler{
            schedules: Vec::new(),
            end_date: None,
            modifiers: Vec::new()
        };
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

        assert_eq!(13, transactions.len());
        assert_eq!("st test 2", transactions[2].entries[0].description);
        assert_eq!("st test 1", transactions[4].entries[0].description);
    }

}