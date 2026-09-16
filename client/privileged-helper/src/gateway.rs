use std::{
    fmt::Write as _,
    fs::{self, File, Metadata},
    io::{self, Read as _, Write as _},
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::Path,
};

use natsume_local_control_api::{ResourceControlError, is_canonical_gateway_hostname};
use serde_json::{Map, Value, json};

const HOSTS: &str = "etc/hosts";
const POLICY: &str = "etc/firefox/policies/policies.json";
const SUBMIT: &str = "etc/natsume/submit.env";
const BEGIN: &str = "# BEGIN NATSUME GATEWAY";
const END: &str = "# END NATSUME GATEWAY";
const MAX_FILE_SIZE: u64 = 1024 * 1024;

/// Applies the Daemon-supplied hostname to fixed local files, without reading deployment config.
pub(super) fn configure(root: &Path, hostname: &str) -> Result<(), ResourceControlError> {
    if !is_canonical_gateway_hostname(hostname) {
        return Err(ResourceControlError::Rejected(
            "Gateway hostname must be a canonical DNS name".to_owned(),
        ));
    }
    let hosts = read_file(root, HOSTS)?.ok_or_else(|| rejected(HOSTS, "file is missing"))?;
    let policy = read_file(root, POLICY)?;
    let submit = read_file(root, SUBMIT)?;
    let submit_content = format!("SUBMITBASEURL='https://{hostname}/'\n");
    let submit_changed = submit.as_ref().is_none_or(|(content, meta)| {
        content != &submit_content
            || meta.mode() & 0o7777 != 0o644
            || meta.gid() != rustix::process::getegid().as_raw()
    });
    let (new_hosts, previous) = hosts_content(&hosts.0, hostname)?;
    let mut document: Value = match &policy {
        Some((text, _)) => {
            serde_json::from_str(text).map_err(|_| rejected(POLICY, "invalid JSON"))?
        }
        None => json!({"policies": {}}),
    };
    let original = document.clone();
    firefox_policy(&mut document, hostname, &previous)?;
    let policy_changed = policy.is_none() || document != original;
    let hosts_changed = new_hosts != hosts.0;

    // Validate all inputs before any replacement. Each file is atomic; failures
    // are repaired by replay. Hosts goes last to retain the previous owned hostname
    // until its browser references and submit settings have been migrated.
    if policy_changed {
        let content = serde_json::to_string_pretty(&document)
            .map_err(|_| rejected(POLICY, "cannot serialize policy"))?
            + "\n";
        replace(
            root,
            POLICY,
            &content,
            policy.as_ref().map(|(_, meta)| meta),
        )?;
    }
    if submit_changed {
        replace(root, SUBMIT, &submit_content, None)?;
    }
    if hosts_changed {
        replace(root, HOSTS, &new_hosts, Some(&hosts.1))?;
    }
    tracing::info!(
        gateway_hostname = hostname,
        hosts_changed,
        policy_changed,
        submit_changed,
        "local Gateway settings synchronized"
    );
    Ok(())
}

fn hosts_content(
    text: &str,
    hostname: &str,
) -> Result<(String, Vec<String>), ResourceControlError> {
    let mut output = String::new();
    let mut inside = false;
    let mut seen = false;
    let mut previous = Vec::new();
    for line in text.split_inclusive('\n') {
        match line.trim() {
            BEGIN if !seen => {
                seen = true;
                inside = true;
            }
            END if inside => inside = false,
            BEGIN | END => return Err(rejected(HOSTS, "malformed Natsume Gateway block")),
            _ if inside => {
                for alias in line
                    .split('#')
                    .next()
                    .unwrap_or_default()
                    .split_whitespace()
                    .skip(1)
                {
                    if is_canonical_gateway_hostname(alias) {
                        previous.push(alias.to_owned());
                    }
                }
            }
            _ => output.push_str(&remove_alias(line, hostname)),
        }
    }
    if inside {
        return Err(rejected(HOSTS, "unterminated Natsume Gateway block"));
    }
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    let _ = write!(
        output,
        "{BEGIN}\n127.0.0.1 {hostname}\n::1 {hostname}\n{END}\n"
    );
    Ok((output, previous))
}

fn remove_alias(line: &str, hostname: &str) -> String {
    let (entry, comment) = line.split_once('#').unwrap_or((line, ""));
    let mut fields = entry.split_whitespace();
    let ip = fields.next().unwrap_or_default();
    let aliases: Vec<_> = fields.collect();
    let keep: Vec<_> = aliases
        .iter()
        .copied()
        .filter(|alias| !alias.trim_end_matches('.').eq_ignore_ascii_case(hostname))
        .collect();
    if keep.len() == aliases.len() {
        return line.to_owned();
    }
    let mut result = if keep.is_empty() {
        String::new()
    } else {
        format!("{ip}\t{}", keep.join(" "))
    };
    if !comment.is_empty() {
        if !result.is_empty() {
            result.push(' ');
        }
        result.push('#');
        result.push_str(comment.trim_end_matches('\n'));
    }
    if !result.is_empty() {
        result.push('\n');
    }
    result
}

fn object<'a>(value: &'a mut Value, key: &str) -> Result<&'a mut Value, ResourceControlError> {
    let parent = value
        .as_object_mut()
        .ok_or_else(|| rejected(POLICY, "expected JSON object"))?;
    let field = parent
        .entry(key)
        .or_insert_with(|| Value::Object(Map::new()));
    if !field.is_object() {
        return Err(rejected(POLICY, "expected JSON object"));
    }
    Ok(field)
}

