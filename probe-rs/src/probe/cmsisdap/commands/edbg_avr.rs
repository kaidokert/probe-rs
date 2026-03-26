use super::{
    CmsisDapDevice, CmsisDapError, SendError, Status,
    general::{
        connect::{ConnectRequest, ConnectResponse},
        disconnect::{DisconnectRequest, DisconnectResponse},
        host_status::HostStatusRequest,
        info::{
            FirmwareVersionCommand, PacketSizeCommand, ProductIdCommand, SerialNumberCommand,
            VendorCommand,
        },
    },
    send_command,
};
use crate::probe::{DebugProbeError, DebugProbeSelector, ProbeError};

const PKOBN_UPDI_VID: u16 = 0x03eb;
const PKOBN_UPDI_PID: u16 = 0x2175;

const TOKEN: u8 = 0x0e;

const SCOPE_INFO: u8 = 0x00;
const SCOPE_GENERAL: u8 = 0x01;
const SCOPE_AVR: u8 = 0x12;

const CMD3_GET_INFO: u8 = 0x00;
const CMD3_SET_PARAMETER: u8 = 0x01;
const CMD3_GET_PARAMETER: u8 = 0x02;
const CMD3_SIGN_ON: u8 = 0x10;
const CMD3_SIGN_OFF: u8 = 0x11;
const CMD3_ENTER_PROGMODE: u8 = 0x15;
const CMD3_LEAVE_PROGMODE: u8 = 0x16;
const CMD3_READ_MEMORY: u8 = 0x21;

const CMD3_INFO_SERIAL: u8 = 0x81;

const RSP3_OK: u8 = 0x80;
const RSP3_INFO: u8 = 0x81;
const RSP3_DATA: u8 = 0x84;
const RSP3_STATUS_MASK: u8 = 0xe0;

const MTYPE_SRAM: u8 = 0x20;
const MTYPE_PRODSIG: u8 = 0xc6;
const MTYPE_SIB: u8 = 0xd3;

const PARM3_HW_VER: u8 = 0x00;
const PARM3_VTARGET: u8 = 0x00;
const PARM3_DEVICEDESC: u8 = 0x00;
const PARM3_ARCH: u8 = 0x00;
const PARM3_ARCH_UPDI: u8 = 5;
const PARM3_SESS_PURPOSE: u8 = 0x01;
const PARM3_SESS_PROGRAMMING: u8 = 1;
const PARM3_CONNECTION: u8 = 0x00;
const PARM3_CONN_UPDI: u8 = 8;
const PARM3_CLK_XMEGA_PDI: u8 = 0x31;

const EDBG_VENDOR_AVR_CMD: u8 = 0x80;
const EDBG_VENDOR_AVR_RSP: u8 = 0x81;

const DEFAULT_MINIMUM_CHARACTERISED_DIV1_VOLTAGE_MV: u16 = 4500;
const DEFAULT_MINIMUM_CHARACTERISED_DIV2_VOLTAGE_MV: u16 = 2700;
const DEFAULT_MINIMUM_CHARACTERISED_DIV4_VOLTAGE_MV: u16 = 2200;
const DEFAULT_MINIMUM_CHARACTERISED_DIV8_VOLTAGE_MV: u16 = 1500;
const MAX_FREQUENCY_SHARED_UPDI_PIN: u16 = 750;
const UPDI_ADDRESS_MODE_16BIT: u8 = 0;
const FUSES_SYSCFG0_OFFSET: u8 = 5;

const AVR_SIBLEN: usize = 32;
const M4809_SIGNATURE: [u8; 3] = [0x1e, 0x96, 0x51];
const M4809_SIGNATURE_BASE: u32 = 0x1100;
const M4809_SYSCFG_BASE: u32 = 0x0f00;

/// Firmware version reported by the EDBG/JTAG3 general parameter block.
#[derive(Debug, Clone)]
pub struct IceFirmwareVersion {
    /// Hardware version number.
    pub hardware: u8,
    /// Major firmware version number.
    pub major: u8,
    /// Minor firmware version number.
    pub minor: u8,
    /// Firmware release/build number.
    pub release: u16,
}

