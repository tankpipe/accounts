#[cfg(test)]
#[macro_use]
mod tests {
    use accounts::account::*;
    use accounts::books::{sort_transactions_by_account, Books, BooksError, TransactionSortOrder};
    use accounts::reconcile::{Field, ReconciliationItem, ReconciliationMatchStatus};
    use accounts::schedule::{Schedule, ScheduleEntry, ScheduleEnum};
    use chrono::NaiveDate;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use uuid::Uuid;

    // Helper function for creating BooksError for integration tests
    fn make_books_error(message: &str) -> BooksError {
        BooksError {
            error: message.to_string(),
        }
    }

    #[test]
    fn test_add_account() {
        let a = Account::create_new("test account", AccountType::Liability);
        let id1 = a.id;
        let mut b = Books::build_empty("My Books");
        b.add_account(a);

        let a2 = &b.accounts()[0];
        assert_eq!(id1, a2.id);
    }

    #[test]
    fn test_add_account_resets_reconciliation_info() {
        let mut a = Account::create_new("test account", AccountType::Liability);
        a.reconciliation_info = Some(ReconciliationInfo {
            date: NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
            balance: dec!(100),
            transaction_id: Uuid::new_v4(),
        });
        let mut b = Books::build_empty("My Books");
        b.add_account(a);

        let a2 = &b.accounts()[0];
        assert!(a2.reconciliation_info.is_none());
    }

    #[test]
    fn test_delete_account() {
        let (mut books, id1, id2) = setup_books();
        let _result = books.delete_account(&id1);
        assert!(matches!((), _result));
        assert!(books.get_account(&id1).is_err());
        assert!(books.get_account(&id2).is_ok());
    }

    #[test]
    fn test_update_account_allows_non_reconciliation_changes() {
        let (mut books, id1, _id2) = setup_books();
        let mut account = books.get_account(&id1).unwrap().clone();
        account.name = "updated name".to_string();

        let result = books.update_account(account);
        assert!(result.is_ok());

        let updated = books.get_account(&id1).unwrap();
        assert_eq!("updated name", updated.name);
    }

    #[test]
    fn test_update_account_rejects_reconciliation_changes() {
        let (mut books, id1, _id2) = setup_books();
        let mut account = books.get_account(&id1).unwrap().clone();
        account.reconciliation_info = Some(ReconciliationInfo {
            date: NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
            balance: dec!(100),
            transaction_id: Uuid::new_v4(),
        });

        let result = books.update_account(account);
        assert!(result.is_err());
    }

    #[test]
    fn test_update_account_rejects_account_type_change_with_transactions() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        books.add_transaction(t1).unwrap();

        let mut account = books.get_account(&id1).unwrap().clone();
        account.account_type = AccountType::Expense;

