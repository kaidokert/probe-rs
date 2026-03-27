use crate::CoreOptions;
use crate::rpc::{
    client::{CoreInterface, RpcClient},
    functions::memory::AvrMemoryRegion as RpcAvrMemoryRegion,
};
use crate::util::cli;
use crate::util::common_options::{ProbeOptions, ReadWriteBitWidth, ReadWriteOptions};
use crate::util::read_output::OutputFormat;
use std::path::PathBuf;

/// Read from target memory address
///
/// e.g. probe-rs read b32 0x400E1490 2
///      Reads 2 32-bit words from address 0x400E1490
///
/// e.g. probe-rs read --protocol updi --region eeprom b8 0x00 16
///      Reads 16 bytes from EEPROM offset 0x00 on the narrow AVR UPDI path
///
/// Default output is a space separated list of hex values padded to the read word width.
/// e.g. 2 words
///     00 00 (8-bit)
///     00000000 00000000 (32-bit)
///     0000000000000000 0000000000000000 (64-bit)
///
/// If the --output argument is provided, readback data is instead saved to a file as hex/bin.
/// In this case, the read word width has no effect except determining the total number of bytes
#[derive(clap::Parser)]
#[clap(verbatim_doc_comment)]
pub struct Cmd {
    #[clap(flatten)]
    shared: CoreOptions,

    #[clap(flatten)]
    probe_options: ProbeOptions,

    #[clap(flatten)]
    read_write_options: ReadWriteOptions,

    /// AVR UPDI memory region to read from.
    ///
    /// This is only used with `--protocol updi`. When omitted there, flash is used.
    #[arg(long, value_enum)]
    region: Option<UpdiRegion>,

    /// Number of words to read from the target
    words: usize,

    /// File to output binary data to
    #[arg(long, short)]
    output: Option<PathBuf>,

    /// Format of the outputted binary data
    #[clap(value_enum, default_value_t=OutputFormat::HexTable)]
    #[arg(long, short)]
    format: OutputFormat,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum UpdiRegion {
    /// Flash memory, addressed relative to the flash start.
    Flash,
    /// EEPROM memory, addressed relative to the EEPROM start.
    Eeprom,
    /// Fuse bytes, addressed relative to the fuse region start.
    Fuses,
    /// Lock byte, addressed relative to the lock region start.
    Lock,
    /// USERROW bytes, addressed relative to the user row start.
    Userrow,
    /// Production signature bytes, addressed relative to the production signature start.
    Prodsig,
}

impl From<UpdiRegion> for RpcAvrMemoryRegion {
    fn from(region: UpdiRegion) -> Self {
        match region {
            UpdiRegion::Flash => RpcAvrMemoryRegion::Flash,
            UpdiRegion::Eeprom => RpcAvrMemoryRegion::Eeprom,
            UpdiRegion::Fuses => RpcAvrMemoryRegion::Fuses,
            UpdiRegion::Lock => RpcAvrMemoryRegion::Lock,
            UpdiRegion::Userrow => RpcAvrMemoryRegion::UserRow,
            UpdiRegion::Prodsig => RpcAvrMemoryRegion::ProdSig,
        }
    }
}

impl Cmd {
    pub async fn run(self, client: RpcClient) -> anyhow::Result<()> {
        if self.region.is_some()
            && self.probe_options.protocol != Some(crate::util::common_options::CliProtocol::Updi)
        {
            anyhow::bail!("The option '--region' is only supported with '--protocol updi'.");
        }

        let session = cli::attach_probe(&client, self.probe_options.clone(), false).await?;
        let core = session.core(self.shared.core);

        let region = if self.probe_options.protocol
            == Some(crate::util::common_options::CliProtocol::Updi)
        {
            Some(self.region.unwrap_or(UpdiRegion::Flash).into())
        } else {
            None
        };

        let data = Self::read_memory(
            core,
            self.read_write_options.address,
            self.read_write_options.width,
            self.words,
            region,
        )
        .await?;

        if self.probe_options.protocol != Some(crate::util::common_options::CliProtocol::Updi) {
            session.resume_all_cores().await?;
        }

        match self.output {
            Some(path) => Self::save_to_file(
                self.read_write_options.address,
                &data,
                path,
                self.read_write_options.width,
                self.format,
            )?,
            None => Self::print_to_console(
                self.read_write_options.address,
                &data,
                self.read_write_options.width,
                self.format,
            )?,
        };

        Ok(())
    }

    async fn read_memory(
        core: CoreInterface,
        address: u64,
        width: ReadWriteBitWidth,
        nwords: usize,
        region: Option<RpcAvrMemoryRegion>,
    ) -> anyhow::Result<Vec<u8>> {
        let bytes = match width {
            ReadWriteBitWidth::B8 => {
                let values = core.read_memory_8(address, nwords, region).await?;
                values
                    .into_iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<_>>()
            }
            ReadWriteBitWidth::B16 => {
                let values = core.read_memory_16(address, nwords, region).await?;
                values
                    .into_iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<_>>()
            }
            ReadWriteBitWidth::B32 => {
                let values = core.read_memory_32(address, nwords, region).await?;
                values
                    .into_iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<_>>()
            }
            ReadWriteBitWidth::B64 => {
                let values = core.read_memory_64(address, nwords, region).await?;
                values
                    .into_iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<_>>()
            }
        };
        Ok(bytes)
    }

    fn save_to_file(
        address: u64,
        data: &[u8],
        path: PathBuf,
        width: ReadWriteBitWidth,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        format.save_to_file(address, width, data, &path)
    }

    fn print_to_console(
        address: u64,
        data: &[u8],
        width: ReadWriteBitWidth,
        format: OutputFormat,
    ) -> anyhow::Result<()> {
        format.print_to_console(address, width, data)
    }
}
