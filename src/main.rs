use std::env;

use accounts::book_repo::load_books;

fn main() {
    let args: Vec<String> = env::args().collect();

    let path = &args[1];
    let books = load_books(path).unwrap();
    print!("{}", ::serde_json::to_string_pretty(&books).unwrap());
}