//! Offline admin utility: mint a fresh **admin** API token directly into a
//! Nucleus database. Useful to seed/recover access (the bootstrap token is shown
//! only once). The server must be stopped — redb is single-process.
//!
//!   cargo run -p nucleus-core --example mint_token -- <path-to.redb> [name]
//!
//! Prints the plaintext token (store it; only the hash is persisted).

use nucleus_core::auth::{self, Scope};
use nucleus_core::storage::Storage;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let db = args
        .next()
        .expect("usage: mint_token <path-to.redb> [name]");
    let name = args.next().unwrap_or_else(|| "admin".to_string());

    let storage = Storage::open(db)?;
    let (plaintext, hash) = auth::generate_token();
    storage.create_token(&name, hash, vec![Scope::admin_all()], None)?;
    println!("{plaintext}");
    Ok(())
}
