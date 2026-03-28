use anyhow::Context;
use ihex::Record;
use std::io::Write;
use std::path::Path;

use super::common_options::ReadWriteBitWidth;

#[derive(clap::ValueEnum, Clone, Copy)]
pub(crate) enum OutputFormat {
    /// Intel Hex Format
    Ihex,
    /// Simple list of hexadecimal numbers
    SimpleHex,
    /// Hexadecimal numbers formatted into a table
    HexTable,
    /// The raw binary
    Binary,
}

impl OutputFormat {
    pub(crate) fn write(
        self,
        dst: impl Write,
        address: u64,
        width: ReadWriteBitWidth,
        data: &[u8],
    ) -> anyhow::Result<()> {
        match self {
            OutputFormat::Binary => Self::write_binary(dst, data),
            OutputFormat::Ihex => Self::write_ihex(dst, address, data),
            OutputFormat::SimpleHex => Self::write_simple_hex(dst, width, data),
            OutputFormat::HexTable => Self::write_hex_table(dst, address, width, data),
        }
    }

    pub(crate) fn save_to_file(
        self,
        address: u64,
        width: ReadWriteBitWidth,
        data: &[u8],
        path: &Path,
    ) -> anyhow::Result<()> {
        // Write to an in-memory buffer first so we don't truncate the output
        // file if format validation or serialization fails.
        let mut buf = Vec::new();
        self.write(&mut buf, address, width, data)?;
        std::fs::write(path, &buf)?;
        Ok(())
    }

    pub(crate) fn print_to_console(
        self,
        address: u64,
        width: ReadWriteBitWidth,
        data: &[u8],
    ) -> anyhow::Result<()> {
        let mut stdout = std::io::stdout();
        self.write(&mut stdout, address, width, data)
    }

    fn write_simple_hex(
        mut dst: impl Write,
        width: ReadWriteBitWidth,
        data: &[u8],
    ) -> anyhow::Result<()> {
        let bytes = width.byte_width();

        let mut first = true;
        for window in data.chunks(bytes) {
            if first {
                first = false;
            } else {
                write!(dst, " ")?;
            }

            for byte in window.iter().rev() {
                write!(dst, "{byte:02x}")?;
            }
        }

        writeln!(dst)?;
        Ok(())
    }

    fn write_hex_table(
        mut dst: impl Write,
        mut address: u64,
        width: ReadWriteBitWidth,
        data: &[u8],
    ) -> anyhow::Result<()> {
        let bytes_in_line = match width {
            ReadWriteBitWidth::B8 => 8,
            ReadWriteBitWidth::B16 => 16,
            ReadWriteBitWidth::B32 | ReadWriteBitWidth::B64 => 32,
        };
        for window in data.chunks(bytes_in_line) {
            write!(dst, "{address:08x}: ")?;
            Self::write_simple_hex(&mut dst, width, window)?;
            address += bytes_in_line as u64;
        }

        Ok(())
    }

    fn write_binary(mut dst: impl Write, data: &[u8]) -> anyhow::Result<()> {
        dst.write_all(data)?;
        Ok(())
    }

    fn write_ihex(mut dst: impl Write, address: u64, data: &[u8]) -> anyhow::Result<()> {
        let mut running_address = address;
        let mut records = vec![];
        let mut last_address_msbs: Option<u16> = None;

        let mut remaining = data;
        while !remaining.is_empty() {
            let address_msbs: u16 = (running_address >> 16)
                .try_into()
                .context("Hex format only supports addressing up to 32 bits")?;

            // Emit an extended linear address record when crossing a 64 KiB boundary.
            if last_address_msbs != Some(address_msbs) {
                records.push(Record::ExtendedLinearAddress(address_msbs));
                last_address_msbs = Some(address_msbs);
            }

            // Limit the chunk so it doesn't cross a 64 KiB segment boundary.
            let bytes_until_boundary = 0x10000u64.saturating_sub(running_address & 0xFFFF);
            let chunk_len = remaining
                .len()
                .min(255)
                .min(bytes_until_boundary as usize)
                .max(1);

            records.push(Record::Data {
                offset: (running_address & 0xFFFF) as u16,
                value: remaining[..chunk_len].to_vec(),
            });
            remaining = &remaining[chunk_len..];
            running_address += chunk_len as u64;
        }

        records.push(Record::EndOfFile);
        let hexdata = ihex::create_object_file_representation(&records)?;
        dst.write_all(hexdata.as_bytes())?;
        Ok(())
    }
}
