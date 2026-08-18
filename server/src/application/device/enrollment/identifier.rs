use uuid::{Uuid, Variant};

pub(in crate::application::device) fn parse_canonical_uuid(
    value: &str,
    version: usize,
) -> Result<Uuid, ()> {
    let parsed = Uuid::parse_str(value).map_err(|_| ())?;
    if parsed.get_version_num() != version
        || parsed.get_variant() != Variant::RFC4122
        || parsed.hyphenated().to_string() != value
    {
        return Err(());
    }
    Ok(parsed)
}
