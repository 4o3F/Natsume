use std::{
    fmt::Write as _,
    fs,
    io::{Cursor, Write as _},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::PathBuf,
};

use clap::Args;
use image::{ExtendedColorType, codecs::webp::WebPEncoder};
use snafu::ResultExt as _;
use zip::{ZipWriter, write::SimpleFileOptions};

use crate::{
    error::{self, Error, Result},
    operator::{Credentials, Operator},
    scenario::Scenario,
};

pub(crate) const XLSX_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

#[derive(Args)]
pub(crate) struct Setup {
    #[arg(long)]
    server: String,
    #[arg(long)]
    trust_root: PathBuf,
    #[command(flatten)]
    operator: Credentials,
    /// New private directory for the roster and seat plan.
    #[arg(long)]
    output_dir: PathBuf,
    /// Server `storage.organization_logos` directory, mounted locally if Server is remote.
    #[arg(long)]
    logo_directory: PathBuf,
    /// Use synthetic files already present in logo-directory.
    #[arg(long)]
    reuse_logos: bool,
    #[arg(long, default_value_t = 600)]
    teams: usize,
    /// Defaults to ceil(teams / 3).
    #[arg(long)]
    schools: Option<usize>,
    /// Exact even byte length, including a RIFF padding chunk.
    #[arg(long, default_value_t = 120_832)]
    logo_bytes: usize,
    #[arg(long, default_value_t = 256)]
    logo_dimension: u32,
    #[arg(long)]
    scenario: Option<PathBuf>,
}

pub(crate) async fn execute(mut setup: Setup) -> Result<()> {
    if let Some(path) = &setup.scenario {
        let scenario = Scenario::load(path)?;
        // Fleet and setup share the same roster size and logo profile.
        if let Some(devices) = scenario.device_count() {
            setup.teams = devices;
        }
        setup.schools = scenario.schools.or(setup.schools);
        setup.logo_bytes = scenario.logo_bytes.unwrap_or(setup.logo_bytes);
        setup.logo_dimension = scenario.logo_dimension.unwrap_or(setup.logo_dimension);
    }
    let schools = setup.schools.unwrap_or(setup.teams.div_ceil(3));
    if setup.teams == 0
        || setup.teams > 100_000
        || schools == 0
        || schools > setup.teams
        || !crate::target::canonical_origin(&setup.server)
    {
        return Err(Error::Invalid {
            code: "SETUP_SIZE_OR_ORIGIN",
        });
    }
    let mut operator = Operator::new(&setup.server, &setup.trust_root, &setup.operator, None)?;
    operator.login().await?;
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&setup.output_dir)
        .context(error::IoSnafu)?;
    fs::create_dir_all(&setup.logo_directory).context(error::IoSnafu)?;
    let dimension = setup.logo_dimension;
    let bytes = setup.logo_bytes;
    let directory = setup.logo_directory.clone();
    let workbook = tokio::task::spawn_blocking(move || {
        for school in 0..schools {
            if setup.reuse_logos {
                continue;
            }
            let image = logo(dimension, bytes, school)?;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o644)
                .open(directory.join(format!("{}.webp", school_name(school))))
                .context(error::IoSnafu)?
                .write_all(&image)
                .context(error::IoSnafu)?;
        }
        roster(setup.teams, schools)
    })
    .await
    .context(error::JoinSnafu)??;
    write_private(&setup.output_dir.join("roster.xlsx"), &workbook)?;
    let mut seats = String::from("seat_code\n");
    for i in 0..setup.teams {
        let _ = writeln!(seats, "S{i:05}");
    }
    write_private(&setup.output_dir.join("seats.csv"), seats.as_bytes())?;
    operator.import(workbook).await?;
    operator.open_window().await?;
    let organizations = operator.organizations().await?;
    if organizations.len() != schools {
        return Err(Error::Invalid {
            code: "SETUP_ORGANIZATIONS",
        });
    }
    for school in organizations {
        let id = school["organization_id"].as_str().ok_or(Error::Invalid {
            code: "ORGANIZATION_RESPONSE",
        })?;
        operator.check_logo(id).await?;
    }
    tracing::info!(
        teams = setup.teams,
        schools,
        logo_bytes = bytes,
        logo_dimension = dimension,
        "synthetic roster imported, logos checked and auto-approval opened"
    );
    Ok(())
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .context(error::IoSnafu)?
        .write_all(bytes)
        .context(error::IoSnafu)
}
fn school_name(school: usize) -> String {
    format!("Synthetic School {school:05}")
}

