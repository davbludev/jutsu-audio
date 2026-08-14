use std::io::Read;

fn main() {
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
