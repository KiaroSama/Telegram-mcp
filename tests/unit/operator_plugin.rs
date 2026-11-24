use super::{install, package, payload};
use anyhow::Result;
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tempfile::TempDir;
use zip::ZipArchive;

fn fixture() -> (TempDir, PathBuf) {
    // macOS's standard temporary path can contain the system /var alias.
    // The payload contract rejects links, so create the fixture in its physical parent.
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let source = root.path().join("source with spaces");
    fs::create_dir_all(&source).unwrap();
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    for name in payload::files()
        .into_iter()
        .chain(["LICENSE".into(), "NOTICE".into()])
    {
        let target = source.join(&name);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(repository.join(name), target).unwrap();
    }
    (root, source)
}

fn archive(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut archive = ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    let mut result = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        result.insert(file.name().unwrap().into_owned(), bytes);
    }
    result
}

#[test]
fn package_flavors_preserve_skills_and_omit_runtime_credentials() {
    let (root, source) = fixture();
    let obsolete = ["mcp.json", ".mcp.json", "mcp_config.json"];
    for name in obsolete {
        fs::write(source.join(name), b"obsolete config").unwrap();
    }
    let portable = root.path().join("portable.zip");
    let antigravity = root.path().join("antigravity.zip");
    package::package(&source, &portable, "portable").unwrap();
    package::package(&source, &antigravity, "antigravity").unwrap();
    let normal = archive(&portable);
    let special = archive(&antigravity);
    let manifests = [
        "plugin.json",
        ".claude-plugin/plugin.json",
        ".codex-plugin/plugin.json",
        ".cursor-plugin/plugin.json",
        ".antigravity-plugin/plugin.json",
    ];
    let read_json = |name: &str| -> serde_json::Value {
        serde_json::from_slice(&normal[&format!("telegram-operator/{name}")]).unwrap()
    };
    for (index, name) in manifests.iter().enumerate() {
        let manifest = read_json(name);
        assert_eq!(manifest["name"], "telegram-operator", "{name}");
        assert_eq!(manifest["version"], "1.5.0", "{name}");
        for field in ["mcpServers", "logo", "icon", "composerIcon"] {
            assert!(manifest.get(field).is_none(), "{name}: {field}");
            assert!(
                manifest["interface"].get(field).is_none(),
                "{name}: {field}"
            );
        }
        if index == 0 {
            assert_eq!(
                manifest["$schema"],
                "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json"
            );
            assert!(manifest.get("skills").is_none());
        }
    }
    for name in [
        ".claude-plugin/marketplace.json",
        ".agents/plugins/marketplace.json",
        ".cursor-plugin/marketplace.json",
    ] {
        let marketplace = read_json(name);
        assert_eq!(
            marketplace["plugins"][0]["name"], "telegram-operator",
            "{name}"
        );
        assert_eq!(marketplace["plugins"][0]["version"], "1.5.0", "{name}");
        for object in [&marketplace, &marketplace["plugins"][0]] {
            for field in ["mcpServers", "logo", "icon", "composerIcon"] {
                assert!(object.get(field).is_none(), "{name}: {field}");
            }
        }
    }
    assert_eq!(
        read_json(".claude-plugin/marketplace.json")["metadata"]["version"],
        "1.5.0"
    );
    for name in obsolete {
        let path = format!("telegram-operator/{name}");
        assert!(!normal.contains_key(&path), "{path}");
        assert!(!special.contains_key(&path), "{path}");
    }
    let mut expected = read_json(".antigravity-plugin/plugin.json");
    expected.as_object_mut().unwrap().remove("category");
    let ag: serde_json::Value =
        serde_json::from_slice(&special["telegram-operator/plugin.json"]).unwrap();
    assert_eq!(ag, expected);
    assert_eq!(normal.len(), 21);
    assert_eq!(special.len(), 14);
    assert!(!special.contains_key("telegram-operator/.agents/plugins/marketplace.json"));
    let skills: Vec<_> = normal
        .keys()
        .filter(|name| name.contains("/skills/"))
        .collect();
    assert_eq!(skills.len(), 10);
    for name in skills {
        assert_eq!(normal[name], special[name]);
    }
    assert!(normal.contains_key("telegram-operator/LICENSE"));
    assert!(normal.contains_key("telegram-operator/.codex-plugin/plugin.json"));
    assert!(!special.keys().any(|name| name.contains("-plugin/")));
    assert!(
        !normal
            .keys()
            .any(|name| name.ends_with(".env") || name.contains("telegram_mcp/"))
    );
    assert!(package::package(&source, &portable, "portable").is_err());
    assert_eq!(archive(&portable), normal);
}

