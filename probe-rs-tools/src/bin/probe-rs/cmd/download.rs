use std::path::PathBuf;

use anyhow::Context;
use object::{Object, ObjectSection, SectionKind};

use crate::rpc::client::RpcClient;
use crate::rpc::functions::memory::AvrMemoryRegion as RpcAvrMemoryRegion;

use crate::util::cli;
use crate::util::common_options::{BinaryDownloadOptions, ProbeOptions};
use crate::{FormatKind, FormatOptions};
use probe_rs::probe::WireProtocol;

#[derive(clap::Parser)]
pub struct Cmd {
    #[clap(flatten)]
    pub probe_options: ProbeOptions,

    /// The path to the file to be downloaded to the flash
    pub path: PathBuf,

    #[clap(flatten)]
    pub download_options: BinaryDownloadOptions,

    #[clap(flatten)]
    pub format_options: FormatOptions,
}

impl Cmd {
    pub async fn run(self, client: RpcClient) -> anyhow::Result<()> {
        if self.probe_options.protocol == Some(WireProtocol::Updi) {
            self.run_updi_download(&client).await?;
        } else {
            let session = cli::attach_probe(&client, self.probe_options, false).await?;

            cli::flash(
                &session,
                &self.path,
                self.format_options,
                self.download_options,
                None,
                None,
            )
            .await?;
        }

        Ok(())
    }

