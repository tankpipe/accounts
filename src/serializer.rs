use chrono::{NaiveDate, ParseError};
use serde::{Deserializer, Serializer, Deserialize};

pub fn deserialize_naivedate<'de, D>(deserializer: D) -> Result<NaiveDate, D::Error>
    where D: Deserializer<'de>
{
    let date = String::deserialize(deserializer)?;
    use serde::de::Error;
    parse_date_str(&date).map_err(Error::custom)
}

pub fn serialize_naivedate<S>(date: &NaiveDate, serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer,
{
    serializer.serialize_some(&date.to_string())
}

pub fn deserialize_option_naivedate<'de, D>(deserializer: D) -> Result<Option<NaiveDate>, D::Error>
    where D: Deserializer<'de>
{
    let date_str = String::deserialize(deserializer)?;
    use serde::de::Error;
    let date = parse_date_str(&date_str).map_err(Error::custom);
    Ok(Some(date?))
    
}

pub fn serialize_option_naivedate<S>(date: &Option<NaiveDate>, serializer: S) -> Result<S::Ok, S::Error>
    where S: Serializer,
{
    match &date {
        Some(d) => serializer.serialize_some(&d.to_string()),
        None => serializer.serialize_some(&"null".to_string())
    }
    
}


fn parse_date_str(date_str: &String) -> Result<NaiveDate, ParseError> {
    NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
}