#[test]
fn package_refuses_extra_payload_file_before_creating_archive() {
    let (root, source) = fixture();
    fs::write(source.join("skills/.env"), "SYNTHETIC=not-a-secret\n").unwrap();
    let output = root.path().join("refused.zip");
    assert!(package::package(&source, &output, "portable").is_err());
    assert!(!output.exists());
}

#[test]
fn package_refuses_a_malformed_native_manifest_before_creating_an_archive() {
    let (root, source) = fixture();
    fs::write(source.join(".cursor-plugin/plugin.json"), b"{").unwrap();
    let output = root.path().join("refused.zip");
    let error = package::package(&source, &output, "portable").unwrap_err();
    assert!(error.to_string().contains(".cursor-plugin/plugin.json"));
    assert!(!output.exists());
}

#[test]
fn package_refuses_an_mcp_declaration_in_a_marketplace_entry() {
    let (root, source) = fixture();
    let manifest = source.join(".claude-plugin/marketplace.json");
    let mut data: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    data["plugins"][0]["mcpServers"] = serde_json::json!("./.mcp.json");
    fs::write(manifest, serde_json::to_vec(&data).unwrap()).unwrap();
    let output = root.path().join("refused.zip");
    let error = package::package(&source, &output, "portable").unwrap_err();
    assert!(error.to_string().contains("mcpServers"));
    assert!(!output.exists());
}

#[test]
fn package_refuses_invalid_utf8_before_creating_archive() {
    let (root, source) = fixture();
    fs::write(
        source.join("skills/telegram-access-audit/SKILL.md"),
        [0xff, 0xfe],
    )
    .unwrap();
    let output = root.path().join("refused.zip");
    assert!(package::package(&source, &output, "portable").is_err());
    assert!(!output.exists());
}

#[test]
fn installer_copies_all_five_complete_skills_and_refuses_collision() {
    let (root, source) = fixture();
    let destination = root.path().join("project with spaces/.hermes/skills");
    install::install(&source, &destination).unwrap();
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 5);
    for (name, reference) in payload::SKILLS {
        for path in ["SKILL.md".to_owned(), format!("references/{reference}")] {
            assert_eq!(
                fs::read(destination.join(name).join(&path)).unwrap(),
                fs::read(source.join("skills").join(name).join(path)).unwrap()
            );
        }
    }
    assert!(install::install(&source, &destination).is_err());
    assert_eq!(fs::read_dir(destination).unwrap().count(), 5);
}

fn write_legacy_configs(destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    let stdio = serde_json::json!({"command": "telegram-mcp", "args": []});
    let config = serde_json::json!({"mcpServers": {"telegram-mcp": stdio}});
    for name in [".mcp.json", "mcp_config.json"] {
        fs::write(destination.join(name), config.to_string()).unwrap();
    }
    let mut portable = config;
    portable["$schema"] =
        serde_json::json!("https://agent-plugins.org/schemas/1.0.0/mcp.schema.json");
    portable["mcpServers"]["telegram-mcp"]["type"] = serde_json::json!("stdio");
    fs::write(destination.join("mcp.json"), portable.to_string()).unwrap();
}

#[test]
fn installer_removes_legacy_configs_on_install_and_upgrade() {
    let (root, source) = fixture();
    let destination = root.path().join("skills");
    let keep = destination.join("owner.json");
    fs::create_dir(&destination).unwrap();
    fs::write(&keep, b"owner data").unwrap();
    for _ in 0..2 {
        write_legacy_configs(&destination);
        install::install(&source, &destination).unwrap();
        for name in ["mcp.json", ".mcp.json", "mcp_config.json"] {
            assert!(!destination.join(name).exists(), "{name}");
        }
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 6);
        assert_eq!(fs::read(&keep).unwrap(), b"owner data");
    }
}

#[test]
fn installer_does_not_clean_up_after_a_skill_collision() {
    let (root, source) = fixture();
    let destination = root.path().join("skills");
    write_legacy_configs(&destination);
    let skill = destination.join("telegram-access-audit/SKILL.md");
    fs::create_dir(skill.parent().unwrap()).unwrap();
    fs::write(&skill, b"custom skill").unwrap();
    let error = install::install(&source, &destination).unwrap_err();
    assert!(error.to_string().contains("not overwritten"));
    assert_eq!(fs::read(skill).unwrap(), b"custom skill");
    assert!(destination.join(".mcp.json").is_file());
}

#[test]
fn installer_preserves_custom_mcp_configuration() {
    let (root, source) = fixture();
    let destination = root.path().join("skills");
    let config = destination.join(".mcp.json");
    let custom = br#"{"mcpServers":{"telegram-mcp":{"command":"configured-server","args":[]}}}"#;
    fs::create_dir(&destination).unwrap();
    fs::write(&config, custom).unwrap();
    install::install(&source, &destination).unwrap();
    assert_eq!(fs::read(config).unwrap(), custom);
}

