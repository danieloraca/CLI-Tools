use anyhow::{Context, Result};
use std::io::{self, Write};

pub fn prompt(label: &str) -> Result<String> {
    print!("{label}");
    io::stdout().flush().context("failed to flush prompt")?;

    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .context("failed to read input")?;

    Ok(value.trim().to_string())
}

pub fn prompt_secret(label: &str) -> Result<String> {
    rpassword::prompt_password(label).context("failed to read password")
}
