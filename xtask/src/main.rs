use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<_> = env::args().skip(1).collect();
    if arguments.as_slice() != ["quality"] {
        eprintln!("usage: cargo quality");
        return ExitCode::from(2);
    }

    match xtask::run_quality() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("quality gate failed: {error}");
            ExitCode::FAILURE
        }
    }
}
