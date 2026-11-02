use super::*;
use snafu::Snafu;

#[derive(Debug, Snafu)]
enum TestError {
    Io { source: io::Error },
}
type TestResult<T = ()> = std::result::Result<T, TestError>;

#[test]
fn encoded_byte_limit_rejects_before_writing_an_excess_chunk() -> TestResult {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let path = directory.path().join("bounded-output");
    let mut writer = CandidateWriter {
        file: BufWriter::new(File::create_new(&path).context(IoSnafu)?),
        hash: Sha256::new(),
        // Fixture the final stream position without writing a 50 MiB test file.
        bytes: OUTPUT_LIMIT,
    };
    assert_eq!(
        writer.write_all(b"excess").err().map(|error| error.kind()),
        Some(io::ErrorKind::FileTooLarge)
    );
    writer.flush().context(IoSnafu)?;
    assert_eq!(fs::metadata(path).context(IoSnafu)?.len(), 0);
    assert_eq!(writer.bytes, OUTPUT_LIMIT);
    Ok(())
}
