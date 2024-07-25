use std::io::{self, Read, Write};

pub struct TeeReader<R, W> {
    reader: R,
    writer: W,
}

impl<R: Read, W: Write> TeeReader<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }

    // // Method to return ownership of the writer
    // pub fn into_writer(self) -> W {
    //     self.writer
    // }

    // // Method to drain the remaining data to a sink
    // pub fn drain_to_sink(&mut self) -> io::Result<()> {
    //     std::io::copy(self, &mut std::io::sink())?;
    //     Ok(())
    // }
}

impl<R: Read, W: io::Write> Read for TeeReader<R, W> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let bytes_read = self.reader.read(buf)?;
        self.writer.write_all(&buf[..bytes_read])?;
        Ok(bytes_read)
    }
}
