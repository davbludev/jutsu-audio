use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<_> = env::args().skip(1).collect();
    let result = match arguments.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["quality"] => xtask::run_quality(),
        ["package"] => xtask::package::run_package(std::path::Path::new(".")).map(drop),
        ["smoke", directory] => xtask::smoke::run_smoke(std::path::Path::new(directory)).map(drop),
        _ => {
            eprintln!("usage: cargo quality | cargo package-release | cargo smoke <directory>");
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
