use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::schedule::Schedule;
use crate::serializer::*;
use crate::{
    account::{Account, Entry, Source, Transaction, TransactionStatus},
    books::{Books, Settings},
    schedule::{ScheduleEntry, ScheduleEnum},
    scheduler::Scheduler,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Older version of Books struct for upgrading old files
#[derive(Serialize, Deserialize)]
pub struct BooksV005 {
    pub id: Uuid,
    pub name: String,
    pub version: String,
    pub accounts: HashMap<Uuid, Account>,
    pub scheduler: Scheduler,
    pub transactions: Vec<TransactionV005>,
    pub settings: Settings,
}

impl Into<Books> for BooksV005 {
    fn into(self) -> Books {
        Books::with_components(
            self.id,
            self.name,
            VERSION.to_string(),
            self.accounts,
            self.scheduler,
            self.transactions.into_iter().map(|t| t.into()).collect(),
            HashMap::new(),
            self.settings,
        )
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TransactionV005 {
    pub id: Uuid,
    pub entries: Vec<Entry>,
    pub status: TransactionStatus,
    pub schedule_id: Option<Uuid>,
}

impl Into<Transaction> for TransactionV005 {
    fn into(self) -> Transaction {
        Transaction {
            id: self.id,
            entries: self.entries,
            status: self.status,
            source_type: if self.schedule_id.is_some() {
                Some(Source::Schedule)
            } else {
                None
            },
            source_id: self.schedule_id,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct BooksV004 {
    pub id: Uuid,
    pub name: String,
    pub version: String,
    pub accounts: HashMap<Uuid, Account>,
    pub scheduler: SchedulerV004,
    pub transactions: Vec<TransactionV005>,
    pub settings: Settings,
}

impl Into<Books> for BooksV004 {
    fn into(self) -> Books {
        Books::with_components(
            self.id,
            self.name,
            VERSION.to_string(),
            self.accounts,
            self.scheduler.into(),
            self.transactions.into_iter().map(|t| t.into()).collect(),
            HashMap::new(),
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
    pub entries: Vec<ScheduleEntry>,
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
            schedule_modifiers: vec![],
        }
    }
}
