use std::{fs, io::Write as _, os::unix::fs::OpenOptionsExt as _, path::PathBuf};

use clap::{Args, ValueEnum};
use image::{ExtendedColorType, codecs::webp::WebPEncoder};
use serde::Deserialize;
use snafu::ResultExt as _;

use crate::{
    error::{self, Error, Result},
    identity::Samples,
};

#[derive(Clone, Copy, Debug, Default, Deserialize, ValueEnum, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum Encoder {
    #[default]
    Image,
    Libwebp,
    /// Add a seeded opacity texture in 8-pixel cells.
    LibwebpRgba,
}

#[derive(Args)]
pub(crate) struct GenerateLogos {
    /// Aggregate targets: `synthetic_140_stratified_targets` with width, height, bytes and count.
    #[arg(long)]
    profile: PathBuf,
    #[arg(long)]
    output_dir: PathBuf,
    #[arg(long, default_value_t = 1033)]
    directory_count: usize,
    #[arg(long, default_value_t = 418)]
    seed: u64,
    /// Percentage of the image over which seeded noise is spread; zero keeps a contiguous patch.
    #[arg(long, value_parser = clap::value_parser!(u32).range(0..=100))]
    noise_spread: Option<u32>,
    /// Lossless encoder; libwebp supports adaptive prediction and opacity texture.
    #[arg(long, value_enum)]
    encoder: Option<Encoder>,
    /// Number of seeded filled ellipses beneath the noise.
    #[arg(long, value_parser = clap::value_parser!(u32).range(0..=256))]
    detail_shapes: Option<u32>,
    /// libwebp lossless compression effort, from 0 to 6.
    #[arg(long, value_parser = clap::value_parser!(u32).range(0..=6))]
    webp_method: Option<u32>,
}
#[derive(Clone, Deserialize)]
struct Target {
    #[serde(default)]
    alpha: bool,
    width: u32,
    height: u32,
    bytes: usize,
    count: usize,
}
#[derive(Deserialize)]
struct Profile {
    webp_method: Option<u32>,
    #[serde(default)]
    encoder: Encoder,
    #[serde(default)]
    detail_shapes: u32,
    noise_spread: Option<u32>,
    synthetic_140_stratified_targets: Vec<Target>,
}

