use chrono::NaiveDate;
use serde::{Deserializer, Serializer, Deserialize};

pub fn deserialize_naivedate<'de, D>(deserializer: D) -> Result<NaiveDate, D::Error>
    where D: Deserializer<'de>
{
    let date = String::deserialize(deserializer)?;
    Ok(parse_date_str(&date))
}

pub fn serialize_naivedate<S>(date: &NaiveDate, serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer,
{
    serializer.serialize_some(&date.to_string())
}

fn parse_date_str(date_str: &String) -> NaiveDate {
    NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").unwrap()
}