pub(crate) fn roster(teams: usize, schools: usize) -> Result<Vec<u8>> {
    let headers = [
        "organization_zh",
        "organization_en",
        "country",
        "account",
        "password",
        "seat",
        "team_name_zh",
        "team_name_en",
        "category",
    ];
    let mut sheet = String::from(
        "<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>",
    );
    for row in 0..=teams {
        let values = if row == 0 {
            headers.map(str::to_owned)
        } else {
            let i = row - 1;
            [
                String::new(),
                school_name(i % schools),
                "CHN".to_owned(),
                format!("synthetic{i:05}"),
                "Synthetic-Test-Password".to_owned(),
                format!("S{i:05}"),
                String::new(),
                format!("Synthetic Team {i:05}"),
                "participant".to_owned(),
            ]
        };
        let _ = write!(sheet, "<row r=\"{}\">", row + 1);
        for (column, value) in values.iter().enumerate() {
            let column = char::from(b'A' + u8::try_from(column).unwrap_or_default());
            let _ = write!(
                sheet,
                "<c r=\"{column}{}\" t=\"inlineStr\"><is><t>{value}</t></is></c>",
                row + 1
            );
        }
        sheet.push_str("</row>");
    }
    sheet.push_str("</sheetData></worksheet>");
    let files = [
        (
            "[Content_Types].xml",
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
        ),
        (
            "_rels/.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
        ),
        (
            "xl/workbook.xml",
            r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Teams" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
        ),
        ("xl/worksheets/sheet1.xml", &sheet),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, content) in files {
        zip.start_file(
            name,
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .context(error::ZipSnafu)?;
        zip.write_all(content.as_bytes()).context(error::IoSnafu)?;
    }
    Ok(zip.finish().context(error::ZipSnafu)?.into_inner())
}

fn logo(dimension: u32, bytes: usize, school: usize) -> Result<Vec<u8>> {
    if !(16..=2048).contains(&dimension)
        || !(1024..=8 * 1024 * 1024).contains(&bytes)
        || !bytes.is_multiple_of(2)
    {
        return Err(Error::Invalid {
            code: "LOGO_DIMENSION_OR_BYTES",
        });
    }
    let mut pixels = Vec::new();
    let school = u32::try_from(school).context(error::SizeSnafu)?;
    for y in 0..dimension {
        for x in 0..dimension {
            pixels.extend([
                (x.wrapping_mul(17).wrapping_add(y * 3).wrapping_add(school)).to_le_bytes()[0],
                (y / 8 + school).to_le_bytes()[0],
                (x / 8 + y / 8).to_le_bytes()[0],
            ]);
        }
    }
    let mut output = Vec::new();
    WebPEncoder::new_lossless(&mut output)
        .encode(&pixels, dimension, dimension, ExtendedColorType::Rgb8)
        .context(error::ImageSnafu)?;
    if output.len() + 8 > bytes {
        return Err(Error::Invalid {
            code: "LOGO_BYTES_TOO_SMALL_FOR_DIMENSION",
        });
    }
    // RIFF readers skip unknown chunks. Padding controls transfer size without changing dimensions.
    let padding =
        u32::try_from(bytes - output.len() - 8).map_err(|source| Error::Size { source })?;
    output.extend(b"JUNK");
    output.extend(padding.to_le_bytes());
    output.resize(bytes, 0);
    output[4..8].copy_from_slice(
        &(u32::try_from(bytes - 8).map_err(|source| Error::Size { source })?).to_le_bytes(),
    );
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_webp_has_requested_transfer_size_and_decodable_dimensions() -> Result<()> {
        let bytes = logo(256, 120_832, 7)?;
        assert_eq!(bytes.len(), 120_832);
        let decoded = image::load_from_memory(&bytes).context(error::ImageSnafu)?;
        assert_eq!((decoded.width(), decoded.height()), (256, 256));
        assert!(logo(4096, 120_832, 0).is_err());
        Ok(())
    }
}