    async fn run_updi_download(self, client: &RpcClient) -> anyhow::Result<()> {
        ensure_updi_download_options_supported(&self.download_options)?;

        if !client.is_local_session() {
            anyhow::bail!(
                "The protocol 'UPDI' is currently only supported by 'download' in a local session."
            );
        }

        let session = cli::attach_probe(client, self.probe_options, false).await?;
        let core = session.core(0);
        let blocks = load_updi_flash_blocks(&self.path, &self.format_options)?;

        if blocks.is_empty() {
            anyhow::bail!("No flashable data found in '{}'.", self.path.display());
        }

        let total_bytes: usize = blocks.iter().map(|block| block.data.len()).sum();
        println!(
            "Programming {} block(s), {} bytes via UPDI",
            blocks.len(),
            total_bytes
        );

        for block in &blocks {
            println!(
                "  flash[0x{address:04x}..0x{end:04x}) <- {} bytes",
                block.data.len(),
                address = block.address,
                end = block.address + u32::try_from(block.data.len()).unwrap_or(u32::MAX),
            );
            core.write_memory_8(
                u64::from(block.address),
                block.data.clone(),
                Some(RpcAvrMemoryRegion::Flash),
            )
            .await?;
        }

        if self.download_options.verify {
            println!("Verifying flash...");
            for block in &blocks {
                let readback = core
                    .read_memory_8(
                        u64::from(block.address),
                        u32::try_from(block.data.len())
                            .context("flash block length exceeds 32-bit range")?
                            as usize,
                        Some(RpcAvrMemoryRegion::Flash),
                    )
                    .await?;
                if readback != block.data {
                    anyhow::bail!(
                        "Verification failed for block at flash offset 0x{:04x}.",
                        block.address
                    );
                }
            }
            println!("Verification successful");
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FlashBlock {
    pub(crate) address: u32,
    pub(crate) data: Vec<u8>,
}

pub(crate) fn ensure_updi_download_options_supported(
    options: &BinaryDownloadOptions,
) -> anyhow::Result<()> {
    if options.restore_unwritten {
        anyhow::bail!("'download --protocol updi' does not support '--restore-unwritten' yet.");
    }
    if options.preverify {
        anyhow::bail!("'download --protocol updi' does not support '--preverify' yet.");
    }
    if options.chip_erase {
        anyhow::bail!("'download --protocol updi' does not support '--chip-erase' yet.");
    }
    if options.disable_double_buffering {
        anyhow::bail!(
            "'download --protocol updi' does not support '--disable-double-buffering' yet."
        );
    }
    if options.flash_layout_output_path.is_some() {
        anyhow::bail!("'download --protocol updi' does not support '--flash-layout' yet.");
    }
    if options.read_flasher_rtt {
        anyhow::bail!("'download --protocol updi' does not support '--read-flasher-rtt' yet.");
    }
    if !options.prefer_flash_algorithm.is_empty() {
        anyhow::bail!(
            "'download --protocol updi' does not support '--prefer-flash-algorithm' yet."
        );
    }

    Ok(())
}

pub(crate) fn load_updi_flash_blocks(
    path: &PathBuf,
    format: &FormatOptions,
) -> anyhow::Result<Vec<FlashBlock>> {
    let binary_format = format.binary_format_for_path(path)?;

    match binary_format {
        FormatKind::Bin => load_updi_bin_blocks(path, format),
        FormatKind::Hex => load_updi_hex_blocks(path),
        FormatKind::Elf => load_updi_elf_blocks(path, format),
        FormatKind::Target | FormatKind::Idf | FormatKind::Uf2 => {
            anyhow::bail!(
                "'download --protocol updi' currently supports only bin, hex, and elf images."
            )
        }
    }
}

fn load_updi_bin_blocks(path: &PathBuf, format: &FormatOptions) -> anyhow::Result<Vec<FlashBlock>> {
    let mut data = std::fs::read(path)
        .with_context(|| format!("Failed to read binary image '{}'.", path.display()))?;
    let skip = usize::try_from(format.bin_skip()).context("binary skip exceeds usize range")?;
    if skip > data.len() {
        anyhow::bail!(
            "The requested binary skip ({skip}) exceeds file size ({} bytes).",
            data.len()
        );
    }
    data.drain(..skip);

    let base_address = format.bin_base_address().unwrap_or(0);
    let address =
        u32::try_from(base_address).context("binary base address exceeds 32-bit range")?;

    Ok(merge_flash_blocks(vec![FlashBlock { address, data }]))
}

fn load_updi_hex_blocks(path: &PathBuf) -> anyhow::Result<Vec<FlashBlock>> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read Intel HEX image '{}'.", path.display()))?;

    let mut blocks = Vec::new();
    let mut base_address = 0u32;

    for record in ihex::Reader::new(&contents) {
        match record.context("Failed to parse Intel HEX record")? {
            ihex::Record::Data { offset, value } => {
                let address = base_address
                    .checked_add(u32::from(offset))
                    .context("Intel HEX address overflowed 32-bit range")?;
                blocks.push(FlashBlock {
                    address,
                    data: value,
                });
            }
            ihex::Record::ExtendedSegmentAddress(segment) => {
                base_address = u32::from(segment) << 4;
            }
            ihex::Record::ExtendedLinearAddress(linear) => {
                base_address = u32::from(linear) << 16;
            }
            ihex::Record::EndOfFile => break,
            ihex::Record::StartSegmentAddress { .. } | ihex::Record::StartLinearAddress(_) => {}
        }
    }

    Ok(merge_flash_blocks(blocks))
}

fn load_updi_elf_blocks(path: &PathBuf, format: &FormatOptions) -> anyhow::Result<Vec<FlashBlock>> {
    let contents = std::fs::read(path)
        .with_context(|| format!("Failed to read ELF image '{}'.", path.display()))?;
    let object = object::File::parse(contents.as_slice()).context("Failed to parse ELF file")?;

    let mut blocks = Vec::new();
    for section in object.sections() {
        let section_name = section
            .name()
            .context("Failed to read ELF section name")?
            .to_string();
        if format
            .elf_skip_sections()
            .iter()
            .any(|skip| skip == &section_name)
        {
            continue;
        }

        if matches!(
            section.kind(),
            SectionKind::Metadata
                | SectionKind::Other
                | SectionKind::OtherString
                | SectionKind::Unknown
                | SectionKind::UninitializedData
                | SectionKind::UninitializedTls
                | SectionKind::Note
        ) {
            continue;
        }

        let data = section.data().with_context(|| {
            format!("Failed to read ELF section data for section '{section_name}'.")
        })?;
        if data.is_empty() {
            continue;
        }

        let address = u32::try_from(section.address()).with_context(|| {
            format!("ELF section '{section_name}' address exceeds 32-bit range.")
        })?;

        blocks.push(FlashBlock {
            address,
            data: data.to_vec(),
        });
    }

    Ok(merge_flash_blocks(blocks))
}

fn merge_flash_blocks(mut blocks: Vec<FlashBlock>) -> Vec<FlashBlock> {
    blocks.retain(|block| !block.data.is_empty());
    blocks.sort_by_key(|block| block.address);

    let mut merged: Vec<FlashBlock> = Vec::new();
    for block in blocks {
        if let Some(previous) = merged.last_mut() {
            let previous_end = previous
                .address
                .saturating_add(u32::try_from(previous.data.len()).unwrap_or(u32::MAX));
            if previous_end == block.address {
                previous.data.extend_from_slice(&block.data);
                continue;
            }
        }
        merged.push(block);
    }

    merged
}
