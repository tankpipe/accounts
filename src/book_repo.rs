#![allow(dead_code)]
use std::{path::Path, fs::File, io::Read};
use std::{io};

use crate::books::{Books, BooksError};

/// Simple JSON file storage for Books.

pub fn load_books<P: AsRef<Path>>(path: P) -> Result<Books, io::Error> {
    match File::open(path) {
        Err(why) => println!("Open file failed : {:?}", why.kind()),
        Ok(mut file) => {
            let mut content: String = String::new();
            file.read_to_string(&mut content)?;
            match serde_json::from_str::<Books>(&mut content) {
                Err(why) => println!("Parsing file json failed : {:?}", why),
                Ok(books) => {
                    return Ok(books)
                }
            }
        }
    }

    // There was no file, or the file failed to load, create new Books.
    Ok(Books::build_empty("My Books"))
}

pub fn save_books<P: AsRef<Path>>(path: P, books: &Books) -> io::Result<()> {
    let _ =::serde_json::to_writer(&File::create(path)?, &books)?;
    Ok(())
}

pub fn new_books<P: AsRef<Path>>(path: P, books: &Books) ->  Result<(), BooksError>{
    let file_result = &File::options()
            .write(true)
            .create_new(true)
            .open(path);

    match file_result {
        Ok(file) => {
            _ = ::serde_json::to_writer(file, &books);
            Ok(())
        },
        Err(e) => match e.kind() {
            io::ErrorKind::AlreadyExists => Err(BooksError::from_str("A file using this name already exists")),
            _ => Err(BooksError{ error: format!("Error while creating file: {:?}", e) })
        }
    }

}

#[cfg(test)]

mod tests {
    use std::fs::File;
    use std::io::prelude::*;
    use rust_decimal::Decimal;
    use uuid::Uuid;
    use chrono::{NaiveDate};
    use rust_decimal_macros::dec;
    use crate::{account::{Account, Transaction, Side, TransactionStatus, Entry, AccountType}, schedule::{Schedule, ScheduleEnum, ScheduleEntry}, book_repo::save_books};
    use super::{Books, load_books};

   fn build_books() -> Books {
        let mut books = Books::build_empty("My Books");
        let dr_account1 = Account::create_new("Savings Account 1", AccountType::Asset);
        let id1: Uuid = dr_account1.id;
        books.add_account(dr_account1);
        let cr_account1 = Account::create_new("Credit Account 1", AccountType::Liability);
        let id2: Uuid = cr_account1.id;
        books.add_account(cr_account1);
        let date = NaiveDate::from_ymd(2022, 6, 4);
        let t1 = build_transaction(id1, id2, "received moneys", date, dec!(10000));
        books.add_transaction(t1).unwrap();

        let t2_date = NaiveDate::from_ymd(2022, 6, 5);
        let t2 = build_transaction(id2, id1, "Gave some moneys back", t2_date, dec!(98.99));
        books.add_transaction(t2).unwrap();
        let s_id_1 = Uuid::new_v4();
        let st = Schedule{
            id: s_id_1,
            name: "Some income".to_string(),
            period: ScheduleEnum::Months,
            frequency: 1,
            start_date: NaiveDate::from_ymd(2022, 6, 4),
            end_date: None,
            last_date: Some(NaiveDate::from_ymd(2022, 6, 4)),
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
            modifier: None
        };
        let _ = books.add_schedule(st);
        books
   }

    fn build_transaction(dr_account_id: Uuid, cr_account_id: Uuid, description_str: &str, date: NaiveDate, amount: Decimal) -> Transaction {
        let transaction_id = Uuid::new_v4();
        let description = description_str;
        let t1 = Transaction{
                id: transaction_id,
                entries: vec![
                    Entry{id:Uuid::new_v4(),transaction_id,date,description:description.to_string(),account_id:dr_account_id,entry_type:Side::Debit,
                        amount,balance:None },
                    Entry{id:Uuid::new_v4(),transaction_id,date,description:description.to_string(),account_id:cr_account_id,entry_type:Side::Credit,
                        amount,balance:None},
                ],
                status: TransactionStatus::Recorded,
                schedule_id: None
            };
        t1
    }
   #[test]
   fn test_load_books() {
        let books = build_books();
        let filepath = "books.json";

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
}