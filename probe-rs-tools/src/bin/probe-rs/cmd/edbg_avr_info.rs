use std::io::Write;

use anyhow::{Context, Result, bail};
use probe_rs::probe::{
    DebugProbeInfo, DebugProbeSelector,
    cmsisdap::{PkobnUpdiM4809Info, query_pkobn_updi_m4809},
    list::Lister,
};

#[derive(clap::Parser)]
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
}

impl Cmd {
    pub fn run(self, lister: &Lister) -> Result<()> {
        let probe = select_probe(lister, self.probe.as_ref(), self.non_interactive)?;
        let selector = DebugProbeSelector::from(&probe);
        let info = query_pkobn_updi_m4809(&selector)?;

        println!("Probe: {}", probe);
        print_info(&info);

        Ok(())
    }
}

pub(crate) fn print_info(info: &PkobnUpdiM4809Info) {
    if let Some(vendor) = &info.cmsis_dap_vendor {
        println!("CMSIS-DAP vendor: {vendor}");
    }
    if let Some(product) = &info.cmsis_dap_product {
        println!("CMSIS-DAP product: {product}");
    }
    if let Some(serial) = &info.cmsis_dap_serial {
        println!("CMSIS-DAP serial: {serial}");
    }
    if let Some(firmware) = &info.cmsis_dap_firmware_version {
        println!("CMSIS-DAP firmware: {firmware}");
    }
    println!(
        "CMSIS-DAP packet size: {} bytes",
        info.cmsis_dap_packet_size
    );
    println!("Probe selector: {}", info.probe_selector);
    if let Some(serial) = &info.ice_serial {
        println!("EDBG serial: {serial}");
    }
    println!(
        "EDBG firmware: HW {} FW {}.{} (rel. {})",
        info.ice_firmware_version.hardware,
        info.ice_firmware_version.major,
        info.ice_firmware_version.minor,
        info.ice_firmware_version.release
    );
    println!(
        "Target voltage: {:.2} V",
        f32::from(info.target_voltage_mv) / 1000.0
    );
    println!("UPDI clock: {} kHz", info.updi_clock_khz);
    if let Some(family_id) = &info.partial_family_id {
        println!("Partial family ID: {family_id}");
    }
    println!("SIB: {}", info.sib_string);
    println!(
        "Chip revision: {}.{}",
        info.chip_revision >> 4,
        info.chip_revision & 0x0f
    );
    println!(
        "Signature: {:02x} {:02x} {:02x}",
        info.signature[0], info.signature[1], info.signature[2]
    );
    if let Some(part) = info.detected_part {
        println!("Detected part: {part}");
    }
}

fn select_probe(
    lister: &Lister,
    selector: Option<&DebugProbeSelector>,
    non_interactive: bool,
) -> Result<DebugProbeInfo> {
    if let Some(selector) = selector {
        let list = lister.list(Some(selector));
        return match list.as_slice() {
            [] => bail!("Probe not found"),
            [probe] => Ok(probe.clone()),
            _ if non_interactive => bail!("Multiple probes matched the selector"),
            _ => interactive_probe_select(&list),
        };
    }

    let list = lister.list_all();
    match list.as_slice() {
        [] => bail!("No probes found"),
        [probe] => Ok(probe.clone()),
        _ if non_interactive => bail!("Multiple probes found"),
        _ => interactive_probe_select(&list),
    }
}

fn interactive_probe_select(list: &[DebugProbeInfo]) -> Result<DebugProbeInfo> {
    println!("Available Probes:");
    for (index, probe_info) in list.iter().enumerate() {
        println!("{index}: {probe_info}");
    }

    print!("Selection: ");
    std::io::stdout()
        .flush()
        .context("Failed to flush stdout")?;

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("Failed to read probe selection")?;

    let probe_index = input
        .trim()
        .parse::<usize>()
        .context("Failed to parse probe index")?;

    list.get(probe_index)
        .cloned()
        .context("Selected probe index is out of range")
}
