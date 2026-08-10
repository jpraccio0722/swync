//! Writes the language's callable surface to stdout as JSON.
//!
//! The same `lang::metadata()` the editor's `language_metadata` command
//! serves, so the website's reference is generated from the tables the lowerer
//! dispatches on rather than transcribed from them. A UGen added in Rust
//! reaches the docs by re-running this — not by anyone remembering to.
//!
//! ```sh
//! cargo run --bin dump-metadata > ../../website/src/data/metadata.json
//! ```

fn main() {
    let metadata = swync_lib::lang::metadata();
    match serde_json::to_string_pretty(&metadata) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("could not serialize the language metadata: {e}");
            std::process::exit(1);
        }
    }
}
