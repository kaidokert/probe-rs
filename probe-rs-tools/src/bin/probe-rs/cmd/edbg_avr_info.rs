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
        let probe = select_probe_for_edbg(lister, self.probe.as_ref(), self.non_interactive)?;
        let selector = DebugProbeSelector::from(&probe);
        let info = query_pkobn_updi_m4809(&selector)?;

        tracing::info!("Probe: {}", probe);
        print_info(&info);

        Ok(())
    }
}

pub(crate) fn format_info_lines(info: &PkobnUpdiM4809Info) -> Vec<String> {
    let mut lines = Vec::new();

    if let Some(vendor) = &info.cmsis_dap_vendor {
        lines.push(format!("CMSIS-DAP vendor: {vendor}"));
    }
    if let Some(product) = &info.cmsis_dap_product {
        lines.push(format!("CMSIS-DAP product: {product}"));
    }
    if let Some(serial) = &info.cmsis_dap_serial {
        lines.push(format!("CMSIS-DAP serial: {serial}"));
    }
    if let Some(firmware) = &info.cmsis_dap_firmware_version {
        lines.push(format!("CMSIS-DAP firmware: {firmware}"));
    }
    lines.push(format!(
        "CMSIS-DAP packet size: {} bytes",
        info.cmsis_dap_packet_size
    ));
    lines.push(format!("Probe selector: {}", info.probe_selector));
    if let Some(serial) = &info.ice_serial {
        lines.push(format!("EDBG serial: {serial}"));
    }
    lines.push(format!(
        "EDBG firmware: HW {} FW {}.{} (rel. {})",
        info.ice_firmware_version.hardware,
        info.ice_firmware_version.major,
        info.ice_firmware_version.minor,
        info.ice_firmware_version.release
    ));
    lines.push(format!(
        "Target voltage: {:.2} V",
        f32::from(info.target_voltage_mv) / 1000.0
    ));
    lines.push(format!("UPDI clock: {} kHz", info.updi_clock_khz));
    if let Some(family_id) = &info.partial_family_id {
        lines.push(format!("Partial family ID: {family_id}"));
    }
    lines.push(format!("SIB: {}", info.sib_string));
    lines.push(format!(
        "Chip revision: {}.{}",
        info.chip_revision >> 4,
        info.chip_revision & 0x0f
    ));
    lines.push(format!(
        "Signature: {:02x} {:02x} {:02x}",
        info.signature[0], info.signature[1], info.signature[2]
    ));
    lines.push(format!("Lock byte: {:02x}", info.lock_byte));
    lines.push(format!("Fuses: {}", format_hex_bytes(&info.fuses)));
    lines.extend(hex_dump_lines("USERROW", &info.userrow));
    lines.extend(hex_dump_lines("PRODSIG", &info.prodsig));
    if let Some(part) = info.detected_part {
        lines.push(format!("Detected part: {part}"));
    }

    lines
}

pub(crate) fn print_info(info: &PkobnUpdiM4809Info) {
    for line in format_info_lines(info) {
        tracing::info!("{line}");
    }
}

fn format_hex_bytes(data: &[u8]) -> String {
    data.iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn hex_dump_lines(label: &str, data: &[u8]) -> Vec<String> {
    let mut lines = vec![format!("{label}:")];
    for (offset, chunk) in data.chunks(16).enumerate() {
        lines.push(format!(
            "  {:04x}: {}",
            offset * 16,
            format_hex_bytes(chunk)
        ));
    }
    lines
}

pub(crate) fn select_probe_for_edbg(
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