fn firefox_policy(
    document: &mut Value,
    hostname: &str,
    previous: &[String],
) -> Result<(), ResourceControlError> {
    let policies = object(document, "policies")?;
    let url = format!("https://{hostname}/");
    let homepage = object(policies, "Homepage")?;
    homepage["URL"] = json!(url);
    homepage["Locked"] = json!(true);
    homepage["StartPage"] = json!("homepage-locked");
    if policies.get("Bookmarks").is_none() {
        policies["Bookmarks"] = json!([]);
    }
    let bookmarks = policies["Bookmarks"]
        .as_array_mut()
        .ok_or_else(|| rejected(POLICY, "Bookmarks must be an array"))?;
    let mut found = false;
    for bookmark in bookmarks.iter_mut().filter(|bookmark| {
        bookmark["Title"] == "Contest Site" && bookmark["Placement"] == "toolbar"
    }) {
        bookmark["URL"] = json!(url);
        found = true;
    }
    if !found {
        bookmarks.push(json!({"Title": "Contest Site", "Placement": "toolbar", "URL": url}));
    }
    // Migrate only the image's legacy allowance and previously owned Gateway
    // origins. Unrelated notification permissions and CA policy are untouched.
    if let Some(allow) = policies.pointer_mut("/Permissions/Notifications/Allow") {
        let allow = allow
            .as_array_mut()
            .ok_or_else(|| rejected(POLICY, "Notifications.Allow must be an array"))?;
        for origin in allow {
            if origin.as_str().is_some_and(|value| {
                let value = value.trim_end_matches('/');
                value == "https://domjudge"
                    || previous.iter().any(|old| value == format!("https://{old}"))
            }) {
                *origin = json!(url);
            }
        }
    }
    Ok(())
}

fn rejected(path: &str, reason: &str) -> ResourceControlError {
    ResourceControlError::Rejected(format!("/{path}: {reason}"))
}

fn unavailable(path: &str, error: &io::Error) -> ResourceControlError {
    ResourceControlError::Unavailable(format!("/{path}: {error}"))
}

fn trusted(path: &str, metadata: &Metadata) -> Result<(), ResourceControlError> {
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o002 != 0
        || (metadata.mode() & 0o020 != 0 && metadata.gid() != rustix::process::getegid().as_raw())
    {
        return Err(rejected(
            path,
            "must be owned by root and writable only by root or the root group",
        ));
    }
    Ok(())
}

fn parents(root: &Path, relative: &str, create: bool) -> Result<(), ResourceControlError> {
    let mut path = root.to_owned();
    let parent = Path::new(relative)
        .parent()
        .ok_or_else(|| rejected(relative, "missing parent"))?;
    for component in parent.components() {
        path.push(component);
        if create && !path.exists() {
            fs::create_dir(&path).map_err(|e| unavailable(relative, &e))?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .map_err(|e| unavailable(relative, &e))?;
        }
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if !create && e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(unavailable(relative, &e)),
        };
        if !meta.is_dir() {
            return Err(rejected(relative, "parent must be a real directory"));
        }
        let actual = path
            .strip_prefix(root)
            .map_err(|_| rejected(relative, "invalid parent"))?;
        trusted(&actual.to_string_lossy(), &meta)?;
    }
    Ok(())
}

fn read_file(
    root: &Path,
    relative: &str,
) -> Result<Option<(String, Metadata)>, ResourceControlError> {
    parents(root, relative, false)?;
    let path = root.join(relative);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(unavailable(relative, &e)),
    };
    if !metadata.is_file() {
        return Err(rejected(relative, "must be a regular file, not a symlink"));
    }
    trusted(relative, &metadata)?;
    let mut content = String::new();
    File::open(path)
        .and_then(|file| file.take(MAX_FILE_SIZE + 1).read_to_string(&mut content))
        .map_err(|e| unavailable(relative, &e))?;
    if content.len() as u64 > MAX_FILE_SIZE {
        return Err(rejected(relative, "file exceeds 1 MiB"));
    }
    Ok(Some((content, metadata)))
}

fn replace(
    root: &Path,
    relative: &str,
    content: &str,
    metadata: Option<&Metadata>,
) -> Result<(), ResourceControlError> {
    parents(root, relative, true)?;
    let path = root.join(relative);
    let parent = path
        .parent()
        .ok_or_else(|| rejected(relative, "missing parent"))?;
    let write = || -> io::Result<()> {
        let mut temporary = tempfile::Builder::new()
            .prefix(".natsume-gateway-")
            .tempfile_in(parent)?;
        if let Some(meta) = metadata {
            rustix::fs::fchown(
                temporary.as_file(),
                Some(rustix::process::Uid::from_raw(meta.uid())),
                Some(rustix::process::Gid::from_raw(meta.gid())),
            )?;
        }
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(
                metadata.map_or(0o644, Metadata::mode),
            ))?;
        temporary.write_all(content.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary.persist(&path).map_err(|e| e.error)?;
        File::open(parent)?.sync_all()
    };
    write().map_err(|e| unavailable(relative, &e))
}

#[cfg(test)]
mod tests;
