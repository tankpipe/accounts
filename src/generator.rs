#![allow(dead_code)]

use chrono::{NaiveDate};
use crate::account::{Schedule, Transaction};

pub struct Generator {
	pub scheduled_transations: Vec<Schedule>
}

impl Generator {
	pub fn generate(&mut self, end_date: NaiveDate) -> Vec<Transaction> {
		let mut transactions : Vec<Transaction> = Vec::new();

		for schedule in &mut self.scheduled_transations {
			let mut next = schedule.schedule_next(end_date);
			while next.is_some() {
				transactions.push(next.unwrap());
				next = schedule.schedule_next(end_date);
			}
		}
		transactions.sort_by(|a, b| a.date.cmp(&b.date));
		return transactions
	}
}


#[cfg(test)]
mod tests {
    use chrono::{NaiveDate};
    use rust_decimal_macros::dec;
    use uuid::Uuid;
    
    use crate::account::{ScheduleEnum, Schedule};
    use crate::generator::Generator;

	#[test]
	fn test_generate() {
		let st1 = Schedule{
			id: Uuid::new_v4(),
			name: "ST 1".to_string(),
			period: ScheduleEnum::Months,
			frequency: 3,
			start_date:   NaiveDate::from_ymd(2022, 3, 11),
			last_date:   Some(NaiveDate::from_ymd(2022, 3, 11)),
			amount:      dec!(100.99),
			description: "st test 1".to_string(),
			dr_account_id: Some(Uuid::new_v4()),
			cr_account_id: Some(Uuid::new_v4())
		};

		let st2 = Schedule{
			id: Uuid::new_v4(),
			name: "ST 2".to_string(),
			period: ScheduleEnum::Days,
			frequency: 45,
			start_date:   NaiveDate::from_ymd(2022, 3, 11),
			last_date:   Some(NaiveDate::from_ymd(2022, 3, 11)),
			amount:      dec!(20.23),
			description: "st test 2".to_string(),
			dr_account_id: Some(Uuid::new_v4()),
			cr_account_id: Some(Uuid::new_v4())
		};



		let mut generator = Generator{scheduled_transations: vec!{st1, st2}};
	

		let max_date = NaiveDate::from_ymd(2023, 3, 11);		
		let transactions = generator.generate(max_date);
		
		assert_eq!(12, transactions.len());
		assert_eq!("st test 2", transactions[0].description);
		assert_eq!("st test 1", transactions[2].description);
	}
}