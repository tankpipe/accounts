#![allow(dead_code)]
use std::{path::Path, fs::File, io::Read};
use std::{fs, io};
use std::collections::HashMap;
use std::io::Write;
use serde_json::Value;
use crate::books_error;
use rust_decimal::Decimal;

use crate::books::{Books, BooksError};
use crate::account::{Transaction, Side};
use crate::books_prev_versions::{BooksV004, BooksV005};
use uuid::Uuid;

const VERSION: &str = env!("CARGO_PKG_VERSION");

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
                        Some("0.0.6") => {
                            return Err(io::Error::new(io::ErrorKind::InvalidData, why));                            
                        },
                        Some("0.0.5") => {
                            println!(">>>>>>>>>>>>>>> Attempting to upgrade file {} from {} to {}", v["name"], v["version"], VERSION);
                            return load_previous_version_0_0_5(content)
                        },
                        _ => {
                            println!(">>>>>>>>>>>>>>> Attempting to upgrade file {} from {} to {}", v["name"], v["version"], VERSION);
                            return load_previous_version_0_0_4(content)
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
/// If `account_id` is `None`, sorts by transaction date.
pub fn sort_transactions_for_account(
    transactions: &mut Vec<Transaction>,
    account_id: Option<Uuid>,
    order: TransactionSortOrder,
) {
    let mut indexed: Vec<(usize, Transaction)> = transactions.drain(..).enumerate().collect();
    indexed.sort_by(|(a_idx, a_txn), (b_idx, b_txn)| {
        let a_date = match account_id {
            Some(id) => a_txn.find_entry_by_account(&id).map(|e| e.date),
            None => a_txn.date(),
        };
        let b_date = match account_id {
            Some(id) => b_txn.find_entry_by_account(&id).map(|e| e.date),
            None => b_txn.date(),
        };

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

fn load_previous_version_0_0_5(mut content: String) -> Result<Books, io::Error> {
    match serde_json::from_str::<BooksV005>(&mut content) {
        Ok(books) => Ok(books.into()),
        Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
    }
}

fn load_previous_version_0_0_4(mut content: String) -> Result<Books, io::Error> {
    match serde_json::from_str::<BooksV004>(&mut content) {
        Ok(books) => Ok(books.into()),
        Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
    }
}

pub fn save_books<P: AsRef<Path>>(path: P, books: &Books) -> io::Result<()> {
    println!(">>>>>>>>>>>>>>>> Saving Books <<<<<<<<<<<<<<<<");
    ::serde_json::to_writer(&File::create(path)?, &books)?;
    println!(">>>>>>>>>>>>>>>> Saved Books  <<<<<<<<<<<<<<<<");
    Ok(())
}

pub fn export_to_csv<P: AsRef<Path>>(
    path: P,
    books: &Books,
    account_id: Option<Uuid>,
) -> io::Result<()> {
    let mut transactions: Vec<Transaction> = match account_id {
        Some(id) => books
            .account_transactions(id)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.error))?,
        None => books.transactions().iter().cloned().collect(),
    };
    sort_transactions_for_account(
        &mut transactions,
        account_id,
        TransactionSortOrder::OldestFirst,
    );

    let accounts_by_id: HashMap<Uuid, _> = books
        .accounts()
        .into_iter()
        .map(|account| (account.id, account))
        .collect();

    let mut csv = String::new();
    csv.push_str(
        "date,description,account,debit,credit,transaction_id,entry_id,status,reconciled_status,balance\n"
    );

    for transaction in transactions {
        for entry in transaction.entries.iter() {
            if let Some(id) = account_id {
                if entry.account_id != id {
                    continue;
                }
            }
            let account = accounts_by_id.get(&entry.account_id);
            let account_name = account
                .map(|a| a.name.as_str())
                .unwrap_or("Unknown Account");
            let (debit, credit) = match entry.entry_type {
                Side::Debit => (format_decimal_2(entry.amount), String::new()),
                Side::Credit => (String::new(), format_decimal_2(entry.amount)),
            };

            let status = format!("{:?}", transaction.status);
            let reconciled_status = entry
                .reconciled_status
                .map(|s| format!("{:?}", s))
                .unwrap_or_default();
            let balance = entry
                .balance
                .map(format_decimal_2)
                .unwrap_or_default();

            let fields = [
                entry.date.format("%Y-%m-%d").to_string(),
                entry.description.clone(),
                account_name.to_string(),
                debit,
                credit,
                transaction.id.to_string(),
                entry.id.to_string(),
                status,
                reconciled_status,
                balance,
            ];

            for (idx, field) in fields.iter().enumerate() {
                if idx > 0 {
                    csv.push(',');
                }
                csv.push_str(&escape_csv_field(field));
            }
            csv.push('\n');
        }
    }

    let mut file = File::create(path)?;
    file.write_all(csv.as_bytes())?;
    Ok(())
}

pub fn export_accounts_to_csv<P: AsRef<Path>>(path: P, books: &Books) -> io::Result<()> {
    let mut csv = String::new();
    csv.push_str(
        "account_id,name,account_type,starting_balance\n"
    );

    for account in books.accounts() {

        let fields = [
            account.id.to_string(),
            account.name,
            format!("{:?}", account.account_type),
            format_decimal_2(account.starting_balance),
        ];

        for (idx, field) in fields.iter().enumerate() {
            if idx > 0 {
                csv.push(',');
            }
            csv.push_str(&escape_csv_field(field));
        }
        csv.push('\n');
    }

    let mut file = File::create(path)?;
    file.write_all(csv.as_bytes())?;
    Ok(())
}

fn escape_csv_field(value: &str) -> String {
    let needs_quotes = value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r');
    if !needs_quotes {
        return value.to_string();
    }

    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        if ch == '"' {
            escaped.push('"');
        }
        escaped.push(ch);
    }
    escaped.push('"');
    escaped
}

fn format_decimal_2(value: Decimal) -> String {
    format!("{:.2}", value.round_dp(2))
}

pub fn file_exists<P: AsRef<Path>>(path: P) -> bool {
    Path::new(path.as_ref()).exists()
}

pub fn delete_file<P: AsRef<Path>>(path: P) -> Result<(), BooksError> {
    match fs::remove_file(&path) {
        Ok(_) => Ok(()),
        Err(e) => Err(books_error!("errors.file_delete_error", path => format!("{:?}", path.as_ref()), error => format!("{:?}", e)))
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
            io::ErrorKind::AlreadyExists => Err(books_error!("errors.file_already_exists")),
            _ => Err(books_error!("errors.file_create_error", error => format!("{:?}", e)))
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
    use crate::interest::{Interest, InterestTerms, InterestType};
    use crate::{account::{Account, AccountType, Entry, Side, Transaction, TransactionStatus}, books_repo::{export_to_csv, save_books}, schedule::{Modifier, Schedule, ScheduleEntry, ScheduleEnum}};
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
        let interest_terms = InterestTerms::simple(
            date,
            Decimal::new(5, 2),            
            InterestType::Daily,
            ScheduleEnum::Months,
            1,
            1,
            "Monthly interest".to_string(),
            None,
        );
        let _ = books.add_interest(Interest::from_components(vec![interest_terms], id1));   
        books
   }

    fn build_transaction(dr_account_id: Uuid, cr_account_id: Uuid, description_str: &str, date: NaiveDate, amount: Decimal) -> Transaction {
        let transaction_id = Uuid::new_v4();
        let description = description_str;
        let t1 = Transaction{
                id: transaction_id,
                entries: vec![
                    Entry{id:Uuid::new_v4(),transaction_id,date,description:description.to_string(),account_id:dr_account_id,entry_type:Side::Debit,
                        amount,balance:None, reconciled_status: None },
                    Entry{id:Uuid::new_v4(),transaction_id,date,description:description.to_string(),account_id:cr_account_id,entry_type:Side::Credit,
                        amount,balance:None, reconciled_status: None},
                ],
                status: TransactionStatus::Recorded,
                source_type: None,
                source_id: None,
            };
        t1
    }
   #[test]
   fn test_load_books() {
        let books = build_books();
        let tmp_file = NamedTempFile::new().expect("create temp file");
        let filepath = tmp_file.path();
        println!("File path: {:?}", filepath);

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
   fn test_load_books_v0_0_5() {
        let filepath = "src/previous_versions/books_v0.0.5.json";

        let result = load_books(filepath);
        let books = result.unwrap();
        assert_eq!(2, books.accounts().len());
        assert_eq!(1, books.schedules().len());
        assert_eq!(2, books.transactions().len());
        assert_eq!(1, books.modifiers().len());        
        assert_eq!(0, books.interests().len());        
    }

    #[test]
    fn test_load_books_missing_version() {
        let filepath = "src/previous_versions/books_no_version.json";
        let result = load_books(filepath);
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("missing field"));
    }

    #[test]
    fn test_export_to_csv_general_ledger() {
        let books = build_books();
        let tmp_file = NamedTempFile::new().expect("create temp file");
        let filepath = tmp_file.path();

        export_to_csv(filepath, &books, None).expect("export csv");

        let csv = std::fs::read_to_string(filepath).expect("read csv");
        let mut lines = csv.lines();
        let header = lines.next().expect("header");
        assert_eq!(
            "date,description,account,debit,credit,transaction_id,entry_id,status,reconciled_status,balance",
            header
        );

        let rows: Vec<&str> = lines.filter(|line| !line.trim().is_empty()).collect();
        assert_eq!(4, rows.len());
        assert_eq!(2, rows.iter().filter(|row| row.contains("received moneys")).count());
        assert_eq!(2, rows.iter().filter(|row| row.contains("Gave some moneys back")).count());
        assert!(rows.iter().any(|row| row.contains(",Savings Account 1,")));
        assert!(rows.iter().any(|row| row.contains(",Credit Account 1,")));
    }

    #[test]
    fn test_export_to_csv_account_includes_balances() {
        let books = build_books();
        let account_id = books
            .accounts()
            .into_iter()
            .find(|account| account.name == "Savings Account 1")
            .expect("account")
            .id;
        let tmp_file = NamedTempFile::new().expect("create temp file");
        let filepath = tmp_file.path();

        export_to_csv(filepath, &books, Some(account_id)).expect("export csv");

        let csv = std::fs::read_to_string(filepath).expect("read csv");
        let mut lines = csv.lines();
        let _header = lines.next().expect("header");

        let rows: Vec<&str> = lines.filter(|line| !line.trim().is_empty()).collect();
        assert_eq!(2, rows.len());
        assert!(rows.iter().all(|row| row.contains(",Savings Account 1")));
        assert!(rows.iter().all(|row| row.trim_end().ends_with(",,")) == false);
        assert!(rows.iter().any(|row| row.ends_with(",10000.00")));
        assert!(rows.iter().any(|row| row.ends_with(",9901.01")));
    }
}
