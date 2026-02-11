fn main() {
    let date = time_now();
    println!("cargo:rustc-env=BUILD_DATE={date}");

    // Load .env file for dev builds (credentials for Box API).
    // Environment variables (e.g. from CI secrets) take precedence.
    load_dotenv();
}

fn load_dotenv() {
    let env_path = std::path::Path::new(".env");
    println!("cargo:rerun-if-changed=.env");
    let content = match std::fs::read_to_string(env_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            // Don't override vars already set in the environment
            if std::env::var(key).is_err() {
                println!("cargo:rustc-env={key}={value}");
            }
        }
    }
}

fn time_now() -> String {
    // Use SOURCE_DATE_EPOCH for reproducible builds, otherwise current time
    if let Ok(epoch) = std::env::var("SOURCE_DATE_EPOCH") {
        return epoch_to_date(&epoch);
    }

    let output = std::process::Command::new("date")
        .args(["+%Y-%m-%d"])
        .output()
        .expect("failed to run date");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn epoch_to_date(epoch: &str) -> String {
    let output = std::process::Command::new("date")
        .args(["-d", &format!("@{epoch}"), "+%Y-%m-%d"])
        .output()
        .expect("failed to run date");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}
