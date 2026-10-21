use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde::Serialize;
use tokio::{
    process::Command,
    time::{Instant, sleep_until},
};
use tokio_util::sync::CancellationToken;

use crate::{
    Error, Result,
    config::{Permission, read_regular},
    wire::{Fact, FailureCode},
};

#[derive(Clone)]
pub struct Runtime {
    pub compiler: PathBuf,
    pub template: PathBuf,
    pub fonts: PathBuf,
}

impl Runtime {
    #[must_use]
    pub fn installed() -> Self {
        Self {
            compiler: "/usr/lib/natsume-print-domjudge/typst".into(),
            template: "/usr/share/natsume-print-domjudge/template.typ".into(),
            fonts: "/usr/share/natsume-print-domjudge/fonts".into(),
        }
    }
}

#[derive(Serialize)]
struct DisplayData<'a> {
    request_id: &'a str,
    account: &'a str,
    team_name: &'a str,
    seat_label: Option<&'a str>,
    original_filename: &'a str,
}

pub(crate) async fn compile(
    runtime: &Runtime,
    fact: &Fact,
    original: &str,
    source: &[u8],
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>> {
    if cancellation.is_cancelled() || Instant::now() >= deadline {
        return Err(Error::Generation(FailureCode::WrapperDeadline));
    }
    let root = tempfile::Builder::new()
        .prefix("natsume-print-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .map_err(|_| Error::Generation(FailureCode::CompileFailed))?;
    prepare(root.path(), runtime, fact, original, source)?;
    let mut command = Command::new(&runtime.compiler);
    command
        .env_clear()
        .current_dir(root.path())
        .args([
            "compile",
            "--root",
            ".",
            "--ignore-system-fonts",
            "--ignore-embedded-fonts",
        ])
        .arg("--font-path")
        .arg(&runtime.fonts)
        .args([
            "--package-path",
            ".",
            "--package-cache-path",
            ".",
            "--jobs",
            "1",
            "--creation-timestamp",
            "0",
            "--no-pdf-tags",
            "template.typ",
            "document.pdf",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| Error::Generation(FailureCode::CompileFailed))?;
    let compile_deadline = Instant::now() + Duration::from_secs(2);
    let failure = tokio::select! {
        biased;
        () = cancellation.cancelled() => Some(FailureCode::WrapperDeadline),
        () = sleep_until(deadline) => Some(FailureCode::WrapperDeadline),
        () = sleep_until(compile_deadline) => Some(FailureCode::CompileTimeout),
        status = child.wait() => {
            if status.is_ok_and(|status| status.success()) { None }
            else { Some(FailureCode::CompileFailed) }
        }
    };
    if let Some(code) = failure {
        // Explicit wait is necessary: killing on Drop alone does not prove reaping.
        let _ = child.kill().await;
        let _ = child.wait().await;
        return Err(Error::Generation(code));
    }
    let pdf = root.path().join("document.pdf");
    if std::fs::metadata(&pdf).is_ok_and(|m| m.len() > 8 * 1024 * 1024) {
        return Err(Error::Generation(FailureCode::DocumentTooLarge));
    }
    let bytes = read_regular(&pdf, 8 * 1024 * 1024, Permission::Source)
        .map_err(|_| Error::Generation(FailureCode::CompileFailed))?;
    if !bytes.starts_with(b"%PDF-") {
        return Err(Error::Generation(FailureCode::CompileFailed));
    }
    Ok(bytes)
}

fn prepare(
    root: &Path,
    runtime: &Runtime,
    fact: &Fact,
    original: &str,
    source: &[u8],
) -> Result<()> {
    let data = DisplayData {
        request_id: &fact.request_id,
        account: &fact.account,
        team_name: &fact.team_name,
        seat_label: fact.seat_label.as_deref(),
        original_filename: original,
    };
    let data =
        serde_json::to_vec(&data).map_err(|_| Error::Generation(FailureCode::CompileFailed))?;
    std::fs::copy(&runtime.template, root.join("template.typ"))
        .and_then(|_| std::fs::write(root.join("source.txt"), source))
        .and_then(|()| std::fs::write(root.join("display.json"), data))
        .map_err(|_| Error::Generation(FailureCode::CompileFailed))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{error::Error as StdError, os::unix::fs::PermissionsExt};

    type TestResult = std::result::Result<(), Box<dyn StdError>>;

    fn preparing() -> std::result::Result<Fact, serde_json::Error> {
        serde_json::from_value(serde_json::json!({
            "protocol_version": 1, "request_id": "01900000-0000-7000-8000-000000000001",
            "account_id": "01900000-0000-7000-8000-000000000002", "account": "account",
            "team_name": "中文队伍 #panic(\"data\")", "seat_label": "座位一",
            "state": "preparing", "document": null, "admission": null, "generation_failure": null,
        }))
    }

    async fn child_cleanup(cancel: bool, whole_deadline: bool) -> TestResult {
        let root = tempfile::tempdir()?;
        let marker = root.path().join("child");
        let compiler = root.path().join("compiler");
        std::fs::write(
            &compiler,
            format!(
                "#!/usr/bin/python3\nimport os, time\nfrom pathlib import Path\nPath({marker:?}).write_text(str(os.getpid()) + '\\n' + str(Path.cwd()))\nwhile True:\n os.write(1, b'x'*65536)\n os.write(2, b'x'*65536)\n time.sleep(.001)\n"
            ),
        )?;
        std::fs::set_permissions(&compiler, std::fs::Permissions::from_mode(0o700))?;
        let template = root.path().join("template.typ");
        std::fs::write(&template, "fixed")?;
        let runtime = Runtime {
            compiler,
            template,
            fonts: root.path().to_owned(),
        };
        let cancellation = CancellationToken::new();
        let token = cancellation.clone();
        let deadline = Instant::now()
            + if whole_deadline {
                Duration::from_secs(1)
            } else {
                Duration::from_secs(5)
            };
        let handle = tokio::spawn(async move {
            compile(
                &runtime,
                &preparing().map_err(|_| Error::InvalidResponse)?,
                "file.cpp",
                b"source",
                deadline,
                &token,
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await?;
        let child = std::fs::read_to_string(&marker)?;
        let mut lines = child.lines();
        let pid = lines.next().ok_or("pid missing")?;
        let compilation_root = lines.next().ok_or("root missing")?;
        let mode = std::fs::metadata(compilation_root)?.permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        if cancel {
            cancellation.cancel();
        } else {
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(if whole_deadline { 1 } else { 2 })).await;
        }
        let result = handle.await?;
        assert_eq!(
            result.err(),
            Some(Error::Generation(if cancel || whole_deadline {
                FailureCode::WrapperDeadline
            } else {
                FailureCode::CompileTimeout
            }))
        );
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "compiler was not reaped"
        );
        assert!(
            !Path::new(compilation_root).exists(),
            "ephemeral input survived"
        );
        Ok(())
    }

    #[tokio::test]
    async fn compile_timeout_kills_and_reaps_noisy_child() -> TestResult {
        child_cleanup(false, false).await
    }

    #[tokio::test]
    async fn whole_deadline_kills_and_reaps_child() -> TestResult {
        child_cleanup(false, true).await
    }

    #[tokio::test]
    async fn cancellation_kills_and_reaps_child() -> TestResult {
        child_cleanup(true, false).await
    }

    #[tokio::test]
    #[ignore = "prepare assets with deno task prepare print-domjudge; requires Poppler tools"]
    async fn real_typst_wraps_long_lines_preserves_multilingual_data_and_is_deterministic()
    -> TestResult {
        let assets =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/print-domjudge/runtime");
        let runtime = Runtime {
            compiler: assets.join("typst"),
            template: assets.join("template.typ"),
            fonts: assets.join("fonts"),
        };
        let source = format!(
            "{}TAIL_FINAL_末尾\n中文源代码：你好世界\n#panic(\"not executable\")\n    indented\nLAST_CHARACTER_終",
            "abcdefghij".repeat(1000)
        );
        let fact = preparing()?;
        let cancellation = CancellationToken::new();
        let first = compile(
            &runtime,
            &fact,
            "原始文件 #panic.cpp",
            source.as_bytes(),
            Instant::now() + Duration::from_secs(5),
            &cancellation,
        )
        .await?;
        let second = compile(
            &runtime,
            &fact,
            "原始文件 #panic.cpp",
            source.as_bytes(),
            Instant::now() + Duration::from_secs(5),
            &cancellation,
        )
        .await?;
        assert_eq!(
            first, second,
            "same-ID retries must upload identical PDF bytes"
        );
        let output = tempfile::tempdir()?;
        let pdf = output.path().join("document.pdf");
        std::fs::write(&pdf, first)?;
        let text = std::process::Command::new("pdftotext")
            .args(["-layout"])
            .arg(&pdf)
            .arg("-")
            .output()?;
        assert!(text.status.success());
        let text = String::from_utf8(text.stdout)?
            .replace('\u{200b}', "")
            .replace('\u{a0}', " ");
        for value in [
            "TAIL_FINAL_末尾",
            "中文源代码：你好世界",
            "中文队伍",
            "座位一",
            "#panic(\"not executable\")",
            "LAST_CHARACTER_終",
            "    indented",
        ] {
            assert!(text.contains(value), "missing rendered data: {value}");
        }
        let source_only = text.split_once("abcdefghij").ok_or("source missing")?.1;
        let visible = source_only
            .split("TAIL_FINAL_末尾")
            .next()
            .ok_or("tail missing")?
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>();
        assert_eq!(visible, "abcdefghij".repeat(999));
        let info = std::process::Command::new("pdfinfo").arg(&pdf).output()?;
        let info = String::from_utf8(info.stdout)?;
        assert!(info.contains("(A4)"));
        assert!(info.contains("Typst 0.14.2"));
        // Every non-whitespace glyph must stay inside the A4 content bounds.
        let boxes = std::process::Command::new("pdftotext")
            .args(["-bbox"])
            .arg(&pdf)
            .arg("-")
            .output()?;
        let boxes = String::from_utf8(boxes.stdout)?;
        for line in boxes.lines().filter(|line| line.contains("<word ")) {
            let number = |attribute: &str| -> std::result::Result<f64, Box<dyn StdError>> {
                Ok(line
                    .split_once(&format!("{attribute}=\""))
                    .ok_or("missing coordinate")?
                    .1
                    .split('"')
                    .next()
                    .ok_or("coordinate missing")?
                    .parse()?)
            };
            assert!(
                number("xMin")? >= 40.0 && number("xMax")? <= 555.0,
                "horizontal clipping: {line}"
            );
            assert!(
                number("yMin")? >= 40.0 && number("yMax")? <= 802.0,
                "vertical clipping: {line}"
            );
        }
        Ok(())
    }
}
