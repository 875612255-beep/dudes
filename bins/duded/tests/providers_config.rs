//! What the operator file's `[providers]` table does, read through the same
//! loader the daemon starts with.

use duded::config::{ProviderTables, load_operator};
use dudes_providers::codex::config::CodexConfig;
use dudes_providers::pi::config::PiConfig;

/// Absolute on the host platform, which is what the loader accepts besides a bare name.
const CODEX: &str = if cfg!(windows) {
    r"C:\codex\bin\codex.exe"
} else {
    "/opt/codex/bin/codex"
};
const PI: &str = if cfg!(windows) {
    r"C:\pi\bin\pi.exe"
} else {
    "/opt/homebrew/bin/pi"
};

fn loaded(source: &str) -> Result<ProviderTables, String> {
    let directory = tempfile::tempdir().expect("a config directory");
    let path = directory.path().join("config.toml");
    std::fs::write(&path, source).expect("write the operator config");
    load_operator(&path)
        .map(|config| config.providers)
        .map_err(|error| error.to_string())
}

fn codex(command: &str) -> ProviderTables {
    ProviderTables {
        codex: Some(CodexConfig {
            streaming: false,
            command: command.to_owned(),
        }),
        ..ProviderTables::default()
    }
}

fn pi(command: &str) -> ProviderTables {
    ProviderTables {
        pi: Some(PiConfig {
            command: command.to_owned(),
        }),
        ..ProviderTables::default()
    }
}

#[test]
fn a_provider_table_configures_its_provider_and_its_absence_configures_nothing() {
    assert_eq!(loaded("[providers.codex]\n"), Ok(codex("codex")));
    assert_eq!(
        loaded(&format!("[providers.codex]\ncommand = '{CODEX}'\n")),
        Ok(codex(CODEX))
    );
    assert_eq!(loaded("[providers.pi]\n"), Ok(pi("pi")));
    assert_eq!(
        loaded(&format!("[providers.pi]\ncommand = '{PI}'\n")),
        Ok(pi(PI))
    );
    assert_eq!(
        loaded("[providers.codex]\n[providers.pi]\n"),
        Ok(ProviderTables {
            codex: Some(CodexConfig::default()),
            pi: Some(PiConfig::default()),
        })
    );
    assert_eq!(loaded(""), Ok(ProviderTables::default()));
    assert_eq!(loaded("[budget]\n"), Ok(ProviderTables::default()));
}

#[test]
fn a_provider_table_no_adapter_reads_is_refused() {
    for name in ["gemini", "claude", "codx", "pie"] {
        let refused = loaded(&format!("[providers.{name}]\ncommand = \"x\"\n"))
            .expect_err("an unread provider table is refused");
        assert!(
            refused.ends_with(&format!(
                "has a [providers.{name}] table, and this daemon reads only [providers.codex] and \
                 [providers.pi]"
            )),
            "[providers.{name}]: {refused}"
        );
    }
    let refused = loaded("[providers.\"two words\"]\n").expect_err("refused");
    assert!(
        refused.ends_with(
            "has a [providers] table other than [providers.codex] and [providers.pi], the only \
             ones this daemon reads"
        ),
        "a table name outside the restricted ASCII is not echoed: {refused}"
    );
}

#[test]
fn a_provider_table_with_an_unknown_key_or_no_command_is_refused() {
    for (name, source) in [
        ("codex", "[providers.codex]\nmodel = \"gpt-5-codex\"\n"),
        ("codex", "[providers.codex]\ncommand = \"\"\n"),
        ("pi", "[providers.pi]\nmodel = \"openai-codex/gpt-5.5\"\n"),
        ("pi", "[providers.pi]\ncommand = \"\"\n"),
    ] {
        let refused = loaded(source).expect_err("the table is refused");
        assert!(
            refused.ends_with(&format!(
                "has an invalid [providers.{name}] table: an unknown key, or a command that is not a bare name or absolute path"
            )),
            "{source:?}: {refused}"
        );
    }
}

#[test]
fn a_provider_command_must_be_absolute_or_a_bare_name() {
    for (name, command) in [
        ("codex", "./codex"),
        ("codex", "bin/codex"),
        ("pi", "./pi"),
        ("pi", "bin/pi"),
    ] {
        let source = format!("[providers.{name}]\ncommand = \"{command}\"\n");
        let refused = loaded(&source).expect_err("a relative provider command is refused at load");
        assert!(
            refused.contains(&format!("[providers.{name}]")),
            "{source:?}: {refused}"
        );
    }
}

#[test]
fn codex_streaming_is_opt_in_and_requires_a_boolean() {
    assert!(
        !loaded("[providers.codex]\n")
            .unwrap()
            .codex
            .unwrap()
            .streaming
    );
    assert!(
        loaded("[providers.codex]\nstreaming = true\n")
            .unwrap()
            .codex
            .unwrap()
            .streaming
    );
    assert!(
        !loaded("[providers.codex]\nstreaming = false\n")
            .unwrap()
            .codex
            .unwrap()
            .streaming
    );
    assert!(loaded("[providers.codex]\nstreaming = 'true'\n").is_err());
}