/// Narrow information block returned by the experimental `pkobn_updi` + `m4809` query path.
#[derive(Debug, Clone)]
pub struct PkobnUpdiM4809Info {
    /// Probe selector used to open the probe.
    pub probe_selector: DebugProbeSelector,
    /// CMSIS-DAP vendor string.
    pub cmsis_dap_vendor: Option<String>,
    /// CMSIS-DAP product string.
    pub cmsis_dap_product: Option<String>,
    /// CMSIS-DAP serial number.
    pub cmsis_dap_serial: Option<String>,
    /// CMSIS-DAP firmware version string.
    pub cmsis_dap_firmware_version: Option<String>,
    /// CMSIS-DAP packet size in bytes.
    pub cmsis_dap_packet_size: u16,
    /// EDBG serial number returned by the AVR info scope.
    pub ice_serial: Option<String>,
    /// EDBG firmware version reported by the general parameter block.
    pub ice_firmware_version: IceFirmwareVersion,
    /// Target voltage in millivolts.
    pub target_voltage_mv: u16,
    /// UPDI clock in kilohertz.
    pub updi_clock_khz: u16,
    /// Partial family identifier returned by AVR sign-on.
    pub partial_family_id: Option<String>,
    /// UPDI SIB string.
    pub sib_string: String,
    /// Raw chip revision byte.
    pub chip_revision: u8,
    /// Raw three-byte device signature.
    pub signature: [u8; 3],
    /// Resolved part name when the signature matches the hardcoded target.
    pub detected_part: Option<&'static str>,
}

