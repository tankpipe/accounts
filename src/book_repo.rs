#![allow(dead_code)]
use std::{path::Path, fs::File, io::Read};
use std::{fs, io};
use serde_json::Value;

use crate::books::{Books, BooksError};
use crate::account::Transaction;
use crate::books_prev_versions::BooksV004;
use uuid::Uuid;

/// Simple JSON file storage for Books.

pub fn load_books<P: AsRef<Path>>(path: P) -> Result<Books, io::Error> {
    match File::open(path) {
        Err(why) => {
            println!("Open file failed : {:?}", why.kind());
            Err(why)
        },
        Ok(mut file) => {
            let mut content: String = String::new();
            file.read_to_string(&mut content)?;
            match serde_json::from_str::<Books>(&mut content) {
                Err(why) => {
                    println!("Parsing file json failed : {:?}", why);
                    let v: Value = serde_json::from_str(&mut content)?;
                    println!(">>>>>>>>>>>>>>> File details: {} {} {}", v["id"], v["name"], v["version"]);
                    
                    match v["version"].as_str() {
                        Some("0.0.5") => {
                            return Err(io::Error::new(io::ErrorKind::InvalidData, why));
                        },
                        _ => {
                            println!(">>>>>>>>>>>>>>> Attempting to upgrade file {} from {} to {}", v["name"], v["version"], "current");
                            return load_previous_version(content)
                        },

                    }
                },
                Ok(books) => {
                    return Ok(books)
                }
            }
        }
    }

}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransactionSortOrder {
    OldestFirst,
    NewestFirst,
}

/// Sort imported transactions by account entry date while preserving original order for same-day items.
pub fn sort_transactions_for_account(
    transactions: &mut Vec<Transaction>,
    account_id: Uuid,
    order: TransactionSortOrder,
) {
    let mut indexed: Vec<(usize, Transaction)> = transactions.drain(..).enumerate().collect();
    indexed.sort_by(|(a_idx, a_txn), (b_idx, b_txn)| {
        let a_date = a_txn.find_entry_by_account(&account_id).map(|e| e.date);
        let b_date = b_txn.find_entry_by_account(&account_id).map(|e| e.date);

        let date_cmp = match (a_date, b_date) {
            (Some(a), Some(b)) => a.cmp(&b),
            _ => std::cmp::Ordering::Equal,
        };

        let date_cmp = match order {
            TransactionSortOrder::OldestFirst => date_cmp,
            TransactionSortOrder::NewestFirst => date_cmp.reverse(),
        };

        if date_cmp == std::cmp::Ordering::Equal {
            a_idx.cmp(b_idx)
        } else {
            date_cmp
        }
    });

    *transactions = indexed.into_iter().map(|(_, txn)| txn).collect();
}

fn load_previous_version(mut content: String) -> Result<Books, io::Error> {
    match serde_json::from_str::<BooksV004>(&mut content) {
        Ok(books) => Ok(books.into()),
        Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
    }
}


pub fn save_books<P: AsRef<Path>>(path: P, books: &Books) -> io::Result<()> {
    let _ =::serde_json::to_writer(&File::create(path)?, &books)?;
    Ok(())
}

pub fn file_exists<P: AsRef<Path>>(path: P) -> bool {
    Path::new(path.as_ref()).exists()
}

pub fn delete_file<P: AsRef<Path>>(path: P) -> Result<(), BooksError> {
    match fs::remove_file(&path) {
        Ok(_) => Ok(()),
        Err(e) => Err(BooksError{ error: format!("Error while deleting file: {:?} error: {:?}", path.as_ref(), e) })
    }
}

pub fn save_new_books<P: AsRef<Path>>(path: P, books: &Books) ->  Result<(), BooksError>{
    let file_result = &File::options()
            .write(true)
            .create_new(true)
            .open(&path);

    match file_result {
        Ok(file) => {
            _ = ::serde_json::to_writer(file, &books);
            Ok(())
        },
        Err(e) => {
            println!("Error creating file. Path: {:?} Error: {:?}", path.as_ref(), e);
            match e.kind() {            
            io::ErrorKind::AlreadyExists => Err(BooksError::from_str("A file using this name already exists")),
            _ => Err(BooksError{ error: format!("Error while creating file: {:?}", e) })
            }
        }
    }

}

#[cfg(test)]

