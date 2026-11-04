//! Pyrite CDP assets; the authoritative contest event feed remains external.
use super::*;
use crate::component::contest::{PhotoLogos, PhotoTeam};
use image::{ImageDecoder as _, ImageReader, Limits, Pixel as _, codecs::jpeg::JpegEncoder};
use std::{
    collections::BTreeMap,
    io::{BufReader, Write as _},
};
use zip::{ZipWriter, write::SimpleFileOptions};

const CONFIG: &[u8] = b"[presentation]\nlogo_extension = \"png\"\nteam_photo_extension = \"jpg\"\n";

#[derive(serde::Serialize)]
struct PyriteRoster {
    format_version: u8,
    teams: Vec<PyriteRosterEntry>,
}

#[derive(serde::Serialize)]
struct PyriteRosterEntry {
    id: String,
    organization_id: String,
    school_name_zh: String,
    school_name_en: String,
    team_name_zh: String,
    team_name_en: String,
    photo_path: Option<String>,
    logo_path: Option<String>,
}

impl PhotoComponent {
    pub(crate) async fn prepare_pyrite_export(
        &self,
        teams: Vec<PhotoTeam>,
        entries: Vec<PhotoRecord>,
        logos: PhotoLogos,
        permit: OwnedSemaphorePermit,
        admission: Admission,
    ) -> Result<PreparedDownload> {
        let store = self.results.clone();
        tokio::task::spawn_blocking(move || {
            let started = Instant::now();
            let deadline = started + super::super::limits::OPERATION_TIMEOUT;
            let photo_count = entries.len();
            let mut writer =
                ZipWriter::new(BoundedFile(store.export_file().context(StorageSnafu)?));
            // Pyrite requires these directories even when no images are available.
            for directory in ["teams/", "affiliations/"] {
                writer
                    .add_directory(directory, SimpleFileOptions::default())
                    .context(ZipSnafu)?;
            }
            let mut roster = build_roster_with_photos(
                &mut writer,
                &store,
                teams,
                entries,
                &admission,
                deadline,
            )?;

            let logo_count = write_logos(&mut writer, &mut roster, &logos, &admission, deadline)?;
            drop(logos);

            start_entry(&mut writer, "config.toml", &admission, deadline)?;
            writer.write_all(CONFIG).context(IoSnafu)?;
            start_entry(&mut writer, "roster.json", &admission, deadline)?;
            serde_json::to_writer(&mut writer, &roster).context(JsonSnafu)?;
            let ready = finish_export(writer, permit, &admission, deadline)?;
            tracing::info!(
                photo_count,
                logo_count,
                bytes = ready.length,
                total_ms = started.elapsed().as_secs_f64() * 1000.0,
                "Pyrite export prepared"
            );
            Ok(ready)
        })
        .await
        .context(WorkerSnafu)?
    }
}

fn build_roster_with_photos(
    writer: &mut ZipWriter<BoundedFile>,
    store: &results_storage::ResultsStore,
    teams: Vec<PhotoTeam>,
    entries: Vec<PhotoRecord>,
    admission: &Admission,
    deadline: Instant,
) -> Result<PyriteRoster> {
    ensure!(
        teams.len() <= natsume_photo_protocol::TEAM_LIMIT,
        LimitSnafu
    );
    let mut entries: BTreeMap<_, _> = entries
        .into_iter()
        .map(|entry| (entry.account_id.clone(), entry))
        .collect();
    let total = entries.values().try_fold(0_u64, |total, entry| {
        total
            .checked_add(entry.image_bytes)
            .ok_or_else(|| LimitSnafu.build())
    })?;
    ensure!(total <= RESULT_IMAGES_LIMIT, LimitSnafu);
    let mut roster = PyriteRoster {
        format_version: 1,
        teams: Vec::with_capacity(teams.len()),
    };
    let mut names = BTreeSet::new();
    for team in teams {
        check(admission, deadline)?;
        let name = account_name(&team.account, "jpg")?;
        ensure!(names.insert(name.to_uppercase()), NamesSnafu);
        let photo_path = if let Some(entry) = entries.remove(&team.account_id) {
            let path = format!("teams/{name}");
            let mut file = store.open_object(&entry).context(StorageSnafu)?;
            start_entry(writer, &path, admission, deadline)?;
            match entry.image_format {
                PhotoFormat::Jpeg => copy(&mut file, writer, &entry, admission, deadline)?,
                PhotoFormat::Png => {
                    copy(&mut file, &mut std::io::sink(), &entry, admission, deadline)?;
                    encode_jpeg(&mut file, writer)?;
                }
            }
            Some(path)
        } else {
            None
        };
        roster.teams.push(PyriteRosterEntry {
            id: team.account,
            organization_id: format!("INST-{:03}", team.organization_id),
            school_name_zh: team.school_name_zh,
            school_name_en: team.school_name_en,
            team_name_zh: team.team_name_zh,
            team_name_en: team.team_name_en,
            photo_path,
            logo_path: None,
        });
    }
    ensure!(entries.is_empty(), IntegritySnafu);
    Ok(roster)
}

fn write_logos(
    writer: &mut ZipWriter<BoundedFile>,
    roster: &mut PyriteRoster,
    logos: &PhotoLogos,
    admission: &Admission,
    deadline: Instant,
) -> Result<usize> {
    let schools: BTreeMap<_, _> = roster
        .teams
        .iter()
        .map(|team| {
            (
                team.organization_id.as_str(),
                (team.school_name_zh.as_str(), team.school_name_en.as_str()),
            )
        })
        .collect();
    let mut available = BTreeSet::new();
    for (id, (zh, en)) in schools {
        check(admission, deadline)?;
        if let Some(bytes) = logos.png(zh, en).context(LogoSnafu)? {
            start_entry(
                writer,
                &format!("affiliations/{id}.png"),
                admission,
                deadline,
            )?;
            writer.write_all(&bytes).context(IoSnafu)?;
            available.insert(id.to_owned());
        }
    }
    for team in &mut roster.teams {
        if available.contains(&team.organization_id) {
            team.logo_path = Some(format!("affiliations/{}.png", team.organization_id));
        }
    }
    Ok(available.len())
}

fn encode_jpeg(file: &mut File, writer: &mut impl std::io::Write) -> Result<()> {
    file.rewind().context(IoSnafu)?;
    let mut reader = ImageReader::with_format(BufReader::new(file), image::ImageFormat::Png);
    let mut limits = Limits::default();
    limits.max_alloc = Some(crate::component::photo::limits::IMAGE_DECODE_BYTES);
    reader.limits(limits);
    let decoder = reader.into_decoder().context(ImageSnafu)?;
    let (width, height) = decoder.dimensions();
    ensure!(
        u64::from(width) * u64::from(height) <= natsume_photo_protocol::results::RESULT_PIXEL_LIMIT
            && decoder.total_bytes() <= crate::component::photo::limits::IMAGE_DECODE_BYTES,
        LimitSnafu
    );
    let rgba = image::DynamicImage::from_decoder(decoder)
        .context(ImageSnafu)?
        .into_rgba8();
    let rgb = image::RgbImage::from_fn(width, height, |x, y| {
        let mut background = image::Rgba([255_u8; 4]);
        background.blend(rgba.get_pixel(x, y));
        background.to_rgb()
    });
    JpegEncoder::new_with_quality(writer, 95)
        .encode_image(&rgb)
        .context(ImageSnafu)
}