#[test]
fn installer_refuses_a_directory_named_like_a_legacy_config() {
    let (root, source) = fixture();
    let destination = root.path().join("skills");
    let directory = destination.join(".mcp.json");
    fs::create_dir_all(&directory).unwrap();
    let keep = directory.join("owner.json");
    fs::write(&keep, b"owner data").unwrap();
    assert!(install::install(&source, &destination).is_err());
    assert_eq!(fs::read(keep).unwrap(), b"owner data");
    assert_eq!(fs::read_dir(destination).unwrap().count(), 1);
}

#[test]
fn installer_failure_rolls_back_only_new_owned_files() {
    let (root, source) = fixture();
    let destination = root.path().join("skills");
    fs::create_dir(&destination).unwrap();
    let keep = destination.join("owner.txt");
    fs::write(&keep, "preserve").unwrap();
    write_legacy_configs(&destination);
    let legacy = destination.join(".mcp.json");
    let previous = fs::read(&legacy).unwrap();
    let mut calls = 0;
    let result = install::install_with(&source, &destination, |file, bytes| -> Result<()> {
        calls += 1;
        if calls == 2 {
            file.write_all(&bytes[..1])?;
            anyhow::bail!("Synthetic write failure");
        }
        file.write_all(bytes)?;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(fs::read_dir(destination).unwrap().count(), 4);
    assert_eq!(fs::read(legacy).unwrap(), previous);
    assert_eq!(fs::read_to_string(keep).unwrap(), "preserve");
}

#[test]
fn installer_dot_segments_cannot_write_into_the_canonical_source() {
    let (_root, source) = fixture();
    let destination = source.join("docs/../skills/extra");
    assert!(install::install(&source, &destination).is_err());
    assert!(!source.join("skills/extra").exists());
}

#[test]
fn outputs_cannot_poison_canonical_payload_trees() {
    let (_root, source) = fixture();
    for directory in [".codex-plugin", ".agents/plugins", ".antigravity-plugin"] {
        let destination = source.join(directory).join("copied-skills");
        assert!(install::install(&source, &destination).is_err());
        assert!(!destination.exists());
    }
    assert!(
        package::package(
            &source,
            &source.join("skills/telegram-bot-reconstruction/references/operator.zip"),
            "portable"
        )
        .is_err()
    );
    assert!(!source.join(".codex-plugin/copied-skills").exists());
    assert!(
        !source
            .join("skills/telegram-bot-reconstruction/references/operator.zip")
            .exists()
    );
}

#[test]
fn all_eight_client_routes_are_explicit_and_unknown_routes_refuse() {
    let expected = [
        ("claude", ".claude/skills"),
        ("codex", ".agents/skills"),
        ("cursor", ".cursor/skills"),
        ("kiro", ".kiro/skills"),
        ("cline", ".cline/skills"),
        ("hermes", ".hermes/skills"),
        ("opencode", ".opencode/skills"),
        ("antigravity", ".agents/skills"),
    ];
    for (client, directory) in expected {
        assert_eq!(payload::client_directory(client).unwrap(), directory);
    }
    assert!(payload::client_directory("unknown").is_err());
}

#[test]
fn linked_destination_or_payload_is_refused() {
    let (root, source) = fixture();
    let target = root.path().join("target");
    fs::create_dir(&target).unwrap();
    let linked = root.path().join("link");
    #[cfg(unix)]
    let created = std::os::unix::fs::symlink(&target, &linked);
    #[cfg(windows)]
    let created = std::os::windows::fs::symlink_dir(&target, &linked);
    #[cfg(not(any(unix, windows)))]
    let created: std::io::Result<()> = Err(std::io::Error::other("No symlink support"));
    if created.is_err() {
        return;
    }
    assert!(install::install(&source, &linked.join("skills")).is_err());
    assert!(fs::read_dir(target).unwrap().next().is_none());
}

#[test]
fn a_linked_docs_parent_cannot_copy_an_external_document_into_a_package() {
    let (root, source) = fixture();
    let external = root.path().join("external-documents");
    fs::create_dir(&external).unwrap();
    let canary = "Synthetic document outside the allowlisted source";
    fs::write(external.join("operator-plugin.md"), canary).unwrap();
    fs::remove_dir_all(source.join("docs")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, source.join("docs")).unwrap();
    #[cfg(windows)]
    assert!(
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(source.join("docs"))
            .arg(&external)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    let output = root.path().join("refused.zip");
    assert!(package::package(&source, &output, "portable").is_err());
    assert!(!output.exists());
    assert_eq!(
        fs::read_to_string(external.join("operator-plugin.md")).unwrap(),
        canary
    );
}