        let result = books.update_account(account);
        assert!(result.is_err());
    }

    #[test]
    fn test_update_account_rejects_starting_balance_change_when_reconciled() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        books.add_transaction(t1.clone()).unwrap();
        books
            .reconcile_account_transactions(id1, vec![t1.id])
            .unwrap();

        let mut account = books.get_account(&id1).unwrap().clone();
        account.starting_balance = dec!(500);

        let result = books.update_account(account);
        assert!(result.is_err());
    }

    #[test]
    fn test_cannot_delete_account_with_transactions() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction(None, Some(id1));
        books.add_transaction(t1).unwrap();
        let result = books.delete_account(&id1);
        assert_eq!(
            format!("Account {} can not be deleted as it has transactions.", id1),
            result.err().unwrap().error
        );
        assert!(books.get_account(&id1).is_ok());
        assert!(books.get_account(&id2).is_ok());
    }

    #[test]
    fn test_cannot_delete_with_invalid_account_id() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction(None, Some(id1));
        books.add_transaction(t1).unwrap();
        let id = &Uuid::new_v4();
        let result = books.delete_account(id);
        assert_eq!(
            format!("Account {} not found.", id),
            result.err().unwrap().error
        );
        assert!(books.get_account(&id1).is_ok());
        assert!(books.get_account(&id2).is_ok());
    }

    #[test]
    fn test_add_transaction() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction(Some(id1), Some(id2));
        let t1_id = t1.id;
        books.add_transaction(t1).unwrap();
        let t1_2 = &books.transactions()[0];
        assert_eq!(t1_id, t1_2.id);
    }

    #[test]
    fn test_double_entry_required() {
        let (mut books, id1, id2) = setup_books();
        books.settings.require_double_entry = true;
        assert_eq!(0, books.transactions().len());
        let mut t1 = build_transaction(Some(id1), Some(id2));
        t1.entries.pop();
        let result = books.add_transaction(t1);
        assert_eq!(
            "A transaction needs at least two entries (double entry required is on).".to_string(),
            result.err().unwrap().error
        );
        assert_eq!(0, books.transactions().len());
    }

    #[test]
    fn test_at_least_one_entry_required() {
        let (mut books, id1, id2) = setup_books();
        assert_eq!(0, books.transactions().len());
        let mut t1 = build_transaction(Some(id1), Some(id2));
        t1.entries.pop();
        t1.entries.pop();
        let result = books.add_transaction(t1);
        assert_eq!(
            "A transaction must have at least one entry.".to_string(),
            result.err().unwrap().error
        );
        assert_eq!(0, books.transactions().len());
    }

    #[test]
    fn test_add_transaction_no_cr_account() {
        let (mut books, id1, _) = setup_books();
        let t1 = build_transaction(Some(id1), None);
        let t1_id = t1.id;
        books.add_transaction(t1).unwrap();
        let t1_2 = &books.transactions()[0];
        assert_eq!(t1_id, t1_2.id);
    }

    #[test]
    fn test_add_transaction_no_dr_account() {
        let (mut books, _, id2) = setup_books();
        let t1 = build_transaction(None, Some(id2));
        let t1_id = t1.id;
        books.add_transaction(t1).unwrap();
        let t1_2 = &books.transactions()[0];
        assert_eq!(t1_id, t1_2.id);
    }

    #[test]
    fn test_add_transaction_invalid_dr_account() {
        let (mut books, _, id2) = setup_books();
        let t1 = build_transaction(Some(Uuid::new_v4()), Some(id2));
        let _result = books.add_transaction(t1);
        let expected: Result<(), BooksError> = Err(make_books_error("errors.invalid_cr_account"));
        assert!(matches!(expected, _result));
        assert_eq!(0, (&books.transactions()).len());
    }

    #[test]
    fn test_add_transaction_invalid_cr_account() {
        let (mut books, id1, _) = setup_books();
        let t1 = build_transaction(Some(id1), Some(Uuid::new_v4()));
        let _result = books.add_transaction(t1);
        let expected: Result<(), BooksError> = Err(make_books_error("errors.invalid_cr_account"));
        assert!(matches!(expected, _result));
        assert_eq!(0, (&books.transactions()).len());
    }

    #[test]
    fn test_add_transaction_before_reconciliation_date_rejected() {
        let (mut books, account1_id, account2_id) = setup_books();

        // Add a transaction and reconcile the account
        let reconciliation_date = NaiveDate::from_ymd_opt(2022, 6, 4).unwrap();
        let t1 =
            build_transaction_with_date(Some(account1_id), Some(account2_id), reconciliation_date);
        books.add_transaction(t1.clone()).unwrap();
        books
            .reconcile_account_transactions(account1_id, vec![t1.id])
            .unwrap();

        println!(
            "Reconciled account {:?}",
            books.get_account(&account1_id).unwrap()
        );

        // Try to add a transaction before the reconciliation date - should be rejected
        let early_date = NaiveDate::from_ymd_opt(2022, 6, 1).unwrap();
        let t2 = build_transaction_with_date(Some(account1_id), Some(account2_id), early_date);
        let result = books.add_transaction(t2);
        assert!(result.is_err());

        let error_msg = format!("Transactions can not be added earlier than the account reconciliation date. {} is before {}.", early_date, reconciliation_date);
        assert_eq!(error_msg, result.err().unwrap().error);
    }

    #[test]
    fn test_add_transaction_after_reconciliation_date_allowed() {
        let (mut books, id1, id2) = setup_books();

        // Add a transaction and reconcile the account
        let reconciliation_date = NaiveDate::from_ymd_opt(2022, 6, 4).unwrap();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), reconciliation_date);
        books.add_transaction(t1.clone()).unwrap();
        books
            .reconcile_account_transactions(id1, vec![t1.id])
            .unwrap();

        // Add a transaction after the reconciliation date - should be allowed
        let later_date = NaiveDate::from_ymd_opt(2022, 6, 10).unwrap();
        let t2 = build_transaction_with_date(Some(id1), Some(id2), later_date);
        let result = books.add_transaction(t2);
        assert!(result.is_ok());
        assert_eq!(2, books.transactions().len());
    }

    #[test]
    fn test_add_transaction_no_account() {
        let (mut books, _id1, _id2) = setup_books();
        let t1 = build_transaction(None, None);
        let _result = books.add_transaction(t1);
        let expected: Result<(), BooksError> =
            Err(make_books_error("errors.transaction_requires_one_account"));
        assert!(matches!(expected, _result));
        assert_eq!(0, (&books.transactions()).len());
    }

    #[test]
    fn test_delete_transaction() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction(Some(id1), Some(id2));
        let t1_id = t1.id;
        books.add_transaction(t1).unwrap();

        let _result = books.delete_transaction(&t1_id);
        let expected: Result<(), BooksError> = Ok(());
        assert!(matches!(expected, _result));
        assert_eq!(0, books.transactions().len());
    }

    #[test]
    fn test_delete_invalid_transaction() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction(Some(id1), Some(id2));
        books.add_transaction(t1).unwrap();

        let id = &Uuid::new_v4();
        let result = books.delete_transaction(&id);
        assert_eq!(
            format!("Transaction {} not found.", id),
            result.err().unwrap().error
        );
        assert_eq!(1, books.transactions().len());
    }

    #[test]
    fn test_account_entries() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        let t2 = build_transaction_with_date(
            None,
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 5).unwrap(),
        );
        let t3 = build_transaction_with_date(
            Some(id1),
            None,
            NaiveDate::from_ymd_opt(2022, 7, 1).unwrap(),
        );
        let t4 = build_transaction_with_date(
            Some(id2),
            Some(id1),
            NaiveDate::from_ymd_opt(2022, 7, 2).unwrap(),
        );
        let t1a1e1 = &t1.account_entries(id1)[0];
        let t3a1e3 = &t3.account_entries(id1)[0];
        let t4a1e4 = &t4.account_entries(id1)[0];
        let t1a2e1 = &t1.account_entries(id2)[0];
        let t2a2e1 = &t2.account_entries(id2)[0];
        let t4a2e1 = &t4.account_entries(id2)[0];
        books.add_transaction(t1).unwrap();
        books.add_transaction(t2).unwrap();
        books.add_transaction(t3).unwrap();
        books.add_transaction(t4).unwrap();
        let a1_entries = books.account_entries(id1).unwrap();
        assert_eq!(3, a1_entries.len());

        let entry1 = &a1_entries[0];
        assert_eq!(t1a1e1.id, entry1.id);
        assert_eq!(dec!(10000), entry1.balance.unwrap());

        let entry2 = &a1_entries[1];
        assert_eq!(t3a1e3.id, entry2.id);
        assert_eq!(dec!(20000), entry2.balance.unwrap());

        let entry3 = &a1_entries[2];
        assert_eq!(t4a1e4.id, entry3.id);
        assert_eq!(dec!(10000), entry3.balance.unwrap());

        let a2_entries = books.account_entries(id2).unwrap();
        assert_eq!(3, a2_entries.len());

        let entry21 = &a2_entries[0];
        assert_eq!(t1a2e1.id, entry21.id);
        assert_eq!(dec!(-10000), entry21.balance.unwrap());

        let entry22 = &a2_entries[1];
        assert_eq!(t2a2e1.id, entry22.id);
        assert_eq!(dec!(-20000), entry22.balance.unwrap());

        let entry23 = &a2_entries[2];
        assert_eq!(t4a2e1.id, entry23.id);
        assert_eq!(dec!(-10000), entry23.balance.unwrap());
    }

    #[test]
    fn test_account_transactions_same_day_preserves_order() {
        let (mut books, id1, id2) = setup_books();
        let date = NaiveDate::from_ymd_opt(2022, 6, 4).unwrap();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), date);
        let t2 = build_transaction_with_date(Some(id1), Some(id2), date);
        let t3 = build_transaction_with_date(Some(id1), Some(id2), date);

        let t1_id = t1.id;
        let t2_id = t2.id;
        let t3_id = t3.id;

        books.add_transaction(t1).unwrap();
        books.add_transaction(t2).unwrap();
        books.add_transaction(t3).unwrap();

        let a2_transactions = books.account_transactions(id2).unwrap();
        assert_eq!(3, a2_transactions.len());
        assert_eq!(t1_id, a2_transactions[0].id);
        assert_eq!(t2_id, a2_transactions[1].id);
        assert_eq!(t3_id, a2_transactions[2].id);
    }

    #[test]
    fn test_sort_transactions_global_uses_transaction_date() {
        let (_books, id1, id2) = setup_books();
        let d1 = NaiveDate::from_ymd_opt(2022, 6, 4).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2022, 6, 5).unwrap();

        let t1 = build_transaction_with_date(Some(id1), Some(id2), d2);
        let t2 = build_transaction_with_date(Some(id1), Some(id2), d1);
        let t3 = build_transaction_with_date(Some(id1), Some(id2), d1);

        let t1_id = t1.id;
        let t2_id = t2.id;
        let t3_id = t3.id;

        let mut transactions = vec![t1, t2, t3];
        sort_transactions_by_account(&mut transactions, None, TransactionSortOrder::OldestFirst);

        assert_eq!(t2_id, transactions[0].id);
        assert_eq!(t3_id, transactions[1].id);
        assert_eq!(t1_id, transactions[2].id);
    }

    #[test]
    fn test_sort_transactions_global_uses_transaction_date_newest_first() {
        let (_books, id1, id2) = setup_books();
        let d1 = NaiveDate::from_ymd_opt(2022, 6, 4).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2022, 6, 5).unwrap();

        let t1 = build_transaction_with_date(Some(id1), Some(id2), d2);
        let t2 = build_transaction_with_date(Some(id1), Some(id2), d1);
        let t3 = build_transaction_with_date(Some(id1), Some(id2), d1);

        let t1_id = t1.id;
        let t2_id = t2.id;
        let t3_id = t3.id;

        let mut transactions = vec![t1, t2, t3];
        sort_transactions_by_account(&mut transactions, None, TransactionSortOrder::NewestFirst);

        assert_eq!(t1_id, transactions[0].id);
        assert_eq!(t2_id, transactions[1].id);
        assert_eq!(t3_id, transactions[2].id);
    }

    #[test]
    fn test_account_transaction() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        let t1a1e1 = &t1.account_entries(id1)[0];
        let _t1a2e1 = &t1.account_entries(id2)[0];
        books.add_transaction(t1).unwrap();
        let a1_entries = books.account_transactions(id1).unwrap();
        assert_eq!(1, a1_entries.len());

        let entry1 = &a1_entries[0].account_entries(id1)[0];
        assert_eq!(t1a1e1.id, entry1.id);
        assert_eq!(dec!(10000), entry1.balance.unwrap());
    }

    #[test]
    fn test_reconcile_matched_and_unmatched() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        let t2 = build_transaction_with_date(
            None,
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 5).unwrap(),
        );
        let t3 = build_transaction_with_date(
            Some(id1),
            None,
            NaiveDate::from_ymd_opt(2022, 7, 1).unwrap(),
        );
        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();
        books.add_transaction(t3.clone()).unwrap();

        // Simulate statement import: same date/amount/accounts as books, but different IDs
        let mut statement_t1 = clone_transaction_for_reconcile(&t1);
        for e in &mut statement_t1.entries {
            if e.account_id == id2 {
                e.balance = Some(dec!(-10000));
                break;
            }
        }
        let mut statement_t2 = clone_transaction_for_reconcile(&t2);
        for e in &mut statement_t2.entries {
            if e.account_id == id2 {
                e.balance = Some(dec!(-20000));
                break;
            }
        }
        // Different date, amount, description -> no match (not even partial)
        let mut statement_t3_unmatched = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 7, 15).unwrap(),
        );
        for e in &mut statement_t3_unmatched.entries {
            if e.account_id == id2 {
                e.amount = dec!(5000);
                e.description = "unmatched tx".to_string();
                break;
            }
        }

        let statement_t1_id = statement_t1.id;
        let statement_t2_id = statement_t2.id;
        let statement_t3_unmatched_id = statement_t3_unmatched.id;
        let to_reconcile = vec![statement_t1, statement_t2, statement_t3_unmatched];
        let results = books.prepare_reconciliation(id2, to_reconcile).unwrap();

        assert_eq!(5, results.len());

        // Existing transactions with matched reconciliations spliced after.
        match &results[0] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.transaction.id, t1.id);
                assert_eq!(target.status, ReconciliationMatchStatus::Matched);
                assert_eq!(target.matched_reconciliation_id, Some(statement_t1_id));
            }
            _ => panic!("expected original transaction"),
        }
        match &results[1] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.transaction.id, statement_t1_id);
                assert_eq!(recon.status, ReconciliationMatchStatus::Matched);
                assert_eq!(recon.matched_transaction_id, Some(t1.id));
                assert_eq!(recon.balance, Some(dec!(-10000)));
            }
            _ => panic!("expected reconciliation transaction"),
        }

        match &results[2] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.transaction.id, t2.id);
                assert_eq!(target.status, ReconciliationMatchStatus::Matched);
                assert_eq!(target.matched_reconciliation_id, Some(statement_t2_id));
            }
            _ => panic!("expected original transaction"),
        }
        match &results[3] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.transaction.id, statement_t2_id);
                assert_eq!(recon.status, ReconciliationMatchStatus::Matched);
                assert_eq!(recon.matched_transaction_id, Some(t2.id));
                assert_eq!(recon.balance, Some(dec!(-20000)));
            }
            _ => panic!("expected reconciliation transaction"),
        }

        match &results[4] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.transaction.id, statement_t3_unmatched_id);
                assert_eq!(recon.status, ReconciliationMatchStatus::Unmatched);
                assert_eq!(recon.matched_transaction_id, None);
                assert_eq!(recon.balance, None);
            }
            _ => panic!("expected reconciliation transaction"),
        }
    }

    #[test]
    fn test_reconcile_account_sets_info_and_marks_entries_correctly() {
        let (mut books, account_id1, account_id2) = setup_books();
        let t0 = build_transaction_with_date(
            Some(account_id1),
            Some(account_id2),
            NaiveDate::from_ymd_opt(2022, 6, 3).unwrap(),
        );
        let t1 = build_transaction_with_date(
            Some(account_id1),
            Some(account_id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        let t2 = build_transaction_with_date(
            Some(account_id1),
            Some(account_id2),
            NaiveDate::from_ymd_opt(2022, 6, 5).unwrap(),
        );
        let t3 = build_transaction_with_date(
            Some(account_id1),
            Some(account_id2),
            NaiveDate::from_ymd_opt(2022, 6, 6).unwrap(),
        );

        books.add_transaction(t0.clone()).unwrap();
        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();
        books.add_transaction(t3.clone()).unwrap();

        // reconcile earliest transaction first to check it does not change with later reconciliations
        books
            .reconcile_account_transactions(account_id1, vec![t0.id])
            .unwrap();
        books
            .reconcile_account_transactions(account_id1, vec![t2.id])
            .unwrap();

        let account = books.get_account(&account_id1).unwrap();
        let info = account.reconciliation_info.as_ref().unwrap();
        assert_eq!(t2.id, info.transaction_id);
        assert_eq!(t2.entries[0].date, info.date);

        let t0_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t0.id)
            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();

        let t1_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t1.id)
            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();
        let t2_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t2.id)
            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();
        let t3_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t3.id)
            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();

        assert_eq!(
            Some(ReconciledStatus::Reconciled),
            t0_entry.reconciled_status
        );
        assert_eq!(
            Some(ReconciledStatus::Outstanding),
            t1_entry.reconciled_status
        );
        assert_eq!(
            Some(ReconciledStatus::Reconciled),
            t2_entry.reconciled_status
        );
        assert_eq!(None, t3_entry.reconciled_status);
    }

    #[test]
    fn test_reconcile_account_no_op_when_earlier_or_already_reconciled() {
        let (mut books, account_id1, account_id2) = setup_books();
        let t1 = build_transaction_with_date(
            Some(account_id1),
            Some(account_id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        let t2 = build_transaction_with_date(
            Some(account_id1),
            Some(account_id2),
            NaiveDate::from_ymd_opt(2022, 6, 5).unwrap(),
        );
        let t3 = build_transaction_with_date(
            Some(account_id1),
            Some(account_id2),
            NaiveDate::from_ymd_opt(2022, 6, 6).unwrap(),
        );

        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();
        books.add_transaction(t3.clone()).unwrap();

        books
            .reconcile_account_transactions(account_id1, vec![t2.id])
            .unwrap();
        let binding = books.get_account(&account_id1).unwrap();
        let info = binding.reconciliation_info.as_ref().unwrap();
        assert_eq!(t2.id, info.transaction_id);

        books
            .reconcile_account_transactions(account_id1, vec![t1.id])
            .unwrap();
        let binding = books.get_account(&account_id1).unwrap();
        let info_after_earlier = binding.reconciliation_info.as_ref().unwrap();
        assert_eq!(t2.id, info_after_earlier.transaction_id);

        books
            .reconcile_account_transactions(account_id1, vec![t2.id])
            .unwrap();
        let binding = books.get_account(&account_id1).unwrap();
        let info_after_reconcile_again = binding.reconciliation_info.as_ref().unwrap();
        assert_eq!(t2.id, info_after_reconcile_again.transaction_id);
    }

    #[test]
    fn test_reconcile_account_no_op_when_same_day_earlier_in_order() {
        let (mut books, id1, id2) = setup_books();
        let date = NaiveDate::from_ymd_opt(2022, 6, 4).unwrap();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), date);
        let t2 = build_transaction_with_date(Some(id1), Some(id2), date);
        let t3 = build_transaction_with_date(Some(id1), Some(id2), date);

        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();
        books.add_transaction(t3.clone()).unwrap();

        books
            .reconcile_account_transactions(id1, vec![t2.id])
            .unwrap();
        let binding = books.get_account(&id1).unwrap();
        let info = binding.reconciliation_info.as_ref().unwrap();
        assert_eq!(t2.id, info.transaction_id);

        books
            .reconcile_account_transactions(id1, vec![t1.id])
            .unwrap();
        let binding = books.get_account(&id1).unwrap();
        let info_after_earlier = binding.reconciliation_info.as_ref().unwrap();
        assert_eq!(t2.id, info_after_earlier.transaction_id);
    }

    #[test]
    fn test_reconcile_account_advances_when_same_day_later_in_order() {
        let (mut books, id1, id2) = setup_books();
        let date = NaiveDate::from_ymd_opt(2022, 6, 4).unwrap();
        let t1 = build_transaction_with_date(Some(id1), Some(id2), date);
        let t2 = build_transaction_with_date(Some(id1), Some(id2), date);
        let t3 = build_transaction_with_date(Some(id1), Some(id2), date);

        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();
        books.add_transaction(t3.clone()).unwrap();

        books
            .reconcile_account_transactions(id1, vec![t2.id])
            .unwrap();
        let binding = books.get_account(&id1).unwrap();
        let info = binding.reconciliation_info.as_ref().unwrap();
        assert_eq!(t2.id, info.transaction_id);

        books
            .reconcile_account_transactions(id1, vec![t3.id])
            .unwrap();
        let binding = books.get_account(&id1).unwrap();
        let info_after_later = binding.reconciliation_info.as_ref().unwrap();
        assert_eq!(t3.id, info_after_later.transaction_id);
    }

    #[test]
    fn test_rollback_reconciliation_resets_to_last_reconciled_before_date() {
        let (mut books, account_id1, account_id2) = setup_books();
        let t1 = build_transaction_with_date(
            Some(account_id1),
            Some(account_id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        let t2 = build_transaction_with_date(
            Some(account_id1),
            Some(account_id2),
            NaiveDate::from_ymd_opt(2022, 6, 5).unwrap(),
        );
        let t3 = build_transaction_with_date(
            Some(account_id1),
            Some(account_id2),
            NaiveDate::from_ymd_opt(2022, 6, 6).unwrap(),
        );

        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();
        books.add_transaction(t3.clone()).unwrap();

        books
            .reconcile_account_transactions(account_id1, vec![t1.id, t2.id])
            .unwrap();
        books
            .rollback_reconciliation(account_id1, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap())
            .unwrap();

        let account = books.get_account(&account_id1).unwrap();
        let info = account.reconciliation_info.as_ref().unwrap();
        assert_eq!(t1.id, info.transaction_id);
        assert_eq!(t1.entries[0].date, info.date);

        let t1_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t1.id)
            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();
        let t2_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t2.id)
            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();
        let t3_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t3.id)
            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();

        assert_eq!(
            Some(ReconciledStatus::Reconciled),
            t1_entry.reconciled_status
        );
        assert_eq!(None, t2_entry.reconciled_status);
        assert_eq!(None, t3_entry.reconciled_status);
    }

    #[test]
    fn test_rollback_reconciliation_clears_all_when_no_reconciled_before_date() {
        let (mut books, account_id1, account_id2) = setup_books();
        let t1 = build_transaction_with_date(
            Some(account_id1),
            Some(account_id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        let t2 = build_transaction_with_date(
            Some(account_id1),
            Some(account_id2),
            NaiveDate::from_ymd_opt(2022, 6, 5).unwrap(),
        );

        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();

        books
            .reconcile_account_transactions(account_id1, vec![t1.id, t2.id])
            .unwrap();
        books
            .rollback_reconciliation(account_id1, NaiveDate::from_ymd_opt(2022, 6, 3).unwrap())
            .unwrap();

        let account = books.get_account(&account_id1).unwrap();
        assert!(account.reconciliation_info.is_none());

        let t1_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t1.id)
            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();
        let t2_entry = books
            .transactions()
            .iter()
            .find(|t| t.id == t2.id)
            .and_then(|t| t.find_entry_by_account(&account_id1))
            .unwrap();

        assert_eq!(None, t1_entry.reconciled_status);
        assert_eq!(None, t2_entry.reconciled_status);
    }

    #[test]
    fn test_reconcile_balances() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        books.add_transaction(t1.clone()).unwrap();

        let mut statement_t1 = clone_transaction_for_reconcile(&t1);
        for e in &mut statement_t1.entries {
            if e.account_id == id2 {
                e.balance = Some(dec!(-10000));
                break;
            }
        }
        let results = books
            .prepare_reconciliation(id2, vec![statement_t1])
            .unwrap();

        assert_eq!(2, results.len());
        match &results[1] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.balance, Some(dec!(-10000)));
            }
            _ => panic!("expected reconciliation transaction"),
        }
    }

    #[test]
    fn test_reconcile_partial_match() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        books.add_transaction(t1.clone()).unwrap();

        // Date within ±1 day + same amount + same balance, but different description -> PartialMatch
        let mut partial_t1 = clone_transaction_for_reconcile(&t1);
        for e in &mut partial_t1.entries {
            if e.account_id == id2 {
                e.date = NaiveDate::from_ymd_opt(2022, 6, 5).unwrap();
                e.description = "adjusted description".to_string();
                e.balance = Some(dec!(-10000));
                break;
            }
        }

        let results = books.prepare_reconciliation(id2, vec![partial_t1]).unwrap();
        assert_eq!(2, results.len());
        match &results[0] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.status, ReconciliationMatchStatus::PartialMatch);
            }
            _ => panic!("expected original transaction"),
        }
        match &results[1] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.status, ReconciliationMatchStatus::PartialMatch);
                assert_eq!(recon.matched_transaction_id, Some(t1.id));
            }
            _ => panic!("expected reconciliation transaction"),
        }
    }

    #[test]
    fn test_reconcile_partial_match_date_variance() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        books.add_transaction(t1.clone()).unwrap();

        // One day after book entry, same amount and description -> date within ±1 day, so 2 of 3 = PartialMatch
        let mut next_day = clone_transaction_for_reconcile(&t1);
        for e in &mut next_day.entries {
            if e.account_id == id2 {
                e.date = NaiveDate::from_ymd_opt(2022, 6, 5).unwrap();
                e.balance = Some(dec!(-10000));
                break;
            }
        }

        let results = books.prepare_reconciliation(id2, vec![next_day]).unwrap();
        assert_eq!(2, results.len());
        match &results[0] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.status, ReconciliationMatchStatus::PartialMatch);
            }
            _ => panic!("expected original transaction"),
        }
        match &results[1] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.status, ReconciliationMatchStatus::PartialMatch);
                assert_eq!(recon.matched_transaction_id, Some(t1.id));
            }
            _ => panic!("expected reconciliation transaction"),
        }
    }

    #[test]
    fn test_reconcile_unmatched_when_more_than_14_days_apart() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        books.add_transaction(t1.clone()).unwrap();

        let mut statement_t1 = clone_transaction_for_reconcile(&t1);
        for e in &mut statement_t1.entries {
            if e.account_id == id2 {
                e.date = NaiveDate::from_ymd_opt(2022, 6, 20).unwrap();
                e.balance = Some(dec!(-10000));
                break;
            }
        }

        let results = books
            .prepare_reconciliation(id2, vec![statement_t1])
            .unwrap();
        assert_eq!(2, results.len());
        match &results[0] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.status, ReconciliationMatchStatus::Unmatched);
                assert_eq!(target.matched_reconciliation_id, None);
            }
            _ => panic!("expected original transaction"),
        }
        match &results[1] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.status, ReconciliationMatchStatus::Unmatched);
                assert_eq!(recon.matched_transaction_id, None);
                assert!(recon.confidence > 0.0);
                assert!(recon
                    .signals
                    .iter()
                    .any(|s| s.field == Field::Linkage && s.deviation < 0.0));
                assert!(recon
                    .signals
                    .iter()
                    .any(|s| { s.field == Field::Candidate }));
            }
            _ => panic!("expected reconciliation transaction"),
        }
    }

    #[test]
    fn test_reconcile_mismatch_balance() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        books.add_transaction(t1.clone()).unwrap();

        let mut statement_t1 = clone_transaction_for_reconcile(&t1);
        for e in &mut statement_t1.entries {
            if e.account_id == id2 {
                e.balance = Some(dec!(-9000));
                break;
            }
        }

        let statement_t1_id = statement_t1.id;
        let results = books
            .prepare_reconciliation(id2, vec![statement_t1])
            .unwrap();

        assert_eq!(2, results.len());
        match &results[0] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.status, ReconciliationMatchStatus::Mismatch);
                assert_eq!(target.matched_reconciliation_id, Some(statement_t1_id));
            }
            _ => panic!("expected original transaction"),
        }
        match &results[1] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.status, ReconciliationMatchStatus::Mismatch);
                assert_eq!(recon.matched_transaction_id, Some(t1.id));
            }
            _ => panic!("expected reconciliation transaction"),
        }
    }

    #[test]
    fn test_reconcile_mismatch_promoted_when_balances_realign() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        let t2 = build_transaction_with_date(
            None,
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 5).unwrap(),
        );
        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();

        let mut statement_t1 = clone_transaction_for_reconcile(&t1);
        for e in &mut statement_t1.entries {
            if e.account_id == id2 {
                e.balance = Some(dec!(-9000));
                break;
            }
        }
        let mut statement_t2 = clone_transaction_for_reconcile(&t2);
        for e in &mut statement_t2.entries {
            if e.account_id == id2 {
                e.balance = Some(dec!(-20000));
                break;
            }
        }

        let results = books
            .prepare_reconciliation(id2, vec![statement_t1, statement_t2])
            .unwrap();

        assert_eq!(4, results.len());
        match &results[0] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.status, ReconciliationMatchStatus::PartialMatch);
            }
            _ => panic!("expected original transaction"),
        }
        match &results[1] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.status, ReconciliationMatchStatus::PartialMatch);
            }
            _ => panic!("expected reconciliation transaction"),
        }
        match &results[2] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.status, ReconciliationMatchStatus::Matched);
            }
            _ => panic!("expected original transaction"),
        }
        match &results[3] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.status, ReconciliationMatchStatus::Matched);
            }
            _ => panic!("expected reconciliation transaction"),
        }
    }

    #[test]
    fn test_reconcile_invalid_account() {
        let (books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        let result = books.prepare_reconciliation(Uuid::new_v4(), vec![t1]);
        assert!(result.is_err());
    }

    #[test]
    fn test_reconcile_match_after_unreconciled_entry() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        let t2 = build_transaction_with_date(
            None,
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 5).unwrap(),
        );
        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();

        let mut statement_t2 = clone_transaction_for_reconcile(&t2);
        for e in &mut statement_t2.entries {
            if e.account_id == id2 {
                e.balance = Some(dec!(-20000));
                break;
            }
        }

        let statement_t2_id = statement_t2.id;
        let results = books
            .prepare_reconciliation(id2, vec![statement_t2])
            .unwrap();
        assert_eq!(3, results.len());
        match &results[0] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.transaction.id, t1.id);
                assert_eq!(target.status, ReconciliationMatchStatus::Unmatched);
                assert_eq!(target.matched_reconciliation_id, None);
            }
            _ => panic!("expected original transaction"),
        }
        match &results[1] {
            ReconciliationItem::Original(target) => {
                assert_eq!(target.transaction.id, t2.id);
                assert_eq!(target.status, ReconciliationMatchStatus::Matched);
                assert_eq!(target.matched_reconciliation_id, Some(statement_t2_id));
            }
            _ => panic!("expected original transaction"),
        }
        match &results[2] {
            ReconciliationItem::Reconciliation(recon) => {
                assert_eq!(recon.status, ReconciliationMatchStatus::Matched);
                assert_eq!(recon.matched_transaction_id, Some(t2.id));
                assert!(recon.confidence > 0.0);
                assert!(!recon.signals.is_empty());
            }
            _ => panic!("expected reconciliation transaction"),
        }
    }

    #[test]
    fn test_prepare_reconciliation_missing_statement_row_stays_unmatched() {
        let (mut books, account_id, _other_account_id) = setup_books();

        let t1 = build_single_entry_transaction(
            account_id,
            NaiveDate::from_ymd_opt(2026, 2, 14).unwrap(),
            "T1",
            dec!(100),
            None,
        );
        let t2 = build_single_entry_transaction(
            account_id,
            NaiveDate::from_ymd_opt(2026, 2, 21).unwrap(),
            "T2",
            dec!(100),
            None,
        );
        let t21 = build_single_entry_transaction(
            account_id,
            NaiveDate::from_ymd_opt(2026, 2, 21).unwrap(),
            "T2.1",
            dec!(100),
            None,
        );
        let t3 = build_single_entry_transaction(
            account_id,
            NaiveDate::from_ymd_opt(2026, 2, 28).unwrap(),
            "T3",
            dec!(100),
            None,
        );
        let t4 = build_single_entry_transaction(
            account_id,
            NaiveDate::from_ymd_opt(2026, 3, 7).unwrap(),
            "T4",
            dec!(100.48),
            None,
        );

        books.add_transaction(t1.clone()).unwrap();
        books.add_transaction(t2.clone()).unwrap();
        books.add_transaction(t21.clone()).unwrap();
        books.add_transaction(t3.clone()).unwrap();
        books.add_transaction(t4.clone()).unwrap();

        let reconciliation_rows = vec![
            build_single_entry_transaction(
                account_id,
                NaiveDate::from_ymd_opt(2026, 2, 14).unwrap(),
                "T1",
                dec!(100),
                Some(dec!(100)),
            ),
            build_single_entry_transaction(
                account_id,
                NaiveDate::from_ymd_opt(2026, 2, 15).unwrap(),
                "Missing",
                dec!(50),
                Some(dec!(150)),
            ),
            build_single_entry_transaction(
                account_id,
                NaiveDate::from_ymd_opt(2026, 2, 21).unwrap(),
                "T2.1",
                dec!(100),
                Some(dec!(250)),
            ),
            build_single_entry_transaction(
                account_id,
                NaiveDate::from_ymd_opt(2026, 2, 27).unwrap(),
                "T3",
                dec!(100),
                Some(dec!(350)),
            ),
        ];

        let results = books
            .prepare_reconciliation(account_id, reconciliation_rows)
            .unwrap();

        let missing_reconciliation = results
            .iter()
            .filter_map(|item| {
                if let ReconciliationItem::Reconciliation(recon) = item {
                    let entry = recon.transaction.find_entry_by_account(&account_id)?;
                    if entry.description == "Missing" {
                        return Some(recon);
                    }
                }
                None
            })
            .next()
            .expect("Missing row should exist in reconciliation output");

        assert_eq!(
            missing_reconciliation.status,
            ReconciliationMatchStatus::Unmatched
        );
        assert_eq!(missing_reconciliation.matched_transaction_id, None);

        let t21_reconciliation = results
            .iter()
            .filter_map(|item| {
                if let ReconciliationItem::Reconciliation(recon) = item {
                    let entry = recon.transaction.find_entry_by_account(&account_id)?;
                    if entry.description == "T2.1" {
                        return Some(recon);
                    }
                }
                None
            })
            .next()
            .expect("T2.1 row should exist in reconciliation output");

        assert_eq!(t21_reconciliation.matched_transaction_id, Some(t21.id));
    }

    #[test]
    fn test_prepare_reconciliation_red_lobster_missing_does_not_steal_match() {
        let (mut books, account_id, _other_account_id) = setup_books();

        let water_txn = build_single_entry_transaction(
            account_id,
            NaiveDate::from_ymd_opt(2024, 4, 19).unwrap(),
            "Water",
            dec!(35.60),
            None,
        );
        let amazon_txn = build_single_entry_transaction(
            account_id,
            NaiveDate::from_ymd_opt(2024, 4, 21).unwrap(),
            "Amazon",
            dec!(89.45),
            None,
        );

        books.add_transaction(water_txn.clone()).unwrap();
        books.add_transaction(amazon_txn.clone()).unwrap();

        let reconciliation_rows = vec![
            build_single_entry_transaction(
                account_id,
                NaiveDate::from_ymd_opt(2024, 4, 8).unwrap(),
                "RED LOBSTER #123",
                dec!(38.75),
                Some(dec!(3263.05)),
            ),
            build_single_entry_transaction(
                account_id,
                NaiveDate::from_ymd_opt(2024, 4, 20).unwrap(),
                "DEPT OF WATER UTILITIES",
                dec!(35.60),
                Some(dec!(2866.91)),
            ),
            build_single_entry_transaction(
                account_id,
                NaiveDate::from_ymd_opt(2024, 4, 22).unwrap(),
                "AMAZON.COM SEATTLE",
                dec!(89.45),
                Some(dec!(2777.46)),
            ),
        ];

        let results = books
            .prepare_reconciliation(account_id, reconciliation_rows)
            .unwrap();

        let red_lobster = results
            .iter()
            .filter_map(|item| match item {
                ReconciliationItem::Reconciliation(recon) => recon
                    .transaction
                    .find_entry_by_account(&account_id)
                    .and_then(|e| (e.description == "RED LOBSTER #123").then_some(recon)),
                _ => None,
            })
            .next()
            .expect("RED LOBSTER row should exist");

        assert_eq!(red_lobster.status, ReconciliationMatchStatus::Unmatched);
        assert_eq!(red_lobster.matched_transaction_id, None);

        let amazon_statement = results
            .iter()
            .filter_map(|item| match item {
                ReconciliationItem::Reconciliation(recon) => recon
                    .transaction
                    .find_entry_by_account(&account_id)
                    .and_then(|e| (e.description == "AMAZON.COM SEATTLE").then_some(recon)),
                _ => None,
            })
            .next()
            .expect("AMAZON.COM SEATTLE row should exist");

        assert_eq!(amazon_statement.matched_transaction_id, Some(amazon_txn.id));
    }

    fn clone_transaction_for_reconcile(t: &Transaction) -> Transaction {
        let new_id = Uuid::new_v4();
        Transaction {
            id: new_id,
            entries: t
                .entries
                .iter()
                .map(|e| Entry {
                    id: Uuid::new_v4(),
                    transaction_id: new_id,
                    date: e.date,
                    description: e.description.clone(),
                    account_id: e.account_id,
                    entry_type: e.entry_type,
                    amount: e.amount,
                    balance: None,
                    reconciled_status: None,
                })
                .collect(),
            status: t.status,
            source_type: None,
            source_id: None,
        }
    }

    fn build_single_entry_transaction(
        account_id: Uuid,
        date: NaiveDate,
        description: &str,
        amount: Decimal,
        balance: Option<Decimal>,
    ) -> Transaction {
        let transaction_id = Uuid::new_v4();
        Transaction {
            id: transaction_id,
            entries: vec![Entry {
                id: Uuid::new_v4(),
                transaction_id,
                date,
                description: description.to_string(),
                account_id,
                entry_type: Side::Debit,
                amount,
                balance,
                reconciled_status: None,
            }],
            status: TransactionStatus::Recorded,
            source_type: None,
            source_id: None,
        }
    }

    #[test]
    fn test_account_transactions() {
        let (mut books, id1, id2) = setup_books();
        let t1 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        let t2 = build_transaction_with_date(
            None,
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 5).unwrap(),
        );
        let t3 = build_transaction_with_date(
            Some(id1),
            None,
            NaiveDate::from_ymd_opt(2022, 7, 1).unwrap(),
        );
        let t4 = build_transaction_with_date(
            Some(id2),
            Some(id1),
            NaiveDate::from_ymd_opt(2022, 7, 2).unwrap(),
        );
        let t1a1e1 = &t1.account_entries(id1)[0];
        let t3a1e3 = &t3.account_entries(id1)[0];
        let t4a1e4 = &t4.account_entries(id1)[0];
        let t1a2e1 = &t1.account_entries(id2)[0];
        let t2a2e1 = &t2.account_entries(id2)[0];
        let t4a2e1 = &t4.account_entries(id2)[0];
        books.add_transaction(t1).unwrap();
        books.add_transaction(t2).unwrap();
        books.add_transaction(t3).unwrap();
        books.add_transaction(t4).unwrap();
        let a1_entries = books.account_transactions(id1).unwrap();
        assert_eq!(3, a1_entries.len());

        let entry1 = &a1_entries[0].account_entries(id1)[0];
        assert_eq!(t1a1e1.id, entry1.id);
        assert_eq!(dec!(10000), entry1.balance.unwrap());

        let entry2 = &a1_entries[1].account_entries(id1)[0];
        assert_eq!(t3a1e3.id, entry2.id);
        assert_eq!(dec!(20000), entry2.balance.unwrap());

        let entry3 = &a1_entries[2].account_entries(id1)[0];
        assert_eq!(t4a1e4.id, entry3.id);
        assert_eq!(dec!(10000), entry3.balance.unwrap());

        let a2_entries = books.account_transactions(id2).unwrap();
        assert_eq!(3, a2_entries.len());

        let entry21 = &a2_entries[0].account_entries(id2)[0];
        assert_eq!(t1a2e1.id, entry21.id);
        assert_eq!(dec!(-10000), entry21.balance.unwrap());

        let entry22 = &a2_entries[1].account_entries(id2)[0];
        assert_eq!(t2a2e1.id, entry22.id);
        assert_eq!(dec!(-20000), entry22.balance.unwrap());

        let entry23 = &a2_entries[2].account_entries(id2)[0];
        assert_eq!(t4a2e1.id, entry23.id);
        assert_eq!(dec!(-10000), entry23.balance.unwrap());
    }

    #[test]
    fn test_add_schedule() {
        let (mut books, id1, id2) = setup_books();
        let st1 = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let _result = books.add_schedule(st1);
        let expected: Result<(), BooksError> = Err(make_books_error("errors.invalid_cr_account"));
        assert!(matches!(expected, _result));
        assert_eq!(1, (&books.schedules()).len());
    }

    #[test]
    fn test_update_schedule() {
        let (mut books, id1, id2) = setup_books();
        let st1 = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let mut st1_copy = st1.clone();
        let _result = books.add_schedule(st1);

        st1_copy.entries[0].description = "test changed".to_string();
        let _result = books.update_schedule(st1_copy);
        assert_eq!(1, (books.schedules()).len());
        assert_eq!("test changed", books.schedules()[0].entries[0].description);
    }

    #[test]
    fn test_delete_schedule() {
        let (mut books, id1, id2) = setup_books();
        let st1 = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let st1_id = st1.id;
        books.add_schedule(st1).unwrap();
        assert_eq!(1, books.schedules().len());

        let result = books.delete_schedule(&st1_id);
        assert!(result.is_ok());
        assert_eq!(0, books.schedules().len());
    }

    #[test]
    fn test_cannot_delete_schedule_with_transactions() {
        let (mut books, id1, id2) = setup_books();
        let st1 = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let st1_id = st1.id;
        books.add_schedule(st1).unwrap();

        // Generate transactions from the schedule
        books.generate(NaiveDate::from_ymd_opt(2023, 6, 4).unwrap());

        // Verify transactions were created with schedule_id
        assert!(books.transactions().len() > 0);
        assert!(books
            .transactions()
            .iter()
            .any(|t| { t.source_type == Some(Source::Schedule) && t.source_id == Some(st1_id) }));

        // Try to delete the schedule - should fail
        let result = books.delete_schedule(&st1_id);
        assert_eq!(
            format!(
                "Schedule {} can not be deleted as it has transactions.",
                st1_id
            )
            .to_string(),
            result.err().unwrap().error
        );
        assert_eq!(1, books.schedules().len());
    }

    #[test]
    fn test_cannot_delete_schedule_with_invalid_id() {
        let (mut books, id1, id2) = setup_books();
        let st1 = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        books.add_schedule(st1).unwrap();

        let invalid_id = Uuid::new_v4();
        let result = books.delete_schedule(&invalid_id);
        assert_eq!(
            format!("Schedule {} not found.", invalid_id).to_string(),
            result.err().unwrap().error
        );
        assert_eq!(1, books.schedules().len());
    }

    #[test]
    fn test_add_schedule_invalid_dr_account() {
        let (mut books, id1, id2) = setup_books();
        let st1 = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let _result = books.add_schedule(st1);
        let expected: Result<(), BooksError> = Ok(());
        assert!(matches!(expected, _result));
        assert_eq!(1, (&books.schedules()).len());
    }

    #[test]
    fn test_add_schedule_invalid_cr_account() {
        let (mut books, id1, _) = setup_books();
        let st1 = build_schedule_std(
            id1,
            Uuid::new_v4(),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        let _result = books.add_schedule(st1);
        let expected: Result<(), BooksError> = Err(make_books_error("errors.invalid_cr_account"));
        assert!(matches!(expected, _result));
        assert_eq!(0, (&books.schedules()).len());
    }

    #[test]
    fn test_generate() {
        let (mut books, id1, id2) = setup_books();
        let _result = books.add_schedule(build_schedule(
            id1,
            id2,
            NaiveDate::from_ymd_opt(2022, 3, 11).unwrap(),
            "S_1",
            "st test 1",
            dec!(100.99),
            3,
            ScheduleEnum::Months,
        ));

        let _result = books.add_schedule(build_schedule(
            id2,
            id1,
            NaiveDate::from_ymd_opt(2022, 3, 11).unwrap(),
            "S_2",
            "st test 2",
            dec!(20.23),
            45,
            ScheduleEnum::Days,
        ));

        assert_eq!(0, books.transactions().len());
        books.generate(NaiveDate::from_ymd_opt(2023, 3, 11).unwrap());

        assert_eq!(14, books.transactions().len());
        assert_eq!("st test 2", books.transactions()[2].entries[0].description);
        assert_eq!("st test 1", books.transactions()[4].entries[0].description);
    }

    pub fn setup_books() -> (Books, Uuid, Uuid) {
        let mut books = Books::build_empty("My Books");
        let dr_account1 = Account::create_new("Savings Account 1", AccountType::Asset);
        let dr_account_id: Uuid = dr_account1.id;
        books.add_account(dr_account1);
        let cr_account1 = Account::create_new("Savings Account 2", AccountType::Asset);
        let cr_account_id: Uuid = cr_account1.id;
        books.add_account(cr_account1);
        (books, dr_account_id, cr_account_id)
    }

    fn build_transaction(id1: Option<Uuid>, id2: Option<Uuid>) -> Transaction {
        build_transaction_with_date(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap())
    }

    pub fn build_transaction_with_date(
        dr_account_id: Option<Uuid>,
        cr_account_id: Option<Uuid>,
        date: NaiveDate,
    ) -> Transaction {
        let transaction_id = Uuid::new_v4();
        let description_str = "received moneys";
        let amount = dec!(10000);
        let mut t1 = Transaction {
            id: transaction_id,
            entries: Vec::new(),
            status: TransactionStatus::Recorded,
            source_type: None,
            source_id: None,
        };

        if dr_account_id.is_some() {
            t1.entries.push(Entry {
                id: Uuid::new_v4(),
                transaction_id,
                date,
                description: description_str.to_string(),
                account_id: dr_account_id.unwrap(),
                entry_type: Side::Debit,
                amount,
                balance: None,
                reconciled_status: None,
            })
        }

        if cr_account_id.is_some() {
            t1.entries.push(Entry {
                id: Uuid::new_v4(),
                transaction_id,
                date,
                description: description_str.to_string(),
                account_id: cr_account_id.unwrap(),
                entry_type: Side::Credit,
                amount,
                balance: None,
                reconciled_status: None,
            })
        }
        t1
    }

    fn build_schedule_std(id1: Uuid, id2: Uuid, start_date: NaiveDate) -> Schedule {
        build_schedule(
            id1,
            id2,
            start_date,
            "Reocurring transaction",
            "Reocurring transaction",
            dec!(100),
            1,
            ScheduleEnum::Months,
        )
    }

    fn build_schedule(
        id1: Uuid,
        id2: Uuid,
        start_date: NaiveDate,
        name: &str,
        description: &str,
        amount: Decimal,
        frequency: i64,
        period: ScheduleEnum,
    ) -> Schedule {
        let s_id_1 = Uuid::new_v4();
        Schedule {
            id: s_id_1,
            name: name.to_string(),
            start_date,
            end_date: None,
            last_date: None,
            frequency,
            period,
            entries: vec![
                ScheduleEntry {
                    amount,
                    description: description.to_string(),
                    account_id: id1,
                    entry_type: Side::Debit,
                    schedule_id: s_id_1,
                },
                ScheduleEntry {
                    amount,
                    description: description.to_string(),
                    account_id: id2,
                    entry_type: Side::Credit,
                    schedule_id: s_id_1,
                },
            ],
            schedule_modifiers: vec![],
        }
    }

    #[test]
    fn test_reset_schedule_last_date_with_transactions() {
        let (mut books, id1, id2) = setup_books();
        let schedule = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let schedule_id = schedule.id;

        // Add the schedule
        books.add_schedule(schedule).unwrap();

        // Create some transactions for this schedule
        let t1 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        let mut t1_with_schedule = t1;
        t1_with_schedule.set_source_schedule(schedule_id);

        let t2 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 7, 4).unwrap(),
        );
        let mut t2_with_schedule = t2;
        t2_with_schedule.set_source_schedule(schedule_id);

        let t3 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 8, 4).unwrap(),
        );
        let mut t3_with_schedule = t3;
        t3_with_schedule.set_source_schedule(schedule_id);

        // Add transactions out of order to test that sorting finds the latest date
        books.add_transaction(t3_with_schedule).unwrap(); // August 4
        books.add_transaction(t1_with_schedule).unwrap(); // June 4
        books.add_transaction(t2_with_schedule).unwrap(); // July 4

        // Reset the schedule last date
        let result = books.reset_schedule_last_date(schedule_id);

        // Should return the date of the latest transaction (August 4, 2022)
        // Now that transactions are sorted by date, it should find August 4th regardless of addition order
        assert_eq!(
            result.unwrap(),
            Some(NaiveDate::from_ymd_opt(2022, 8, 4).unwrap())
        );

        // Verify the schedule was updated
        let updated_schedule = books.get_schedule(schedule_id).unwrap();
        assert_eq!(
            updated_schedule.last_date,
            Some(NaiveDate::from_ymd_opt(2022, 8, 4).unwrap())
        );
    }

    #[test]
    fn test_reset_schedule_last_date_no_transactions() {
        let (mut books, id1, id2) = setup_books();
        let schedule = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let schedule_id = schedule.id;

        // Add the schedule but no transactions
        books.add_schedule(schedule).unwrap();

        // Reset the schedule last date
        let result = books.reset_schedule_last_date(schedule_id);

        // Should return None since there are no transactions
        assert_eq!(result.unwrap(), None);

        // Verify the schedule was updated with None
        let updated_schedule = books.get_schedule(schedule_id).unwrap();
        assert_eq!(updated_schedule.last_date, None);
    }

    #[test]
    fn test_reset_schedule_last_date_transactions_for_other_schedules() {
        let (mut books, id1, id2) = setup_books();
        let schedule1 = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let schedule1_id = schedule1.id;

        let schedule2 = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let schedule2_id = schedule2.id;

        // Add both schedules
        books.add_schedule(schedule1).unwrap();
        books.add_schedule(schedule2).unwrap();

        // Create transactions for schedule1
        let t1 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        let mut t1_with_schedule = t1;
        t1_with_schedule.set_source_schedule(schedule1_id);

        // Create transactions for schedule2 (later date)
        let t2 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 8, 4).unwrap(),
        );
        let mut t2_with_schedule = t2;
        t2_with_schedule.set_source_schedule(schedule2_id);

        // Add transactions
        books.add_transaction(t1_with_schedule).unwrap();
        books.add_transaction(t2_with_schedule).unwrap();

        // Reset schedule1's last date
        let result = books.reset_schedule_last_date(schedule1_id);

        // Should return the date of schedule1's last transaction (June 4, 2022)
        assert_eq!(
            result.unwrap(),
            Some(NaiveDate::from_ymd_opt(2022, 6, 4).unwrap())
        );

        // Verify schedule1 was updated correctly
        let updated_schedule1 = books.get_schedule(schedule1_id).unwrap();
        assert_eq!(
            updated_schedule1.last_date,
            Some(NaiveDate::from_ymd_opt(2022, 6, 4).unwrap())
        );

        // Verify schedule2 was not affected
        let updated_schedule2 = books.get_schedule(schedule2_id).unwrap();
        assert_eq!(updated_schedule2.last_date, None);
    }

    #[test]
    fn test_reset_schedule_last_date_nonexistent_schedule() {
        let (mut books, _id1, _id2) = setup_books();
        let fake_schedule_id = Uuid::new_v4();

        // Try to reset last date for a schedule that doesn't exist - should return error
        let result = books.reset_schedule_last_date(fake_schedule_id);
        assert!(result.is_err());
        assert_eq!(
            result.err().unwrap().error,
            format!("Schedule {} not found.", fake_schedule_id).to_string()
        );
    }

    #[test]
    fn test_reset_schedule_last_date_with_existing_last_date() {
        let (mut books, id1, id2) = setup_books();
        let schedule = build_schedule_std(id1, id2, NaiveDate::from_ymd_opt(2022, 6, 4).unwrap());
        let schedule_id = schedule.id;

        // Add the schedule with an existing last_date
        let mut schedule_with_last_date = schedule;
        schedule_with_last_date.last_date = Some(NaiveDate::from_ymd_opt(2022, 5, 4).unwrap());
        books.add_schedule(schedule_with_last_date).unwrap();

        // Create a transaction after the existing last_date
        let t1 = build_transaction_with_date(
            Some(id1),
            Some(id2),
            NaiveDate::from_ymd_opt(2022, 6, 4).unwrap(),
        );
        let mut t1_with_schedule = t1;
        t1_with_schedule.set_source_schedule(schedule_id);

        books.add_transaction(t1_with_schedule).unwrap();

        // Reset the schedule last date
        let result = books.reset_schedule_last_date(schedule_id);

        // Should return the date of the last transaction (June 4, 2022), overwriting the old date
        assert_eq!(
            result.unwrap(),
            Some(NaiveDate::from_ymd_opt(2022, 6, 4).unwrap())
        );

        // Verify the schedule was updated with the new date
        let updated_schedule = books.get_schedule(schedule_id).unwrap();
        assert_eq!(
            updated_schedule.last_date,
            Some(NaiveDate::from_ymd_opt(2022, 6, 4).unwrap())
        );
    }
}
