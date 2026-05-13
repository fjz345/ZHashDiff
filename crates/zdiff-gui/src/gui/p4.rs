use std::process::Command;
use std::str;

pub struct P4Revision {
    pub rev: u32,
    pub change: u32,
    pub action: String,
    pub date: String,
}

pub fn get_p4_file_content(path: &str) -> Result<String, String> {
    let output = Command::new("p4")
        .args(["print", "-q", path])
        .output()
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        Ok(str::from_utf8(&output.stdout)
            .map_err(|e| e.to_string())?
            .to_string())
    } else {
        Err(str::from_utf8(&output.stderr)
            .unwrap_or("Unknown error")
            .to_string())
    }
}

pub fn get_revision_history(path: &str) -> Result<Vec<P4Revision>, String> {
    // -ztag provides formatted output that is easier to parse than raw text
    let output = Command::new("p4")
        .args(["-ztag", "filelog", "-m", "10", path])
        .output()
        .map_err(|e| e.to_string())?;

    let stdout = str::from_utf8(&output.stdout).map_err(|e| e.to_string())?;
    let mut history = Vec::new();

    // Basic ztag parser logic
    let mut current_rev = 0;
    let mut current_change = 0;
    let mut current_action = String::new();

    for line in stdout.lines() {
        if line.starts_with("... rev ") {
            current_rev = line["... rev ".len()..].parse().unwrap_or(0);
        } else if line.starts_with("... change ") {
            current_change = line["... change ".len()..].parse().unwrap_or(0);
        } else if line.starts_with("... action ") {
            current_action = line["... action ".len()..].to_string();
        } else if line.starts_with("... time ") {
            history.push(P4Revision {
                rev: current_rev,
                change: current_change,
                action: current_action.clone(),
                date: line["... time ".len()..].to_string(),
            });
        }
    }

    Ok(history)
}
