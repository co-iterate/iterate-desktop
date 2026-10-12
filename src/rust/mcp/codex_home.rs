use std::env;
use std::process::Command;

pub fn codex_home_from_process_or_parent_env() -> Option<String> {
    codex_home_from_current_env().or_else(codex_home_from_parent_env)
}

fn codex_home_from_current_env() -> Option<String> {
    env::var("CODEX_HOME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn codex_home_from_parent_env() -> Option<String> {
    let pid = std::process::id().to_string();
    let parent_output = Command::new("ps")
        .args(["-o", "ppid=", "-p", pid.as_str()])
        .output()
        .ok()?;
    if !parent_output.status.success() {
        return None;
    }

    let ppid = String::from_utf8_lossy(&parent_output.stdout)
        .trim()
        .to_string();
    if ppid.is_empty() {
        return None;
    }

    let env_output = Command::new("ps")
        .args(["eww", "-p", ppid.as_str()])
        .output()
        .ok()?;
    if !env_output.status.success() {
        return None;
    }

    parse_codex_home_from_process_env(String::from_utf8_lossy(&env_output.stdout).as_ref())
}

fn parse_codex_home_from_process_env(output: &str) -> Option<String> {
    output.split_whitespace().find_map(|part| {
        part.strip_prefix("CODEX_HOME=")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

#[cfg(test)]
mod tests {
    use super::parse_codex_home_from_process_env;

    #[test]
    fn parses_codex_home_from_ps_environment() {
        let output = "123 ?? S /Applications/Codex\\ Clone.app CODEX_HOME=/Users/example/.codex-clone PATH=/bin";

        assert_eq!(
            parse_codex_home_from_process_env(output).as_deref(),
            Some("/Users/example/.codex-clone")
        );
    }

    #[test]
    fn ignores_empty_codex_home_from_ps_environment() {
        assert_eq!(
            parse_codex_home_from_process_env("CODEX_HOME= PATH=/bin"),
            None
        );
    }
}