mod tests {
    use std::{fs::File};
    use std::io::prelude::*;
    use rust_decimal::Decimal;
    use uuid::Uuid;
    use chrono::{NaiveDate};
    use rust_decimal_macros::dec;
    use crate::{account::{Account, AccountType, Entry, Side, Transaction, TransactionStatus}, book_repo::{save_books}, schedule::{Modifier, Schedule, ScheduleEntry, ScheduleEnum}};
    use tempfile::NamedTempFile;
    use super::{Books, load_books};

   fn build_books() -> Books {
        let mut books = Books::build_empty("My Books");
        let dr_account1 = Account::create_new("Savings Account 1", AccountType::Asset);
        let id1: Uuid = dr_account1.id;
        books.add_account(dr_account1);
        let cr_account1 = Account::create_new("Credit Account 1", AccountType::Liability);
        let id2: Uuid = cr_account1.id;
        books.add_account(cr_account1);
        let date = NaiveDate::from_ymd_opt(2022, 6, 4).unwrap();
        let t1 = build_transaction(id1, id2, "received moneys", date, dec!(10000));
        books.add_transaction(t1).unwrap();

        let t2_date = NaiveDate::from_ymd_opt(2022, 6, 5).unwrap();
        let t2 = build_transaction(id2, id1, "Gave some moneys back", t2_date, dec!(98.99));
        books.add_transaction(t2).unwrap();
        let s_id_1 = Uuid::new_v4();
        let st = Schedule{
            id: s_id_1,
            name: "Some income".to_string(),
            period: ScheduleEnum::Months,
            frequency: 1,
            start_date: NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
            end_date: None,
            last_date: Some(NaiveDate::from_ymd_opt(2022, 6, 4).unwrap()),
            entries: vec![
                ScheduleEntry {
                    amount: dec!(200),
                    description: "Money in".to_string(),
                    account_id: id1,
                    entry_type: Side::Debit,
                    schedule_id: s_id_1,
                },
                ScheduleEntry {
                    amount: dec!(200),
                    description: "Money in".to_string(),
                    account_id: id2,
                    entry_type: Side::Credit,
                    schedule_id: s_id_1,
                }
            ],
            schedule_modifiers: vec![]
        };
        let _ = books.add_schedule(st);
        let m = Modifier {
            id: Uuid::new_v4(),
            name: "Inflation modifier".to_string(),
            period: ScheduleEnum::Years,
            frequency: 1,
            start_date: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            end_date: None,
            amount: Decimal::ZERO,
            percentage: Decimal::new(3, 2),
        };
        let _ = books.add_modifier(m);
        books
   }

    fn build_transaction(dr_account_id: Uuid, cr_account_id: Uuid, description_str: &str, date: NaiveDate, amount: Decimal) -> Transaction {
        let transaction_id = Uuid::new_v4();
        let description = description_str;
        let t1 = Transaction{
                id: transaction_id,
                entries: vec![
                    Entry{id:Uuid::new_v4(),transaction_id,date,description:description.to_string(),account_id:dr_account_id,entry_type:Side::Debit,
                        amount,balance:None, reconciled:false },
                    Entry{id:Uuid::new_v4(),transaction_id,date,description:description.to_string(),account_id:cr_account_id,entry_type:Side::Credit,
                        amount,balance:None, reconciled:false},
                ],
                status: TransactionStatus::Recorded,
                schedule_id: None
            };
        t1
    }
   #[test]
   fn test_load_books() {
        let books = build_books();
        let tmp_file = NamedTempFile::new().expect("create temp file");
        let filepath = tmp_file.path();

        let _ = save_books(filepath, &books);

        match File::open(filepath) {
            Err(why) => {
                println!("Open file failed 2: {:?}", why.kind());
            },
            Ok(mut file) => {
                let mut content: String = String::new();
                file.read_to_string(&mut content).unwrap();
                match serde_json::from_str::<Books>(&mut content) {
                    Err(why) => println!("Open file failed : {:?}", why),
                    Ok(books2) => {
                        assert_eq!(books.accounts().len(), books2.accounts().len());
                    }
                }
            }
        }

        let result = load_books(filepath);
        assert_eq!(books.accounts().len(), result.unwrap().accounts().len());
    }

    #[test]
   fn test_load_books_v0_0_4() {
        let filepath = "src/previous_versions/books_v0.0.4.json";

        let result = load_books(filepath);
        let books = result.unwrap();
        assert_eq!(2, books.accounts().len());
        assert_eq!(1, books.schedules().len());
        assert_eq!(2, books.transactions().len());
        assert_eq!(0, books.modifiers().len());
        
    }

    #[test]
    fn test_load_books_missing_version() {
        let filepath = "src/previous_versions/books_no_version.json";
        let result = load_books(filepath);
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("missing field"));
    }
}
