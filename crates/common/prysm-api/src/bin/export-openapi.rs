use std::{error::Error, io::Write};

fn main() -> Result<(), Box<dyn Error>> {
    let schema = prysm_api::router().split_for_parts().1.to_pretty_json()?;
    std::io::stdout().lock().write_all(schema.as_bytes())?;
    Ok(())
}