pub(crate) fn execute(options: GenerateLogos) -> Result<()> {
    let profile: Profile =
        serde_json::from_slice(&fs::read(options.profile).context(error::IoSnafu)?)
            .context(error::JsonSnafu)?;
    let noise_spread = options.noise_spread.or(profile.noise_spread);
    let codec = options.encoder.unwrap_or(profile.encoder);
    let detail_shapes = options.detail_shapes.unwrap_or(profile.detail_shapes);
    let webp_method = options.webp_method.or(profile.webp_method).unwrap_or(2);
    if webp_method > 6 {
        return Err(error::InvalidSnafu {
            code: "LOGO_WEBP_METHOD",
        }
        .build());
    }
    if detail_shapes > 256 {
        return Err(Error::Invalid {
            code: "LOGO_DETAIL_SHAPES",
        });
    }
    if noise_spread.is_some_and(|spread| spread > 100) {
        return Err(Error::Invalid {
            code: "LOGO_NOISE_SPREAD",
        });
    }
    let mut targets = Vec::new();
    for row in profile.synthetic_140_stratified_targets {
        if row.count > 10000 || targets.len() + row.count > 10000 {
            return Err(Error::Invalid {
                code: "LOGO_PROFILE_SIZE",
            });
        }
        targets.extend(std::iter::repeat_n(row.clone(), row.count));
    }
    if targets.is_empty()
        || options.directory_count < targets.len()
        || options.directory_count > 100_000
    {
        return Err(Error::Invalid {
            code: "LOGO_PROFILE_SIZE",
        });
    }
    let mut shuffle = Samples::new(options.seed, 0, "logo-shuffle");
    for index in (1..targets.len()).rev() {
        let other = usize::try_from(shuffle.uniform(index as u64)).context(error::SizeSnafu)?;
        targets.swap(index, other);
    }
    fs::create_dir(&options.output_dir).context(error::IoSnafu)?;
    let mut measured = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        let (bytes, width, height, encoded) = generate(
            target,
            options.seed,
            index,
            noise_spread,
            codec,
            detail_shapes,
            webp_method,
        )?;
        write(
            &options
                .output_dir
                .join(format!("Synthetic School {index:05}.webp")),
            &bytes,
        )?;
        measured.push(serde_json::json!({"width":width,"height":height,"bytes":bytes.len(),"encoded_bytes":encoded,"alpha":target.alpha || matches!(codec, Encoder::LibwebpRgba)}));
    }
    for index in targets.len()..options.directory_count {
        let source = options.output_dir.join(format!(
            "Synthetic School {:05}.webp",
            (index - targets.len()) % targets.len()
        ));
        write(
            &options.output_dir.join(format!(
                "Synthetic Unused {:05}.webp",
                index - targets.len()
            )),
            &fs::read(source).context(error::IoSnafu)?,
        )?;
    }
    let manifest = serde_json::json!({"seed":options.seed,"used_count":targets.len(),"directory_count":options.directory_count,"encoder":codec,"detail_shapes":detail_shapes,"webp_method":webp_method,"webp_quality":80,"noise_spread":noise_spread,"rows":measured});
    write(
        &options.output_dir.join("profile.json"),
        &serde_json::to_vec_pretty(&manifest).context(error::JsonSnafu)?,
    )?;
    tracing::info!(
        count = targets.len(),
        directory_count = options.directory_count,
        "synthetic logos generated"
    );
    Ok(())
}
fn write(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(path)
        .context(error::IoSnafu)?
        .write_all(bytes)
        .context(error::IoSnafu)
}
fn background(
    width: u32,
    height: u32,
    seed: u64,
    index: usize,
    detail_shapes: u32,
    codec: Encoder,
) -> Result<Vec<u8>> {
    let pixels = width as usize * height as usize;
    let mut clean = vec![0; pixels * 3];
    for (pixel, color) in clean.chunks_exact_mut(3).enumerate() {
        let x = pixel % width as usize;
        let y = pixel / width as usize;
        color.copy_from_slice(&match codec {
            Encoder::Image => [
                180 + u8::try_from(x / 64 % 3).context(error::SizeSnafu)? * 10,
                200 + u8::try_from(y / 64 % 3).context(error::SizeSnafu)? * 10,
                220,
            ],
            Encoder::Libwebp | Encoder::LibwebpRgba => [240, 240, 250],
        });
    }
    let mut shapes = Samples::new(seed, index, "logo-shapes");
    for _ in 0..detail_shapes {
        let left = u32::try_from(shapes.uniform(u64::from(width - 1))).context(error::SizeSnafu)?;
        let top = u32::try_from(shapes.uniform(u64::from(height - 1))).context(error::SizeSnafu)?;
        let right = (left + width / 4).min(width - 1);
        let bottom = (top + height / 4).min(height - 1);
        let color = [
            u8::try_from(shapes.uniform(255)).context(error::SizeSnafu)?,
            u8::try_from(shapes.uniform(255)).context(error::SizeSnafu)?,
            u8::try_from(shapes.uniform(255)).context(error::SizeSnafu)?,
        ];
        let rx = f64::from(right - left).max(1.0) / 2.0;
        let ry = f64::from(bottom - top).max(1.0) / 2.0;
        for y in top..=bottom {
            for x in left..=right {
                let dx = (f64::from(x - left) - rx) / rx;
                let dy = (f64::from(y - top) - ry) / ry;
                if dx * dx + dy * dy <= 1.0 {
                    let offset = (y as usize * width as usize + x as usize) * 3;
                    clean[offset..offset + 3].copy_from_slice(&color);
                }
            }
        }
    }
    Ok(clean)
}

fn encode(
    rgb: &[u8],
    width: u32,
    height: u32,
    codec: Encoder,
    seed: u64,
    webp_method: u32,
) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    match codec {
        Encoder::Image => WebPEncoder::new_lossless(&mut output)
            .encode(rgb, width, height, ExtendedColorType::Rgb8)
            .context(error::ImageSnafu)?,
        Encoder::Libwebp | Encoder::LibwebpRgba => {
            let mut config = webp::WebPConfig::new().map_err(|()| {
                error::InvalidSnafu {
                    code: "LOGO_ENCODER_CONFIG",
                }
                .build()
            })?;
            config.lossless = 1;
            config.method = i32::try_from(webp_method).context(error::SizeSnafu)?;
            config.quality = 80.0;
            let rgba;
            let source = if matches!(codec, Encoder::LibwebpRgba) {
                rgba = rgb
                    .chunks_exact(3)
                    .enumerate()
                    .flat_map(|(pixel, color)| {
                        let x = pixel % width as usize / 8;
                        let y = pixel / width as usize / 8;
                        let alpha = 224
                            + ((x * 37 + y * 19 + usize::from(seed.to_le_bytes()[0])) % 32)
                                .to_le_bytes()[0];
                        [color[0], color[1], color[2], alpha]
                    })
                    .collect::<Vec<_>>();
                webp::Encoder::from_rgba(&rgba, width, height)
            } else {
                webp::Encoder::from_rgb(rgb, width, height)
            };
            let encoded = source
                .encode_advanced(&config)
                .map_err(|reason| error::LogoEncodeSnafu { reason }.build())?;
            output.extend_from_slice(&encoded);
        }
    }
    Ok(output)
}

