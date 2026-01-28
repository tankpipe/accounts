use std::{collections::HashMap};
use chrono::{NaiveDate};
use serde::{Serialize, Deserialize};
use uuid::Uuid;

use crate::{account::{Account, Transaction}, books::Settings};
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

#[derive(Serialize, Deserialize)]
pub struct SchedulerV004 {
    pub schedules: Vec<Schedule>,
    #[serde(serialize_with = "serialize_option_naivedate")]
    #[serde(deserialize_with = "deserialize_option_naivedate")]
    pub end_date: Option<NaiveDate>,
}

