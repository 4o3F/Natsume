use super::*;
#[test]
fn configuration_normalization_and_persisted_canonical_spelling_have_distinct_boundaries() {
    let Err(error) = ServerOrigin::parse("://invalid") else {
        panic!("invalid URL must fail before origin validation");
    };
    assert!(matches!(&error, OriginError::Url { .. }));
    assert!(std::error::Error::source(&error).is_some());
    let origin = ServerOrigin::parse("HTTPS://Example.COM:443/")
        .unwrap_or_else(|error| panic!("valid HTTPS input: {error:?}"));
    assert_eq!(origin.as_str(), "https://example.com");
    let encoded = serde_json::to_string(&origin)
        .unwrap_or_else(|error| panic!("serialize origin: {error:?}"));
    assert_eq!(
        serde_json::from_str::<ServerOrigin>(&encoded)
            .unwrap_or_else(|error| panic!("canonical origin: {error:?}")),
        origin
    );
    for value in [
        "http://example.com",
        "https://user:pass@example.com",
        "https://example.com/api",
        "https://example.com/?secret=x",
        "https://example.com/#fragment",
    ] {
        assert!(ServerOrigin::parse(value).is_err(), "reject non-origin URL");
    }
    assert!(serde_json::from_str::<ServerOrigin>(r#""https://example.com/""#).is_err());
}
