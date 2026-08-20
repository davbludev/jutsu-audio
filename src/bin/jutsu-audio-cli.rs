use std::io::Read;

/// What the two flags print. Everything else about this binary is the JSON
/// protocol on stdin — these exist so a user who just put the tool on their
/// PATH can confirm it is there and find out where to read.
const USAGE: &str = "jutsu-audio-cli — one JSON request on stdin, one JSON response on stdout.

    echo '{\"protocol_version\":1,\"operation\":\"describe_protocol\"}' | jutsu-audio-cli

describe_protocol lists every operation this build accepts. Full documentation:
docs/cli.md, shipped beside this binary.";

fn main() {
    match std::env::args().nth(1).as_deref() {
        None => run(),
        Some("--version" | "-V") => {
            println!("jutsu-audio-cli {}", env!("CARGO_PKG_VERSION"));
        }
        Some("--help" | "-h") => println!("{USAGE}"),
        Some(unknown) => {
            // Not a JSON error envelope: this is a shell mistake, not a
            // protocol one, and a person is reading it.
            eprintln!("jutsu-audio-cli: unexpected argument '{unknown}'\n\n{USAGE}");
            std::process::exit(2);
        }
    }
}

fn run() {
    let mut input = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut input) {
        let response = jutsu_audio::cli::error_response("invalid_request", error.to_string());
        println!("{response}");
        std::process::exit(2);
    }
    let (exit_code, response) = jutsu_audio::cli::execute_json(&input);
    println!("{response}");
    std::process::exit(exit_code);
}
