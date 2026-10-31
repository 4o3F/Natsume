use super::*;
use ed25519_dalek::SigningKey;
use natsume_photo_protocol::MANIFEST_LIMIT;

#[derive(Serialize)]
struct Content<'a> {
    format_version: FormatVersion,
    kind: ManifestKind,
    context: &'a ResultContext,
    signing_public_key: &'a natsume_photo_protocol::SigningPublicKey,
    results: &'a [natsume_photo_protocol::results::TeamResult],
}
fn fingerprint(manifest: &ResultsManifest) -> Result<Sha256Digest> {
    manifest.validate().context(ContractSnafu)?;
    let content = Content {
        format_version: manifest.format_version,
        kind: manifest.kind,
        context: &manifest.context,
        signing_public_key: &manifest.signing_public_key,
        results: &manifest.results,
    };
    let bytes = serde_json_canonicalizer::to_vec(&content).context(JsonSnafu)?;
    ensure!(bytes.len() <= MANIFEST_LIMIT, LimitSnafu);
    Ok(Sha256Digest::of(&bytes))
}
pub(super) fn prepare(
    root: &Path,
    manifest: &ResultsManifest,
    key: &SigningKey,
) -> Result<(SignedResults, Sha256Digest)> {
    let fingerprint = fingerprint(manifest)?;
    let directory = root.join("exports");
    fs::create_dir_all(&directory).context(IoSnafu)?;
    ensure!(
        fs::canonicalize(&directory).context(IoSnafu)? == directory,
        PathSnafu
    );
    let path = directory.join(format!("{fingerprint}.json"));
    if path.try_exists().context(IoSnafu)? {
        let bytes = read(&path)?;
        let saved = SignedResults::parse_and_verify(
            &bytes,
            &manifest.context,
            &manifest.signing_public_key,
        )
        .context(ContractSnafu)?;
        ensure!(
            self::fingerprint(&saved.manifest)? == fingerprint
                && saved.manifest.results == manifest.results,
            IntegritySnafu
        );
        return Ok((saved, fingerprint));
    }
    let signed = manifest.sign(key).context(ContractSnafu)?;
    publish(&path, &signed.to_json().context(ContractSnafu)?)?;
    Ok((signed, fingerprint))
}
pub(super) fn complete(
    root: &Path,
    fingerprint: &Sha256Digest,
    result: &ExportedPackage,
) -> Result<()> {
    let path = root
        .join("exports")
        .join(format!("{fingerprint}.complete.json"));
    if path.try_exists().context(IoSnafu)? {
        let previous: ExportedPackage = serde_json::from_slice(&read(&path)?).context(JsonSnafu)?;
        ensure!(&previous == result, IntegritySnafu);
        return Ok(());
    }
    publish(&path, &serde_json::to_vec(result).context(JsonSnafu)?)
}
fn read(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_regular(path)?
        .take(MANIFEST_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .context(IoSnafu)?;
    ensure!(bytes.len() <= MANIFEST_LIMIT, LimitSnafu);
    Ok(bytes)
}
fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
    let directory = path.parent().ok_or_else(|| PathSnafu.build())?;
    ensure!(
        fs::canonicalize(directory).context(IoSnafu)? == directory,
        PathSnafu
    );
    let mut temporary = tempfile::Builder::new()
        .prefix(".export-record-")
        .tempfile_in(directory)
        .context(IoSnafu)?;
    temporary.write_all(bytes).context(IoSnafu)?;
    temporary.as_file().sync_all().context(IoSnafu)?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| IoSnafu.into_error(error.error))?;
    Ok(())
}
