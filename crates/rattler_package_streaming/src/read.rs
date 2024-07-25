//! Functions that enable extracting or streaming a Conda package for objects that implement the
//! [`std::io::Read`] trait.

use super::{ExtractError, ExtractResult};
use crate::tee_reader::TeeReader;
use std::io::Cursor;
use std::mem::ManuallyDrop;
use std::{ffi::OsStr, io::Read, path::Path};
use zip::read::{read_zipfile_from_stream, ZipArchive, ZipFile};
use zip::result::ZipError;

/// Returns the `.tar.bz2` as a decompressed `tar::Archive`. The `tar::Archive` can be used to
/// extract the files from it, or perform introspection.
pub fn stream_tar_bz2(reader: impl Read) -> tar::Archive<impl Read + Sized> {
    tar::Archive::new(bzip2::read::BzDecoder::new(reader))
}

/// Returns the `.tar.zst` as a decompressed `tar` archive. The `tar::Archive` can be used to
/// extract the files from it, or perform introspection.
pub(crate) fn stream_tar_zst(
    reader: impl Read,
) -> Result<tar::Archive<impl Read + Sized>, ExtractError> {
    Ok(tar::Archive::new(zstd::stream::read::Decoder::new(reader)?))
}

/// Extracts the contents a `.tar.bz2` package archive.
pub fn extract_tar_bz2(
    reader: impl Read,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;

    // Wrap the reading in aditional readers that will compute the hashes of the file while its
    // being read.
    let sha256_reader = rattler_digest::HashingReader::<_, rattler_digest::Sha256>::new(reader);
    let mut md5_reader =
        rattler_digest::HashingReader::<_, rattler_digest::Md5>::new(sha256_reader);

    // Unpack the archive
    stream_tar_bz2(&mut md5_reader).unpack(destination)?;

    // Get the hashes
    let (sha256_reader, md5) = md5_reader.finalize();
    let (_, sha256) = sha256_reader.finalize();

    Ok(ExtractResult { sha256, md5 })
}

/// Extracts the contents of a `.conda` package archive.
pub fn extract_conda(reader: impl Read, destination: &Path) -> Result<ExtractResult, ExtractError> {
    // Create a buffer to store the read bytes
    let fallback_buffer: Vec<u8> = Vec::new();
    let mut tee_reader = TeeReader::new(reader, fallback_buffer);

    // Construct the destination path if it doesnt exist yet
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;

    // Wrap the reading in aditional readers that will compute the hashes of the file while its
    // being read.
    let sha256_reader =
        rattler_digest::HashingReader::<_, rattler_digest::Sha256>::new(&mut tee_reader);
    let mut md5_reader =
        rattler_digest::HashingReader::<_, rattler_digest::Md5>::new(sha256_reader);

    // Iterate over all entries in the zip-file and extract them one-by-one
    let result = loop {
        match read_zipfile_from_stream(&mut md5_reader) {
            Ok(Some(file)) => {
                extract_zipfile(file, destination)?;
            }
            Ok(None) => break Ok(()), // No more files to read
            Err(e) => break Err(e),
        };
    };

    return match result {
        Ok(()) => {
            // Read the file to the end to make sure the hash is properly computed.
            std::io::copy(&mut md5_reader, &mut std::io::sink()).map_err(ExtractError::IoError)?;

            // Get the hashes
            let (sha256_reader, md5) = md5_reader.finalize();
            let (_, sha256) = sha256_reader.finalize();

            Ok(ExtractResult { sha256, md5 })
        }
        Err(ZipError::InvalidArchive(msg))
            if msg == "The file length is not available in the local header" =>
        {
            // Read the file to the end to ensure buffer is all in memory
            eprintln!(
                "Invalid archive: {}, falling back to downloading to disk",
                msg
            );
            std::io::copy(&mut tee_reader, &mut std::io::sink()).map_err(ExtractError::IoError)?;
            return extract_conda_from_local_buffer(&mut tee_reader, destination);
        }
        Err(e) => {
            return Err(ExtractError::ZipError(e));
        }
    };
}

fn extract_zipfile(zip_file: ZipFile<'_>, destination: &Path) -> Result<(), ExtractError> {
    // If an error occurs while we are reading the contents of the zip we don't want to
    // seek to the end of the file. Using [`ManuallyDrop`] we prevent `drop` to be called on
    // the `file` in case the stack unwinds.
    let mut file = ManuallyDrop::new(zip_file);

    if file
        .mangled_name()
        .file_name()
        .map(OsStr::to_string_lossy)
        .map_or(false, |file_name| file_name.ends_with(".tar.zst"))
    {
        stream_tar_zst(&mut *file)?.unpack(destination)?;
    } else {
        // Manually read to the end of the stream if that didn't happen.
        std::io::copy(&mut *file, &mut std::io::sink())?;
    }

    // Take the file out of the [`ManuallyDrop`] to properly drop it.
    let _ = ManuallyDrop::into_inner(file);

    Ok(())
}

/// Extracts the contents of a `.conda` package archive.
pub fn extract_conda_from_local_buffer(
    reader: impl Read,
    destination: &Path,
) -> Result<ExtractResult, ExtractError> {
    // delete destination first

    // Construct the destination path if it doesnt exist yet
    std::fs::create_dir_all(destination).map_err(ExtractError::CouldNotCreateDestination)?;

    let mut buffer = Vec::new();
    let sha256_reader = rattler_digest::HashingReader::<_, rattler_digest::Sha256>::new(reader);
    let mut md5_reader =
        rattler_digest::HashingReader::<_, rattler_digest::Md5>::new(sha256_reader);
    md5_reader.read_to_end(&mut buffer)?;

    // Create a Cursor from the buffer
    let cursor = Cursor::new(buffer);

    // Create a ZipArchive from the cursor
    let mut archive = ZipArchive::new(cursor)?;

    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        extract_zipfile(file, destination)?;
    }
    // Read the file to the end to make sure the hash is properly computed.
    std::io::copy(&mut md5_reader, &mut std::io::sink())?;

    // Get the hashes
    let (sha256_reader, md5) = md5_reader.finalize();
    let (_, sha256) = sha256_reader.finalize();

    Ok(ExtractResult { sha256, md5 })
}
