use crate::CoreOptions;
use crate::rpc::client::{CoreInterface, RpcClient};
use crate::util::cli;
use crate::util::common_options::{ProbeOptions, ReadWriteBitWidth, ReadWriteOptions};
use crate::util::read_output::OutputFormat;
use anyhow::Context;
use probe_rs::probe::{
    DebugProbeSelector,
    cmsisdap::{AvrMemoryRegion, read_pkobn_updi_m4809_region},
};
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

impl From<UpdiRegion> for AvrMemoryRegion {
    fn from(region: UpdiRegion) -> Self {
        match region {
            UpdiRegion::Flash => AvrMemoryRegion::Flash,
            UpdiRegion::Eeprom => AvrMemoryRegion::Eeprom,
            UpdiRegion::Fuses => AvrMemoryRegion::Fuses,
            UpdiRegion::Lock => AvrMemoryRegion::Lock,
            UpdiRegion::Userrow => AvrMemoryRegion::UserRow,
            UpdiRegion::Prodsig => AvrMemoryRegion::ProdSig,
        }
    }
}

impl Cmd {
    pub async fn run(self, client: RpcClient) -> anyhow::Result<()> {
        let data = if self.probe_options.protocol
            == Some(crate::util::common_options::CliProtocol::Updi)
        {
            self.read_updi_memory(&client).await?
        } else {
            if self.region.is_some() {
                anyhow::bail!("The option '--region' is only supported with '--protocol updi'.");
            }

            let session = cli::attach_probe(&client, self.probe_options.clone(), false).await?;
            let core = session.core(self.shared.core);

            let data = Self::read_memory(
                core,
                self.read_write_options.address,
                self.read_write_options.width,
                self.words,
            )
            .await?;

            session.resume_all_cores().await?;
            data
        };

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

    async fn read_updi_memory(&self, client: &RpcClient) -> anyhow::Result<Vec<u8>> {
        if !client.is_local_session() {
            anyhow::bail!(
                "The protocol 'UPDI' is currently only supported by 'read' in a local session."
            );
        }

        let probe =
            cli::select_probe(client, self.probe_options.probe.clone().map(Into::into)).await?;
        let selector: DebugProbeSelector = probe.selector().into();
        let region: AvrMemoryRegion = self.region.unwrap_or(UpdiRegion::Flash).into();
        let byte_len = self
            .words
            .checked_mul(self.read_write_options.width.byte_width())
            .context("requested read length overflowed")?;
        let byte_len =
            u32::try_from(byte_len).context("requested read length exceeds 32-bit range")?;
        let offset = u32::try_from(self.read_write_options.address)
            .context("region-relative AVR address exceeds 32-bit range")?;

        read_pkobn_updi_m4809_region(&selector, region, offset, byte_len).map_err(Into::into)
    }

    async fn read_memory(
        core: CoreInterface,
        address: u64,
        width: ReadWriteBitWidth,
        nwords: usize,
    ) -> anyhow::Result<Vec<u8>> {
        let bytes = match width {
            ReadWriteBitWidth::B8 => {
                let values = core.read_memory_8(address, nwords).await?;
                values
                    .into_iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<_>>()
            }
            ReadWriteBitWidth::B16 => {
                let values = core.read_memory_16(address, nwords).await?;
                values
                    .into_iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<_>>()
            }
            ReadWriteBitWidth::B32 => {
                let values = core.read_memory_32(address, nwords).await?;
                values
                    .into_iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<_>>()
            }
            ReadWriteBitWidth::B64 => {
                let values = core.read_memory_64(address, nwords).await?;
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
