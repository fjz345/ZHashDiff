use std::process::Command;
use std::{env, str};

pub struct P4Revision {
    pub rev: u32,
    pub change: u32,
    pub action: String,
    pub date: String,
}

pub fn get_p4_file_content(path: &str) -> Result<String, String> {
    let p4_exe = env::var("P4_PATH").unwrap_or_else(|_| "p4".to_string());

    let output = Command::new(&p4_exe)
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
    let (program, args) = if cfg!(target_os = "windows") {
        (
            "cmd",
            vec!["/C", "p4", "-ztag", "filelog", "-m", "10", path],
        )
    } else {
        ("p4", vec!["-ztag", "filelog", "-m", "10", path])
    };

    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;

    let stdout = str::from_utf8(&output.stdout).map_err(|e| e.to_string())?;
    let mut history = Vec::new();

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
