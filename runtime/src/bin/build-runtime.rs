use pr_consolidate::{distribution, process::mask};

fn main() {
    let result = std::env::current_dir()
        .map_err(pr_consolidate::error::Error::from)
        .and_then(|root| distribution::build(&root));
    let code = match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{}: {}", error.label(), mask(&error.to_string()));
            error.code()
        }
    };
    std::process::exit(code);
}