fn generate(
    target: &Target,
    seed: u64,
    index: usize,
    noise_spread: Option<u32>,
    codec: Encoder,
    detail_shapes: u32,
    webp_method: u32,
) -> Result<(Vec<u8>, u32, u32, usize)> {
    let codec = if target.alpha && matches!(codec, Encoder::Libwebp) {
        Encoder::LibwebpRgba
    } else {
        codec
    };
    let width = target.width.min(2048);
    let height = target.height.min(2048);
    let budget = target.bytes.next_multiple_of(2);
    if width == 0 || height == 0 || !(1024..=8 * 1024 * 1024).contains(&budget) {
        return Err(Error::Invalid {
            code: "LOGO_PROFILE_TARGET",
        });
    }
    let pixels = usize::try_from(width * height).context(error::SizeSnafu)?;
    let mut random = seed ^ (index as u64).wrapping_add(2026);
    let mut noise = vec![0; pixels * 3];
    for value in &mut noise {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        *value = random.to_le_bytes()[0];
    }
    let clean = background(width, height, seed, index, detail_shapes, codec)?;
    let mut noise_pixels = (budget / 3).saturating_sub(2000).min(pixels);
    let mut output = Vec::new();
    let mut lower = 0;
    let mut upper = pixels;
    for _ in 0..20 {
        let mut rgb = clean.clone();
        let span =
            noise_pixels + (pixels - noise_pixels) * noise_spread.unwrap_or(0) as usize / 100;
        for index in 0..noise_pixels {
            let destination = index * span / noise_pixels;
            rgb[destination * 3..destination * 3 + 3]
                .copy_from_slice(&noise[index * 3..index * 3 + 3]);
        }
        output = encode(&rgb, width, height, codec, seed, webp_method)?;
        if output.len() + 8 <= budget && output.len() * 100 >= budget * 85 {
            break;
        }
        if output.len() + 8 > budget {
            upper = noise_pixels;
        } else {
            lower = noise_pixels;
        }
        noise_pixels = lower + (upper - lower) / 2;
    }
    if output.len() + 8 > budget || output.len() * 100 < budget * 85 {
        return Err(Error::Invalid {
            code: "LOGO_PROFILE_ENCODING_BUDGET",
        });
    }
    let encoded = output.len();
    output.extend(b"JUNK");
    output.extend(
        u32::try_from(budget - encoded - 8)
            .context(error::SizeSnafu)?
            .to_le_bytes(),
    );
    output.resize(budget, 0);
    output[4..8].copy_from_slice(
        &u32::try_from(budget - 8)
            .context(error::SizeSnafu)?
            .to_le_bytes(),
    );
    Ok((output, width, height, encoded))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiled_images_preserve_budget_dimensions_and_determinism() -> Result<()> {
        let target = Target {
            alpha: false,
            width: 90,
            height: 90,
            bytes: 3200,
            count: 1,
        };
        let (bytes, w, h, encoded) = generate(&target, 418, 0, None, Encoder::Image, 0, 2)?;
        assert_eq!(bytes.len(), 3200);
        assert!(encoded > 2700);
        let decoded = image::load_from_memory(&bytes).context(error::ImageSnafu)?;
        assert_eq!((decoded.width(), decoded.height()), (w, h));
        assert_eq!(
            bytes,
            generate(&target, 418, 0, None, Encoder::Image, 0, 2)?.0
        );
        let textured = generate(&target, 418, 0, Some(100), Encoder::Image, 0, 2)?.0;
        assert_ne!(bytes, textured);
        assert_eq!(
            textured,
            generate(&target, 418, 0, Some(100), Encoder::Image, 0, 2)?.0
        );
        assert_eq!(textured.len(), bytes.len());
        let decoded = image::load_from_memory(&textured).context(error::ImageSnafu)?;
        assert_eq!((decoded.width(), decoded.height()), (w, h));
        let native = generate(&target, 418, 0, None, Encoder::Libwebp, 55, 2)?.0;
        assert_eq!(native.len(), bytes.len());
        assert_eq!(
            native,
            generate(&target, 418, 0, None, Encoder::Libwebp, 55, 2)?.0
        );
        let raster = image::load_from_memory(&native).context(error::ImageSnafu)?;
        assert_eq!((raster.width(), raster.height()), (w, h));
        let alpha_target = Target {
            alpha: true,
            ..target
        };
        let alpha = generate(&alpha_target, 418, 0, None, Encoder::Libwebp, 55, 6)?.0;
        assert_eq!(alpha.len(), bytes.len());
        let raster = image::load_from_memory(&alpha).context(error::ImageSnafu)?;
        assert!(raster.color().has_alpha());
        assert_eq!((raster.width(), raster.height()), (w, h));
        assert_eq!(
            alpha,
            generate(&alpha_target, 418, 0, None, Encoder::Libwebp, 55, 6)?.0
        );
        Ok(())
    }
}
