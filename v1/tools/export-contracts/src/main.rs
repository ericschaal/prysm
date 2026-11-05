//! Build-only entry point. The host owns document selection; libraries own type exports.
mod fixtures;
use std::{
    error::Error,
    io::{self, Write},
};

fn main() -> Result<(), Box<dyn Error>> {
    let document = match std::env::args().nth(1).as_deref() {
        None => serde_json::to_value(prysm_assemblies::contracts::documents()?)?,
        Some("--fixtures") => fixtures::serialized()?,
        Some(_) => return Err("usage: prysm-export-contracts [--fixtures]".into()),
    };
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, &document)?;
    writeln!(stdout)?;
    Ok(())
}