#[derive(Debug, thiserror::Error, docsplay::Display)]
pub enum EdbgAvrError {
    /// Error while using the CMSIS-DAP transport layer.
    CmsisDap(#[from] CmsisDapError),
    /// Error while using the probe transport.
    Transport(#[from] SendError),
    /// The selected probe is not supported by the narrow EDBG AVR path: {selector}
    UnsupportedProbe { selector: DebugProbeSelector },
    /// Unexpected EDBG AVR response while handling {context}: {details}
    UnexpectedResponse {
        context: &'static str,
        details: String,
    },
    /// EDBG AVR command {context} failed with status code 0x{code:02x}
    CommandFailed { context: &'static str, code: u8 },
}

impl ProbeError for EdbgAvrError {}

/// Query a Microchip `pkobn_updi` / `03eb:2175` probe for a hardcoded `ATmega4809` target.
pub fn query_pkobn_updi_m4809(
    selector: &DebugProbeSelector,
) -> Result<PkobnUpdiM4809Info, DebugProbeError> {
    if selector.vendor_id != PKOBN_UPDI_VID || selector.product_id != PKOBN_UPDI_PID {
        return Err(EdbgAvrError::UnsupportedProbe {
            selector: selector.clone(),
        }
        .into());
    }

    let mut device = super::super::tools::open_device_from_selector(selector)?;
    device.drain();
    let packet_size = device.find_packet_size()? as u16;

    let cmsis_dap_vendor = trim_probe_string(send_command(&mut device, &VendorCommand {})?);
    let cmsis_dap_product = trim_probe_string(send_command(&mut device, &ProductIdCommand {})?);
    let cmsis_dap_serial = trim_probe_string(send_command(&mut device, &SerialNumberCommand {})?);
    let cmsis_dap_firmware_version =
        trim_probe_string(send_command(&mut device, &FirmwareVersionCommand {})?);
    let _ = send_command(&mut device, &PacketSizeCommand {})?;

    let mut transport = EdbgAvrTransport::new(device);

    let result = transport.query_target(PkobnUpdiM4809Info {
        probe_selector: selector.clone(),
        cmsis_dap_vendor,
        cmsis_dap_product,
        cmsis_dap_serial,
        cmsis_dap_firmware_version,
        cmsis_dap_packet_size: packet_size,
        ice_serial: None,
        ice_firmware_version: IceFirmwareVersion {
            hardware: 0,
            major: 0,
            minor: 0,
            release: 0,
        },
        target_voltage_mv: 0,
        updi_clock_khz: 0,
        partial_family_id: None,
        sib_string: String::new(),
        chip_revision: 0,
        signature: [0; 3],
        detected_part: None,
    });

    let _ = transport.cleanup();

    result.map_err(DebugProbeError::from)
}

struct EdbgAvrTransport {
    device: CmsisDapDevice,
    command_sequence: u16,
    prepared: bool,
    general_signed_on: bool,
    avr_signed_on: bool,
    programming_enabled: bool,
}

impl EdbgAvrTransport {
    fn new(device: CmsisDapDevice) -> Self {
        Self {
            device,
            command_sequence: 0,
            prepared: false,
            general_signed_on: false,
            avr_signed_on: false,
            programming_enabled: false,
        }
    }

    fn query_target(
        &mut self,
        mut info: PkobnUpdiM4809Info,
    ) -> Result<PkobnUpdiM4809Info, EdbgAvrError> {
        self.prepare()?;
        self.general_sign_on()?;

        info.ice_firmware_version = self.read_ice_firmware_version()?;
        info.ice_serial = self.get_info_string(CMD3_INFO_SERIAL)?;

        self.set_param(SCOPE_AVR, 0, PARM3_ARCH, &[PARM3_ARCH_UPDI])?;
        self.set_param(SCOPE_AVR, 0, PARM3_SESS_PURPOSE, &[PARM3_SESS_PROGRAMMING])?;
        self.set_param(SCOPE_AVR, 1, PARM3_CONNECTION, &[PARM3_CONN_UPDI])?;

        info.target_voltage_mv = self.get_u16_param(SCOPE_GENERAL, 1, PARM3_VTARGET)?;
        info.updi_clock_khz = self.get_u16_param(SCOPE_AVR, 1, PARM3_CLK_XMEGA_PDI)?;

        self.set_param(
            SCOPE_AVR,
            2,
            PARM3_DEVICEDESC,
            &m4809_updi_device_descriptor(),
        )?;

        let avr_sign_on_response = self.command(&[SCOPE_AVR, CMD3_SIGN_ON, 0, 0], "AVR sign-on")?;
        self.avr_signed_on = true;
        info.partial_family_id = partial_family_id_from_response(&avr_sign_on_response);

        info.sib_string = trim_ascii_nul(self.read_memory(MTYPE_SIB, 0, AVR_SIBLEN as u32)?);

        self.enter_progmode()?;

        let chip_revision = self.read_memory(MTYPE_SRAM, M4809_SYSCFG_BASE + 1, 1)?;
        if chip_revision.len() != 1 {
            return Err(EdbgAvrError::UnexpectedResponse {
                context: "chip revision",
                details: format!("expected 1 byte, got {}", chip_revision.len()),
            });
        }
        info.chip_revision = chip_revision[0];

        let signature = self.read_memory(MTYPE_PRODSIG, M4809_SIGNATURE_BASE, 3)?;
        if signature.len() != 3 {
            return Err(EdbgAvrError::UnexpectedResponse {
                context: "signature read",
                details: format!("expected 3 bytes, got {}", signature.len()),
            });
        }
        info.signature.copy_from_slice(&signature);
        info.detected_part = (info.signature == M4809_SIGNATURE).then_some("ATmega4809");

        Ok(info)
    }

    fn cleanup(&mut self) -> Result<(), EdbgAvrError> {
        let mut first_error = None;

        if self.programming_enabled
            && let Err(error) = self.leave_progmode()
        {
            first_error.get_or_insert(error);
        }
        if self.avr_signed_on
            && let Err(error) = self.avr_sign_off()
        {
            first_error.get_or_insert(error);
        }
        if self.general_signed_on
            && let Err(error) = self.general_sign_off()
        {
            first_error.get_or_insert(error);
        }
        if self.prepared
            && let Err(error) = self.finish_prepare()
        {
            first_error.get_or_insert(error);
        }

        if let Some(error) = first_error {
            Err(error)
        } else {
            Ok(())
        }
    }

    fn prepare(&mut self) -> Result<(), EdbgAvrError> {
        match send_command(&mut self.device, &ConnectRequest::Swd)? {
            ConnectResponse::SuccessfulInitForSWD => {}
            ConnectResponse::SuccessfulInitForJTAG | ConnectResponse::InitFailed => {
                return Err(EdbgAvrError::UnexpectedResponse {
                    context: "CMSIS-DAP connect",
                    details: "probe did not enter SWD mode".to_string(),
                });
            }
        }

        let _ = send_command(&mut self.device, &HostStatusRequest::connected(true))?;
        self.prepared = true;
        Ok(())
    }

    fn finish_prepare(&mut self) -> Result<(), EdbgAvrError> {
        let _ = send_command(&mut self.device, &HostStatusRequest::connected(false))?;
        let DisconnectResponse(status) = send_command(&mut self.device, &DisconnectRequest {})?;
        if status != Status::DapOk {
            return Err(EdbgAvrError::UnexpectedResponse {
                context: "CMSIS-DAP disconnect",
                details: format!("unexpected disconnect status {status:?}"),
            });
        }
        self.prepared = false;
        Ok(())
    }

    fn general_sign_on(&mut self) -> Result<(), EdbgAvrError> {
        let _ = self.command(&[SCOPE_GENERAL, CMD3_SIGN_ON, 0], "general sign-on")?;
        self.general_signed_on = true;
        Ok(())
    }

    fn general_sign_off(&mut self) -> Result<(), EdbgAvrError> {
        let _ = self.command(&[SCOPE_GENERAL, CMD3_SIGN_OFF, 0, 0], "general sign-off")?;
        self.general_signed_on = false;
        Ok(())
    }

    fn avr_sign_off(&mut self) -> Result<(), EdbgAvrError> {
        let _ = self.command(&[SCOPE_AVR, CMD3_SIGN_OFF, 0], "AVR sign-off")?;
        self.avr_signed_on = false;
        Ok(())
    }

    fn enter_progmode(&mut self) -> Result<(), EdbgAvrError> {
        let _ = self.command(&[SCOPE_AVR, CMD3_ENTER_PROGMODE, 0], "enter progmode")?;
        self.programming_enabled = true;
        Ok(())
    }

    fn leave_progmode(&mut self) -> Result<(), EdbgAvrError> {
        let _ = self.command(&[SCOPE_AVR, CMD3_LEAVE_PROGMODE, 0], "leave progmode")?;
        self.programming_enabled = false;
        Ok(())
    }

    fn read_ice_firmware_version(&mut self) -> Result<IceFirmwareVersion, EdbgAvrError> {
        let params = self.get_param(SCOPE_GENERAL, 0, PARM3_HW_VER, 5)?;
        if params.len() < 5 {
            return Err(EdbgAvrError::UnexpectedResponse {
                context: "ICE firmware version",
                details: format!("expected 5 bytes, got {}", params.len()),
            });
        }

        Ok(IceFirmwareVersion {
            hardware: params[0],
            major: params[1],
            minor: params[2],
            release: u16::from_le_bytes([params[3], params[4]]),
        })
    }

    fn get_info_string(&mut self, info_kind: u8) -> Result<Option<String>, EdbgAvrError> {
        let response = self.command(
            &[SCOPE_INFO, CMD3_GET_INFO, 0, info_kind],
            "get info string",
        )?;

        if response.len() < 3 || response[1] != RSP3_INFO {
            return Err(EdbgAvrError::UnexpectedResponse {
                context: "get info string",
                details: format!("unexpected response {:02x?}", response),
            });
        }

        Ok(trim_probe_string(Some(
            String::from_utf8_lossy(&response[3..]).into_owned(),
        )))
    }

    fn get_param(
        &mut self,
        scope: u8,
        section: u8,
        parameter: u8,
        length: u8,
    ) -> Result<Vec<u8>, EdbgAvrError> {
        let response = self.command(
            &[scope, CMD3_GET_PARAMETER, 0, section, parameter, length],
            "get parameter",
        )?;

        if response.len() < 3 || response[1] != RSP3_DATA {
            return Err(EdbgAvrError::UnexpectedResponse {
                context: "get parameter",
                details: format!("unexpected response {:02x?}", response),
            });
        }

        let mut value = response[3..].to_vec();
        value.truncate(length as usize);
        Ok(value)
    }

    fn get_u16_param(
        &mut self,
        scope: u8,
        section: u8,
        parameter: u8,
    ) -> Result<u16, EdbgAvrError> {
        let value = self.get_param(scope, section, parameter, 2)?;
        if value.len() < 2 {
            return Err(EdbgAvrError::UnexpectedResponse {
                context: "get 16-bit parameter",
                details: format!("expected 2 bytes, got {}", value.len()),
            });
        }

        Ok(u16::from_le_bytes([value[0], value[1]]))
    }

    fn set_param(
        &mut self,
        scope: u8,
        section: u8,
        parameter: u8,
        value: &[u8],
    ) -> Result<(), EdbgAvrError> {
        let length = u8::try_from(value.len()).map_err(|_| EdbgAvrError::UnexpectedResponse {
            context: "set parameter",
            details: format!("value too large: {}", value.len()),
        })?;

        let mut command = Vec::with_capacity(6 + value.len());
        command.extend_from_slice(&[scope, CMD3_SET_PARAMETER, 0, section, parameter, length]);
        command.extend_from_slice(value);
        let _ = self.command(&command, "set parameter")?;
        Ok(())
    }

    fn read_memory(
        &mut self,
        memory_type: u8,
        address: u32,
        length: u32,
    ) -> Result<Vec<u8>, EdbgAvrError> {
        let mut command = Vec::with_capacity(12);
        command.extend_from_slice(&[SCOPE_AVR, CMD3_READ_MEMORY, 0, memory_type]);
        command.extend_from_slice(&address.to_le_bytes());
        command.extend_from_slice(&length.to_le_bytes());

        let response = self.command(&command, "read memory")?;
        if response.len() < 3 || response[1] != RSP3_DATA {
            return Err(EdbgAvrError::UnexpectedResponse {
                context: "read memory",
                details: format!("unexpected response {:02x?}", response),
            });
        }

        let mut data = response[3..].to_vec();
        data.truncate(length as usize);
        Ok(data)
    }

    fn command(&mut self, payload: &[u8], context: &'static str) -> Result<Vec<u8>, EdbgAvrError> {
        self.send_payload(payload)?;
        let response = self.receive_payload()?;
        if response.len() < 2 {
            return Err(EdbgAvrError::UnexpectedResponse {
                context,
                details: format!("response too short: {:02x?}", response),
            });
        }

        if response[1] & RSP3_STATUS_MASK != RSP3_OK {
            let code = response.get(3).copied().unwrap_or(0);
            return Err(EdbgAvrError::CommandFailed { context, code });
        }

        Ok(response)
    }

    fn send_payload(&mut self, payload: &[u8]) -> Result<(), EdbgAvrError> {
        let packet_size = self.packet_size();
        let first_capacity =
            packet_size
                .checked_sub(8)
                .ok_or_else(|| EdbgAvrError::UnexpectedResponse {
                    context: "EDBG send",
                    details: format!("packet size {packet_size} too small"),
                })?;
        let continuation_capacity =
            packet_size
                .checked_sub(4)
                .ok_or_else(|| EdbgAvrError::UnexpectedResponse {
                    context: "EDBG send",
                    details: format!("packet size {packet_size} too small"),
                })?;

        let nfragments = if payload.len() <= first_capacity {
            1usize
        } else {
            1 + (payload.len() - first_capacity).div_ceil(continuation_capacity)
        };
        let nfragments_u8 =
            u8::try_from(nfragments).map_err(|_| EdbgAvrError::UnexpectedResponse {
                context: "EDBG send",
                details: format!("payload fragmented into too many packets: {nfragments}"),
            })?;

        let mut cursor = 0usize;

        for fragment_index in 0..nfragments {
            let fragment_number =
                u8::try_from(fragment_index + 1).map_err(|_| EdbgAvrError::UnexpectedResponse {
                    context: "EDBG send",
                    details: format!("invalid fragment number: {}", fragment_index + 1),
                })?;
            let is_first = fragment_index == 0;
            let is_last = fragment_index + 1 == nfragments;
            let max_chunk = if is_first {
                first_capacity
            } else {
                continuation_capacity
            };
            let chunk_len = (payload.len() - cursor).min(max_chunk);

            let mut packet = Vec::with_capacity(packet_size);
            packet.push(EDBG_VENDOR_AVR_CMD);
            packet.push((fragment_number << 4) | nfragments_u8);

            let fragment_len = if is_first { chunk_len + 4 } else { chunk_len };
            let fragment_len_u16 =
                u16::try_from(fragment_len).map_err(|_| EdbgAvrError::UnexpectedResponse {
                    context: "EDBG send",
                    details: format!("fragment too large: {fragment_len}"),
                })?;
            packet.extend_from_slice(&fragment_len_u16.to_be_bytes());

            if is_first {
                packet.push(TOKEN);
                packet.push(0);
                packet.extend_from_slice(&self.command_sequence.to_le_bytes());
            }

            packet.extend_from_slice(&payload[cursor..cursor + chunk_len]);
            cursor += chunk_len;

            let ack = self.transfer(&packet)?;
            if ack.first().copied() != Some(EDBG_VENDOR_AVR_CMD) {
                return Err(EdbgAvrError::UnexpectedResponse {
                    context: "EDBG send ack",
                    details: format!("unexpected ack {:02x?}", ack),
                });
            }
            if is_last && ack.get(1).copied() != Some(0x01) {
                return Err(EdbgAvrError::UnexpectedResponse {
                    context: "EDBG send ack",
                    details: format!("last-fragment completion missing in ack {:02x?}", ack),
                });
            }
        }

        Ok(())
    }

    fn receive_payload(&mut self) -> Result<Vec<u8>, EdbgAvrError> {
        loop {
            let mut collected = Vec::new();
            let mut fragment_count = None;
            let mut expected_fragment = 1u8;

            loop {
                let response = self.transfer(&[EDBG_VENDOR_AVR_RSP])?;
                if response.first().copied() != Some(EDBG_VENDOR_AVR_RSP) {
                    return Err(EdbgAvrError::UnexpectedResponse {
                        context: "EDBG receive",
                        details: format!("unexpected response prefix {:02x?}", response),
                    });
                }
                if response.get(1).copied() == Some(0) {
                    return Err(EdbgAvrError::UnexpectedResponse {
                        context: "EDBG receive",
                        details: "no response data available".to_string(),
                    });
                }

                let fragment_info = response[1];
                let total_fragments = fragment_info & 0x0f;
                let fragment_number = (fragment_info >> 4) & 0x0f;

                if let Some(existing_count) = fragment_count {
                    if existing_count != total_fragments {
                        return Err(EdbgAvrError::UnexpectedResponse {
                            context: "EDBG receive",
                            details: format!("inconsistent fragment count {:02x?}", response),
                        });
                    }
                } else {
                    fragment_count = Some(total_fragments);
                }

                if fragment_number != expected_fragment {
                    return Err(EdbgAvrError::UnexpectedResponse {
                        context: "EDBG receive",
                        details: format!(
                            "expected fragment {expected_fragment}, received {fragment_number}"
                        ),
                    });
                }
                expected_fragment += 1;

                let claimed_len = u16::from_be_bytes([response[2], response[3]]) as usize;
                let available_len = response.len().saturating_sub(4);
                let fragment_len = claimed_len.min(available_len);
                collected.extend_from_slice(&response[4..4 + fragment_len]);

                if fragment_number == total_fragments {
                    break;
                }
            }

            if collected.len() < 4 || collected[0] != TOKEN {
                return Err(EdbgAvrError::UnexpectedResponse {
                    context: "EDBG receive",
                    details: format!("malformed response {:02x?}", collected),
                });
            }

            let received_sequence = u16::from_le_bytes([collected[1], collected[2]]);
            if received_sequence != self.command_sequence {
                continue;
            }

            self.command_sequence = if self.command_sequence == 0xfffe {
                0
            } else {
                self.command_sequence + 1
            };

            return Ok(collected[3..].to_vec());
        }
    }

    fn transfer(&mut self, payload: &[u8]) -> Result<Vec<u8>, SendError> {
        let packet_size = self.packet_size();
        let buffer_len = packet_size + 1;
        let mut tx = vec![0u8; buffer_len];
        tx[1..1 + payload.len()].copy_from_slice(payload);

        let write_len = match self.device {
            CmsisDapDevice::V1 { .. } => buffer_len,
            CmsisDapDevice::V2 { .. } => payload.len() + 1,
        };
        let _ = self.device.write(&tx[..write_len])?;

        let mut rx = vec![0u8; buffer_len];
        let read_len = self.device.read(&mut rx)?;
        rx.truncate(read_len);
        Ok(rx)
    }

    fn packet_size(&self) -> usize {
        match self.device {
            CmsisDapDevice::V1 { report_size, .. } => report_size,
            CmsisDapDevice::V2 {
                max_packet_size, ..
            } => max_packet_size,
        }
    }
}

fn trim_probe_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim_end_matches('\0').to_string())
        .filter(|value| !value.is_empty())
}

fn trim_ascii_nul(mut bytes: Vec<u8>) -> String {
    while matches!(bytes.last(), Some(0)) {
        let _ = bytes.pop();
    }
    String::from_utf8_lossy(&bytes).trim_end().to_string()
}

fn partial_family_id_from_response(response: &[u8]) -> Option<String> {
    (response.len() >= 7 && response[1] == RSP3_DATA)
        .then(|| String::from_utf8_lossy(&response[3..7]).into_owned())
        .filter(|family| !family.trim_end_matches('\0').is_empty())
        .map(|family| family.trim_end_matches('\0').to_string())
}

fn m4809_updi_device_descriptor() -> [u8; 48] {
    let mut descriptor = [0u8; 48];

    descriptor[0..2].copy_from_slice(&0x4000u16.to_le_bytes());
    descriptor[2] = 128;
    descriptor[3] = 64;
    descriptor[4..6].copy_from_slice(&0x1000u16.to_le_bytes());
    descriptor[6..8].copy_from_slice(&0x0f80u16.to_le_bytes());
    descriptor[8..10].copy_from_slice(&DEFAULT_MINIMUM_CHARACTERISED_DIV1_VOLTAGE_MV.to_le_bytes());
    descriptor[10..12]
        .copy_from_slice(&DEFAULT_MINIMUM_CHARACTERISED_DIV2_VOLTAGE_MV.to_le_bytes());
    descriptor[12..14]
        .copy_from_slice(&DEFAULT_MINIMUM_CHARACTERISED_DIV4_VOLTAGE_MV.to_le_bytes());
    descriptor[14..16]
        .copy_from_slice(&DEFAULT_MINIMUM_CHARACTERISED_DIV8_VOLTAGE_MV.to_le_bytes());
    descriptor[16..18].copy_from_slice(&MAX_FREQUENCY_SHARED_UPDI_PIN.to_le_bytes());
    descriptor[18..22].copy_from_slice(&0x0000_c000u32.to_le_bytes());
    descriptor[22..24].copy_from_slice(&256u16.to_le_bytes());
    descriptor[24..26].copy_from_slice(&64u16.to_le_bytes());
    descriptor[26] = 10;
    descriptor[27] = FUSES_SYSCFG0_OFFSET;
    descriptor[28] = 0xff;
    descriptor[29] = 0x00;
    descriptor[30] = 0xff;
    descriptor[31] = 0x00;
    descriptor[32..34].copy_from_slice(&0x1400u16.to_le_bytes());
    descriptor[34..36].copy_from_slice(&0x1300u16.to_le_bytes());
    descriptor[36..38].copy_from_slice(&(M4809_SIGNATURE_BASE as u16).to_le_bytes());
    descriptor[38..40].copy_from_slice(&0x1280u16.to_le_bytes());
    descriptor[40..42].copy_from_slice(&0x128au16.to_le_bytes());
    descriptor[42] = M4809_SIGNATURE[1];
    descriptor[43] = M4809_SIGNATURE[2];
    descriptor[46] = UPDI_ADDRESS_MODE_16BIT;
    descriptor[47] = 1;

    descriptor
}

#[cfg(test)]
mod tests {
    use super::{M4809_SIGNATURE, m4809_updi_device_descriptor};

    #[test]
    fn m4809_descriptor_matches_expected_layout() {
        let descriptor = m4809_updi_device_descriptor();

        assert_eq!(descriptor.len(), 48);
        assert_eq!(&descriptor[0..2], &0x4000u16.to_le_bytes());
        assert_eq!(descriptor[2], 128);
        assert_eq!(descriptor[3], 64);
        assert_eq!(&descriptor[4..6], &0x1000u16.to_le_bytes());
        assert_eq!(&descriptor[6..8], &0x0f80u16.to_le_bytes());
        assert_eq!(&descriptor[36..38], &0x1100u16.to_le_bytes());
        assert_eq!(descriptor[42], M4809_SIGNATURE[1]);
        assert_eq!(descriptor[43], M4809_SIGNATURE[2]);
        assert_eq!(descriptor[46], 0);
        assert_eq!(descriptor[47], 1);
    }
}
