use crate::rpc::client::RpcClient;

use crate::CoreOptions;
use crate::util::common_options::{CliProtocol, ProbeOptions, ReadWriteBitWidth, ReadWriteOptions};
use crate::util::{cli, parse_u64};
use anyhow::Context;
use probe_rs::probe::{DebugProbeSelector, cmsisdap::write_pkobn_updi_m4809_flash};

/// Write to target memory address
///
/// e.g. probe-rs write b32 0x400E1490 0xDEADBEEF 0xCAFEF00D
///      Writes 0xDEADBEEF to address 0x400E1490 and 0xCAFEF00D to address 0x400E1494
///
/// e.g. probe-rs write --protocol updi b8 0x12 0x40 0xE1
///      Writes two bytes to flash offset 0x12 on the narrow AVR UPDI path
///
/// NOTE: The generic path supports RAM addresses. The local UPDI path currently only supports flash.
#[derive(clap::Parser)]
#[clap(verbatim_doc_comment)]
pub struct Cmd {
    #[clap(flatten)]
    shared: CoreOptions,

    #[clap(flatten)]
    probe_options: ProbeOptions,

    #[clap(flatten)]
    read_write_options: ReadWriteOptions,

    /// Values to write to the target.
    /// Takes a list of integer values and can be specified in decimal (16), hexadecimal (0x10) or octal (0o20) format.
    #[clap(value_parser = parse_u64)]
    values: Vec<u64>,
}

fn ensure_data_in_range(data: &[u64], width: ReadWriteBitWidth) -> anyhow::Result<()> {
    let max = match width {
        ReadWriteBitWidth::B8 => u8::MAX as u64,
        ReadWriteBitWidth::B16 => u16::MAX as u64,
        ReadWriteBitWidth::B32 => u32::MAX as u64,
        ReadWriteBitWidth::B64 => u64::MAX,
    };
    if let Some(big) = data.iter().find(|&&v| v > max) {
        anyhow::bail!(
            "{} in {:?} is too large for an {} bit write.",
            big,
            data,
            width as u8,
        );
    }

    Ok(())
}

impl Cmd {
    pub async fn run(self, client: RpcClient) -> anyhow::Result<()> {
        ensure_data_in_range(&self.values, self.read_write_options.width)?;

        if self.probe_options.protocol == Some(CliProtocol::Updi) {
            self.write_updi_flash(&client).await?;
        } else {
            let session = cli::attach_probe(&client, self.probe_options, false).await?;
            let core = session.core(self.shared.core);

            match self.read_write_options.width {
                ReadWriteBitWidth::B8 => {
                    core.write_memory_8(
                        self.read_write_options.address,
                        self.values.iter().map(|v| *v as u8).collect(),
                    )
                    .await?;
                }
                ReadWriteBitWidth::B16 => {
                    core.write_memory_16(
                        self.read_write_options.address,
                        self.values.iter().map(|v| *v as u16).collect(),
                    )
                    .await?;
                }
                ReadWriteBitWidth::B32 => {
                    core.write_memory_32(
                        self.read_write_options.address,
                        self.values.iter().map(|v| *v as u32).collect(),
                    )
                    .await?;
                }
                ReadWriteBitWidth::B64 => {
                    core.write_memory_64(self.read_write_options.address, self.values)
                        .await?;
                }
            }
        }

        Ok(())
    }

    async fn write_updi_flash(&self, client: &RpcClient) -> anyhow::Result<()> {
        if !client.is_local_session() {
            anyhow::bail!(
                "The protocol 'UPDI' is currently only supported by 'write' in a local session."
            );
        }

        let probe =
            cli::select_probe(client, self.probe_options.probe.clone().map(Into::into)).await?;
        let selector: DebugProbeSelector = probe.selector().into();
        let offset = u32::try_from(self.read_write_options.address)
            .context("flash-relative AVR address exceeds 32-bit range")?;
        let data = values_to_le_bytes(self.read_write_options.width, &self.values);

        write_pkobn_updi_m4809_flash(&selector, offset, &data).map_err(Into::into)
    }
}

fn values_to_le_bytes(width: ReadWriteBitWidth, values: &[u64]) -> Vec<u8> {
    match width {
        ReadWriteBitWidth::B8 => values.iter().map(|value| *value as u8).collect(),
        ReadWriteBitWidth::B16 => values
            .iter()
            .flat_map(|value| (*value as u16).to_le_bytes())
            .collect(),
        ReadWriteBitWidth::B32 => values
            .iter()
            .flat_map(|value| (*value as u32).to_le_bytes())
            .collect(),
        ReadWriteBitWidth::B64 => values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect(),
    }
}
