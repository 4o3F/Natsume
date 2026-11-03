use super::*;
#[test]
fn preserves_source_modification_time_including_pre_epoch_and_subseconds() {
    for instant in [
        std::time::UNIX_EPOCH,
        std::time::UNIX_EPOCH - std::time::Duration::from_secs(5),
        std::time::UNIX_EPOCH + std::time::Duration::from_millis(1234),
    ] {
        let time = PhotoTime::read(instant);
        assert_eq!(time.instant(), instant);
        assert_eq!(time.source(), PhotoTimeSource::FileModified);
    }
}
