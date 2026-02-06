use std::{collections::HashMap};
use chrono::{NaiveDate};
use serde::{Serialize, Deserialize};
use uuid::Uuid;

use crate::{account::{Account, Transaction}, books::{Books, Settings}, schedule::{ScheduleEntry, ScheduleEnum}, scheduler::Scheduler};
use crate::schedule::{Schedule};
use crate::serializer::*;

const VERSION: &str = env!("CARGO_PKG_VERSION");


/// Older version of Books struct for upgrading old files
#[derive(Serialize, Deserialize)]
pub struct BooksV004 {
    pub id: Uuid,
    pub name: String,
    pub version: String,
    pub accounts: HashMap<Uuid, Account>,
    pub scheduler: SchedulerV004,
    pub transactions: Vec<Transaction>,
    pub settings: Settings,
}

impl Into<Books> for BooksV004 {
    fn into(self) -> Books{
        Books::with_components(
            self.id,
            self.name,
            VERSION.to_string(),
            self.accounts,
            self.scheduler.into(),
            self.transactions,
            self.settings,
        )
    }
}


#[derive(Serialize, Deserialize)]
pub struct SchedulerV004 {
    pub schedules: Vec<ScheduleV004>,
    #[serde(serialize_with = "serialize_option_naivedate")]
    #[serde(deserialize_with = "deserialize_option_naivedate")]
    pub end_date: Option<NaiveDate>,
}

impl Into<Scheduler> for SchedulerV004 {
    fn into(self) -> Scheduler {
        Scheduler::with_components(
            self.schedules.into_iter().map(|s| s.into()).collect(),
            self.end_date,
            vec![],
    )
    }
}

#[derive(Serialize, Deserialize)]
pub struct ScheduleV004 {
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
    pub entries: Vec<ScheduleEntry>
}

impl Into<Schedule> for ScheduleV004 {
    fn into(self) -> Schedule {
        Schedule { 
            id: self.id, 
            name: self.name, 
            period: self.period, 
            frequency: self.frequency, 
            start_date: self.start_date, 
            end_date: self.end_date, 
            last_date: self.last_date, 
            entries: self.entries, 
            schedule_modifiers: vec![] }
    }
}