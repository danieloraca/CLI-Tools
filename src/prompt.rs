use anyhow::{Context, Result, bail};
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

pub fn prompt_index(label: &str, max: usize) -> Result<usize> {
    if max == 0 {
        bail!("nothing to select");
    }

    loop {
        let raw = prompt(label)?;
        match raw.parse::<usize>() {
            Ok(value) if (1..=max).contains(&value) => return Ok(value),
            _ => eprintln!("Enter a number between 1 and {max}."),
        }
    }
}
