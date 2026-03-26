use std::path::PathBuf;

use anyhow::Context;
use probe_rs::probe::{
    DebugProbeSelector,
    cmsisdap::{AvrMemoryRegion, read_pkobn_updi_m4809_region},
    list::Lister,
};

use crate::util::{common_options::ReadWriteOptions, read_output::OutputFormat};

use super::edbg_avr_info::select_probe_for_edbg;

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum Region {
    /// EEPROM memory, addressed relative to the EEPROM start.
    Eeprom,
}

impl From<Region> for AvrMemoryRegion {
    fn from(region: Region) -> Self {
        match region {
            Region::Eeprom => AvrMemoryRegion::Eeprom,
        }
    }
}

/// Experimental AVR UPDI region read.
///
/// The address argument is relative to the selected AVR region.
///
/// e.g. `probe-rs edbg-avr-read --region eeprom b8 0x00 16`
///      Reads 16 bytes from EEPROM starting at EEPROM offset 0x00.
#[derive(clap::Parser)]
#[clap(verbatim_doc_comment)]
pub struct Cmd {
    /// Disable interactive probe selection
    #[arg(
        long,
        env = "PROBE_RS_NON_INTERACTIVE",
        help_heading = "PROBE CONFIGURATION"
    )]
    non_interactive: bool,
    /// Use this flag to select a specific probe in the list.
    #[arg(long, env = "PROBE_RS_PROBE", help_heading = "PROBE CONFIGURATION")]
    probe: Option<DebugProbeSelector>,

    /// AVR memory region to read from
    #[arg(long, value_enum)]
    region: Region,

    #[clap(flatten)]
    read_write_options: ReadWriteOptions,

    /// Number of words to read from the selected region
    words: usize,

    /// File to output binary data to
    #[arg(long, short)]
    output: Option<PathBuf>,

    /// Format of the outputted binary data
    #[clap(value_enum, default_value_t=OutputFormat::HexTable)]
    #[arg(long, short)]
    format: OutputFormat,
}

impl Cmd {
    pub fn run(self, lister: &Lister) -> anyhow::Result<()> {
        let probe = select_probe_for_edbg(lister, self.probe.as_ref(), self.non_interactive)?;
        let selector = DebugProbeSelector::from(&probe);
        let region: AvrMemoryRegion = self.region.into();
        let byte_len = self
            .words
            .checked_mul(self.read_write_options.width.byte_width())
            .context("requested read length overflowed")?;
        let byte_len =
            u32::try_from(byte_len).context("requested read length exceeds 32-bit range")?;
        let offset = u32::try_from(self.read_write_options.address)
            .context("region-relative AVR address exceeds 32-bit range")?;

        let data = read_pkobn_updi_m4809_region(&selector, region, offset, byte_len)?;

        match self.output {
            Some(path) => self.format.save_to_file(
                self.read_write_options.address,
                self.read_write_options.width,
                &data,
                &path,
            )?,
            None => self.format.print_to_console(
                self.read_write_options.address,
                self.read_write_options.width,
                &data,
            )?,
        }

        Ok(())
    }
}
