#![allow(dead_code)]
use std::{path::Path, fs::File, io::Read};
use std::io;
use crate::books::Books;

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
    Ok(Books::build_empty())
}

fn save_books<P: AsRef<Path>>(path: P, books: Books) -> io::Result<()> {
    ::serde_json::to_writer(&File::create(path)?, &books)?;
    Ok(())
}


#[cfg(test)]

mod tests {
    use std::fs::File;
    use std::io::prelude::*;
    use uuid::Uuid;
    use chrono::{NaiveDate};
    use rust_decimal_macros::dec;
    use crate::account::{Account, Transaction, AccountType, TransactionStatus, ScheduledTransaction, ScheduleEnum};
    use super::{Books, load_books};

   fn build_books() -> Books {
        let mut books = Books::build_empty();
        let dr_account1 = Account::create_new("Savings Account 1", AccountType::Debit);
        let id1: Uuid = dr_account1.id;
        books.add_account(dr_account1);
        let cr_account1 = Account::create_new("Credit Account 1", AccountType::Debit);
        let id2: Uuid = cr_account1.id;
        books.add_account(cr_account1);

        let t1 = Transaction{ 
            id: Uuid::new_v4(), 
            date: NaiveDate::from_ymd(2022, 6, 4), 
            description: "received moneys".to_string(), 
            dr_account_id: Some(id1), 
            cr_account_id: Some(id2), 
            amount: dec!(10000), 
            status: TransactionStatus::Recorded,
            balance: None };
        books.add_transaction(t1).unwrap();
        let t2 = Transaction{ 
            id: Uuid::new_v4(), 
            date: NaiveDate::from_ymd(2022, 6, 5), 
            description: "Gave some moneys back".to_string(), 
            dr_account_id: Some(id2), 
            cr_account_id: Some(id1), 
            amount: dec!(98.99), 
            status: TransactionStatus::Recorded,
            balance: None };
        books.add_transaction(t2).unwrap();
        let st = ScheduledTransaction{ 
            id: Uuid::new_v4(), 
            name: "Some income".to_string(), 
            period: ScheduleEnum::Months, 
            frequency: 1, 
            start_date: NaiveDate::from_ymd(2022, 6, 4),
            last_date: NaiveDate::from_ymd(2022, 6, 4), 
            amount: dec!(200), 
            description: "Money in".to_string(), 
            dr_account_id: Some(id1), 
            cr_account_id: Some(id2) 
        };
        let _ = books.add_schedule(st);
        books
   } 
   #[test]
   fn test_load_books() {
        let books = build_books();
        let filepath = "books.json";        

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