## Accounts

Basic accounting entities written in Rust for use in a double entry accounts based system. Does not actually enforce double entry accounting standards, specifiying the second account in a transaction is optional.

Supports generating transactions to help model future cash flows.

Example usage loads a sample JSON file and prints it out:
```bash
cargo run books.json
```
