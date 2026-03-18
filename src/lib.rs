rust_i18n::i18n!("locales");

pub mod account;
pub mod books;
pub mod books_repo;
pub mod serializer;
pub mod scheduler;
pub mod schedule;
pub mod reconcile;
pub mod books_prev_versions;
pub mod interest;

#[macro_export]
macro_rules! books_error {
    ($($tt:tt)*) => {
        $crate::books::BooksError {
            error: rust_i18n::t!($($tt)*).to_string(),
        }
    };
}
