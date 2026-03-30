//! AVR architecture support for UPDI-attached chips via EDBG/nEDBG probes.
//!
//! Provides [`CoreInterface`] + [`MemoryInterface`] that routes operations
//! through the EDBG AVR transport layer, including OCD-based debug support
//! (halt, step, breakpoints, register reads).

use crate::{
    CoreInterface, CoreRegister, CoreStatus, CoreType, Error, MemoryInterface,
    core::{
        Architecture, CoreInformation, CoreRegisters, RegisterId, RegisterValue,
        registers::UnwindRule,
    },
    probe::{
        DebugProbe,
        cmsisdap::{
            AvrChipDescriptor, AvrDebugState, AvrMemoryRegion, CmsisDap, DEBUG_MTYPE_EEPROM,
            DEBUG_MTYPE_FLASH, DEBUG_MTYPE_SRAM, debug_avr_cleanup, debug_avr_halt,
            debug_avr_hw_break_clear, debug_avr_hw_break_set, debug_avr_read_memory,
            debug_avr_read_pc, debug_avr_read_registers, debug_avr_read_sp, debug_avr_read_sreg,
            debug_avr_reset, debug_avr_run, debug_avr_status, debug_avr_step,
            read_attached_pkobn_updi_region, write_attached_pkobn_updi_flash,
        },
    },
};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// Core state for AVR, including persistent OCD debug session state.
#[derive(Debug, Default)]
pub struct AvrCoreState {
    /// Debug session state tracked across CoreInterface calls.
    pub debug_state: AvrDebugState,
}

impl AvrCoreState {
    /// Create a new AVR core state.
    pub fn new() -> Self {
        Self::default()
    }
}

// ---- AVR Register Definitions ----
//
// Register IDs:
//   0..31  -> R0..R31 (8-bit general purpose)
//   32     -> PC (program counter, 32-bit byte address)
//   33     -> SP (stack pointer, 16-bit)
//   34     -> SREG (status register, 8-bit)
//
// For the CoreInterface trait we need designated PC, SP, FP, and RA registers.
// AVR GCC convention: Y (R28:R29) is the frame pointer. The return address
// lives on the stack, not in a register, so we use a placeholder for RA.

macro_rules! avr_gpr {
    ($name:ident, $id:expr, $label:expr) => {
        static $name: CoreRegister = CoreRegister {
            roles: &[crate::RegisterRole::Core($label)],
            id: RegisterId($id),
            data_type: crate::RegisterDataType::UnsignedInteger(8),
            unwind_rule: UnwindRule::Clear,
        };
    };
}

avr_gpr!(AVR_R0, 0, "R0");
avr_gpr!(AVR_R1, 1, "R1");
avr_gpr!(AVR_R2, 2, "R2");
avr_gpr!(AVR_R3, 3, "R3");
avr_gpr!(AVR_R4, 4, "R4");
avr_gpr!(AVR_R5, 5, "R5");
avr_gpr!(AVR_R6, 6, "R6");
avr_gpr!(AVR_R7, 7, "R7");
avr_gpr!(AVR_R8, 8, "R8");
avr_gpr!(AVR_R9, 9, "R9");
avr_gpr!(AVR_R10, 10, "R10");
avr_gpr!(AVR_R11, 11, "R11");
avr_gpr!(AVR_R12, 12, "R12");
avr_gpr!(AVR_R13, 13, "R13");
avr_gpr!(AVR_R14, 14, "R14");
avr_gpr!(AVR_R15, 15, "R15");
avr_gpr!(AVR_R16, 16, "R16");
avr_gpr!(AVR_R17, 17, "R17");
avr_gpr!(AVR_R18, 18, "R18");
avr_gpr!(AVR_R19, 19, "R19");
avr_gpr!(AVR_R20, 20, "R20");
avr_gpr!(AVR_R21, 21, "R21");
avr_gpr!(AVR_R22, 22, "R22");
avr_gpr!(AVR_R23, 23, "R23");
avr_gpr!(AVR_R24, 24, "R24");
avr_gpr!(AVR_R25, 25, "R25");
avr_gpr!(AVR_R26, 26, "R26");
avr_gpr!(AVR_R27, 27, "R27");

// R28 (Y low) serves as frame pointer in AVR GCC convention
static AVR_R28: CoreRegister = CoreRegister {
    roles: &[
        crate::RegisterRole::Core("R28"),
        crate::RegisterRole::FramePointer,
    ],
    id: RegisterId(28),
    data_type: crate::RegisterDataType::UnsignedInteger(8),
    unwind_rule: UnwindRule::Preserve,
};

avr_gpr!(AVR_R29, 29, "R29");
avr_gpr!(AVR_R30, 30, "R30");
avr_gpr!(AVR_R31, 31, "R31");

// GDB AVR register numbering: r0-r31=0-31, SREG=32, SP=33, PC=34
// This order MUST match GDB's built-in avr-tdep.c layout since GDB ignores
// target-supplied register descriptions for AVR architecture.
static AVR_SREG: CoreRegister = CoreRegister {
    roles: &[
        crate::RegisterRole::Core("SREG"),
        crate::RegisterRole::ProcessorStatus,
    ],
    id: RegisterId(32),
    data_type: crate::RegisterDataType::UnsignedInteger(8),
    unwind_rule: UnwindRule::SpecialRule,
};

static AVR_SP: CoreRegister = CoreRegister {
    roles: &[
        crate::RegisterRole::Core("SP"),
        crate::RegisterRole::StackPointer,
    ],
    id: RegisterId(33),
    data_type: crate::RegisterDataType::UnsignedInteger(16),
    unwind_rule: UnwindRule::SpecialRule,
};

static AVR_PC: CoreRegister = CoreRegister {
    roles: &[
        crate::RegisterRole::Core("PC"),
        crate::RegisterRole::ProgramCounter,
    ],
    id: RegisterId(34),
    data_type: crate::RegisterDataType::UnsignedInteger(32),
    unwind_rule: UnwindRule::SpecialRule,
};

// Return address placeholder — AVR pushes RA onto the stack, there is no
// dedicated RA register. We alias it to R30 (Z low) as a best-effort stand-in.
static AVR_RA: CoreRegister = CoreRegister {
    roles: &[
        crate::RegisterRole::Core("RA"),
        crate::RegisterRole::ReturnAddress,
    ],
    id: RegisterId(30),
    data_type: crate::RegisterDataType::UnsignedInteger(8),
    unwind_rule: UnwindRule::SpecialRule,
};

/// All AVR registers exposed through the debug interface.
pub static AVR_CORE_REGISTERS: LazyLock<CoreRegisters> = LazyLock::new(|| {
    CoreRegisters::new(vec![
        &AVR_R0, &AVR_R1, &AVR_R2, &AVR_R3, &AVR_R4, &AVR_R5, &AVR_R6, &AVR_R7, &AVR_R8, &AVR_R9,
        &AVR_R10, &AVR_R11, &AVR_R12, &AVR_R13, &AVR_R14, &AVR_R15, &AVR_R16, &AVR_R17, &AVR_R18,
        &AVR_R19, &AVR_R20, &AVR_R21, &AVR_R22, &AVR_R23, &AVR_R24, &AVR_R25, &AVR_R26, &AVR_R27,
        &AVR_R28, &AVR_R29, &AVR_R30, &AVR_R31, &AVR_SREG, &AVR_SP, &AVR_PC,
    ])
});

/// An AVR core that implements memory and debug operations through the EDBG transport.
///
/// Supports halt, step, breakpoints, and register reads through the OCD module,
/// as well as flash/erase/read through the programming interface.
pub struct Avr<'probe> {
    probe: &'probe mut CmsisDap,
    chip: &'static AvrChipDescriptor,
    state: &'probe mut AvrCoreState,
}

impl<'probe> Avr<'probe> {
    /// Create a new AVR core interface.
    pub fn new(
        probe: &'probe mut CmsisDap,
        state: &'probe mut AvrCoreState,
        chip: &'static AvrChipDescriptor,
    ) -> Self {
        Self { probe, state, chip }
    }

    /// Map an absolute address to an (AvrMemoryRegion, region-relative offset) pair.
    ///
    /// The address space layout uses the chip descriptor's base addresses:
    /// Flash is addressed as 0-based offsets (`[0 .. flash_size)`), but we also accept
    /// the data-space mapping (`[flash_base .. flash_base + flash_size)`) and translate it
    /// back to a 0-based offset automatically.
    ///
    /// - `[0 .. flash_size)` -> Flash (region offset = address)
    /// - `[flash_base .. flash_base + flash_size)` -> Flash (region offset = address - flash_base)
    /// - `[eeprom_base .. eeprom_base + eeprom_size)` -> EEPROM
    /// - `[fuses_base .. fuses_base + fuses_size)` -> Fuses
    /// - `[lock_base .. lock_base + lock_size)` -> Lock
    /// - `[userrow_base .. userrow_base + userrow_size)` -> UserRow
    /// - `[signature_base .. signature_base + prodsig_size)` -> ProdSig
    fn address_to_region(&self, address: u64) -> Result<(AvrMemoryRegion, u32), Error> {
        let addr = u32::try_from(address).map_err(|_| {
            Error::Other(format!("AVR address {address:#010x} exceeds 32-bit range"))
        })?;
        let chip = self.chip;

        if addr < chip.flash_size {
            return Ok((AvrMemoryRegion::Flash, addr));
        }
        if chip.flash_base > 0
            && addr >= chip.flash_base
            && addr < chip.flash_base + chip.flash_size
        {
            return Ok((AvrMemoryRegion::Flash, addr - chip.flash_base));
        }
        if addr >= chip.eeprom_base && addr < chip.eeprom_base + chip.eeprom_size {
            return Ok((AvrMemoryRegion::Eeprom, addr - chip.eeprom_base));
        }
        if addr >= chip.fuses_base && addr < chip.fuses_base + chip.fuses_size {
            return Ok((AvrMemoryRegion::Fuses, addr - chip.fuses_base));
        }
        if addr >= chip.lock_base && addr < chip.lock_base + chip.lock_size {
            return Ok((AvrMemoryRegion::Lock, addr - chip.lock_base));
        }
        if addr >= chip.userrow_base && addr < chip.userrow_base + chip.userrow_size {
            return Ok((AvrMemoryRegion::UserRow, addr - chip.userrow_base));
        }
        if addr >= chip.signature_base && addr < chip.signature_base + chip.prodsig_size {
            return Ok((AvrMemoryRegion::ProdSig, addr - chip.signature_base));
        }

        Err(Error::Other(format!(
            "AVR address {addr:#010x} does not map to any known memory region for {}",
            chip.name
        )))
    }

    /// Map an absolute data-space address to a (debug memtype, address) pair for
    /// use when the OCD debug transport is active.
    fn debug_address_to_memtype(&self, address: u64) -> Result<(u8, u32), Error> {
        let addr = u32::try_from(address).map_err(|_| {
            Error::Other(format!("AVR address {address:#010x} exceeds 32-bit range"))
        })?;
        let chip = self.chip;

        // GDB AVR address spaces:
        //   0x000000..          -> Program memory (flash), byte-addressed
        //   0x800000..0x80FFFF  -> Data memory (SRAM/IO/peripherals)
        // Strip the 0x800000 GDB data-space offset if present.
        const GDB_AVR_DATA_OFFSET: u32 = 0x800000;

        if addr >= GDB_AVR_DATA_OFFSET {
            let data_addr = addr - GDB_AVR_DATA_OFFSET;
            // Map through the data-space: SRAM, IO, peripherals, memory-mapped flash
            if chip.flash_base > 0
                && data_addr >= chip.flash_base
                && data_addr < chip.flash_base + chip.flash_size
            {
                return Ok((DEBUG_MTYPE_FLASH, data_addr - chip.flash_base));
            }
            if data_addr >= chip.eeprom_base && data_addr < chip.eeprom_base + chip.eeprom_size {
                return Ok((DEBUG_MTYPE_EEPROM, data_addr));
            }
            return Ok((DEBUG_MTYPE_SRAM, data_addr));
        }

        // Program-space address (flash), byte-addressed.
        // Flash is memory-mapped in the data space at flash_base, so read it
        // via SRAM memtype at the data-space address.
        if addr < chip.flash_size {
            return Ok((DEBUG_MTYPE_SRAM, chip.flash_base + addr));
        }

        // Fallback: treat as data space
        Ok((DEBUG_MTYPE_SRAM, addr))
    }
}

// ---- MemoryInterface (directly on Avr, since we own the probe) ----

impl MemoryInterface for Avr<'_> {
    fn supports_native_64bit_access(&mut self) -> bool {
        false
    }

    fn read_64(&mut self, address: u64, data: &mut [u64]) -> Result<(), Error> {
        let byte_len = data.len() * 8;
        let mut bytes = vec![0u8; byte_len];
        self.read_8(address, &mut bytes)?;
        for (chunk, word) in bytes.chunks_exact(8).zip(data.iter_mut()) {
            *word = u64::from_le_bytes(chunk.try_into().unwrap());
        }
        Ok(())
    }

    fn read_32(&mut self, address: u64, data: &mut [u32]) -> Result<(), Error> {
        let byte_len = data.len() * 4;
        let mut bytes = vec![0u8; byte_len];
        self.read_8(address, &mut bytes)?;
        for (chunk, word) in bytes.chunks_exact(4).zip(data.iter_mut()) {
            *word = u32::from_le_bytes(chunk.try_into().unwrap());
        }
        Ok(())
    }

    fn read_16(&mut self, address: u64, data: &mut [u16]) -> Result<(), Error> {
        let byte_len = data.len() * 2;
        let mut bytes = vec![0u8; byte_len];
        self.read_8(address, &mut bytes)?;
        for (chunk, word) in bytes.chunks_exact(2).zip(data.iter_mut()) {
            *word = u16::from_le_bytes(chunk.try_into().unwrap());
        }
        Ok(())
    }

    fn read_8(&mut self, address: u64, data: &mut [u8]) -> Result<(), Error> {
        if data.is_empty() {
            return Ok(());
        }
        tracing::trace!(
            "AVR read_8: addr=0x{address:08x} len={} debug_mode={}",
            data.len(),
            self.state.debug_state.in_debug_mode
        );
        if self.state.debug_state.in_debug_mode {
            // In debug mode, use the debug transport directly
            let (memtype, addr) = self.debug_address_to_memtype(address)?;
            tracing::trace!("AVR read_8: memtype=0x{memtype:02x} mapped_addr=0x{addr:04x}");
            let length = u32::try_from(data.len())
                .map_err(|_| Error::Other("AVR read length exceeds 32-bit range".to_string()))?;
            let bytes = match debug_avr_read_memory(
                self.probe,
                self.chip,
                &mut self.state.debug_state,
                memtype,
                addr,
                length,
            ) {
                Ok(b) => b,
                Err(e) => {
                    tracing::debug!("AVR read_8: EDBG read failed: {e}");
                    return Err(e.into());
                }
            };
            if bytes.len() < data.len() {
                return Err(Error::Other(format!(
                    "AVR debug read returned {} bytes, expected {}",
                    bytes.len(),
                    data.len()
                )));
            }
            data.copy_from_slice(&bytes[..data.len()]);
            Ok(())
        } else {
            // Programming mode path
            let (region, offset) = self.address_to_region(address)?;
            let length = u32::try_from(data.len())
                .map_err(|_| Error::Other("AVR read length exceeds 32-bit range".to_string()))?;
            let bytes =
                read_attached_pkobn_updi_region(self.probe, self.chip, region, offset, length)?;
            if bytes.len() < data.len() {
                return Err(Error::Other(format!(
                    "AVR read returned {} bytes, expected {}",
                    bytes.len(),
                    data.len()
                )));
            }
            data.copy_from_slice(&bytes[..data.len()]);
            Ok(())
        }
    }

    fn read(&mut self, address: u64, data: &mut [u8]) -> Result<(), Error> {
        self.read_8(address, data)
    }

    fn write_64(&mut self, address: u64, data: &[u64]) -> Result<(), Error> {
        let bytes: Vec<u8> = data.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.write_8(address, &bytes)
    }

    fn write_32(&mut self, address: u64, data: &[u32]) -> Result<(), Error> {
        let bytes: Vec<u8> = data.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.write_8(address, &bytes)
    }

    fn write_16(&mut self, address: u64, data: &[u16]) -> Result<(), Error> {
        let bytes: Vec<u8> = data.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.write_8(address, &bytes)
    }

    fn write_8(&mut self, address: u64, data: &[u8]) -> Result<(), Error> {
        if data.is_empty() {
            return Ok(());
        }
        let (region, offset) = self.address_to_region(address)?;
        if region != AvrMemoryRegion::Flash {
            return Err(Error::NotImplemented(
                "AVR writes currently only support the flash region",
            ));
        }
        write_attached_pkobn_updi_flash(self.probe, self.chip, offset, data)?;
        Ok(())
    }

    fn write(&mut self, address: u64, data: &[u8]) -> Result<(), Error> {
        self.write_8(address, data)
    }

    fn supports_8bit_transfers(&self) -> Result<bool, Error> {
        Ok(true)
    }

    fn flush(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

// ---- CoreInterface (OCD debug support) ----

impl CoreInterface for Avr<'_> {
    fn wait_for_core_halted(&mut self, timeout: Duration) -> Result<(), Error> {
        let start = Instant::now();
        loop {
            match debug_avr_status(self.probe, self.chip, &mut self.state.debug_state) {
                Ok(true) => return Ok(()),
                Ok(false) => {
                    if start.elapsed() >= timeout {
                        return Err(Error::Timeout);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => return Err(Error::Probe(e)),
            }
        }
    }

    fn core_halted(&mut self) -> Result<bool, Error> {
        if !self.state.debug_state.in_debug_mode {
            // Not in debug mode yet — report halted for compatibility with
            // callers that check before entering debug mode.
            return Ok(true);
        }
        debug_avr_status(self.probe, self.chip, &mut self.state.debug_state).map_err(Error::Probe)
    }

    fn status(&mut self) -> Result<CoreStatus, Error> {
        if !self.state.debug_state.in_debug_mode {
            return Ok(CoreStatus::Unknown);
        }
        let halted = debug_avr_status(self.probe, self.chip, &mut self.state.debug_state)
            .map_err(Error::Probe)?;
        if halted {
            // Check if we stopped at a breakpoint address
            let reason = if self.state.debug_state.hw_breakpoint.is_some() {
                crate::HaltReason::Breakpoint(crate::BreakpointCause::Hardware)
            } else {
                crate::HaltReason::Request
            };
            Ok(CoreStatus::Halted(reason))
        } else {
            Ok(CoreStatus::Running)
        }
    }

    fn halt(&mut self, _timeout: Duration) -> Result<CoreInformation, Error> {
        let pc = debug_avr_halt(self.probe, self.chip, &mut self.state.debug_state)
            .map_err(Error::Probe)?;
        Ok(CoreInformation { pc: pc as u64 })
    }

    fn run(&mut self) -> Result<(), Error> {
        debug_avr_run(self.probe, self.chip, &mut self.state.debug_state).map_err(Error::Probe)
    }

    fn reset(&mut self) -> Result<(), Error> {
        if self.state.debug_state.in_debug_mode {
            debug_avr_reset(self.probe, self.chip, &mut self.state.debug_state)
                .map_err(Error::Probe)
        } else {
            self.probe.target_reset().map_err(Error::from)
        }
    }

    fn reset_and_halt(&mut self, _timeout: Duration) -> Result<CoreInformation, Error> {
        // Enter debug mode, reset, then halt
        debug_avr_reset(self.probe, self.chip, &mut self.state.debug_state)
            .map_err(Error::Probe)?;
        let pc = debug_avr_halt(self.probe, self.chip, &mut self.state.debug_state)
            .map_err(Error::Probe)?;
        Ok(CoreInformation { pc: pc as u64 })
    }

    fn step(&mut self) -> Result<CoreInformation, Error> {
        let pc = debug_avr_step(self.probe, self.chip, &mut self.state.debug_state)
            .map_err(Error::Probe)?;
        Ok(CoreInformation { pc: pc as u64 })
    }

    fn read_core_reg(&mut self, address: RegisterId) -> Result<RegisterValue, Error> {
        let id = address.0;
        match id {
            0..=31 => {
                let regs =
                    debug_avr_read_registers(self.probe, self.chip, &mut self.state.debug_state)
                        .map_err(Error::Probe)?;
                Ok(RegisterValue::U32(regs[id as usize] as u32))
            }
            32 => {
                // SREG (GDB register 32)
                let sreg = debug_avr_read_sreg(self.probe, self.chip, &mut self.state.debug_state)
                    .map_err(Error::Probe)?;
                Ok(RegisterValue::U32(sreg as u32))
            }
            33 => {
                // SP (GDB register 33)
                let sp = debug_avr_read_sp(self.probe, self.chip, &mut self.state.debug_state)
                    .map_err(Error::Probe)?;
                Ok(RegisterValue::U32(sp as u32))
            }
            34 => {
                // PC (GDB register 34)
                let pc = debug_avr_read_pc(self.probe, self.chip, &mut self.state.debug_state)
                    .map_err(Error::Probe)?;
                Ok(RegisterValue::U32(pc))
            }
            _ => Err(Error::Other(format!("AVR: unknown register id {}", id))),
        }
    }

    fn write_core_reg(&mut self, _address: RegisterId, _value: RegisterValue) -> Result<(), Error> {
        Err(Error::NotImplemented(
            "AVR: register writes not yet supported",
        ))
    }

    fn available_breakpoint_units(&mut self) -> Result<u32, Error> {
        Ok(1)
    }

    fn hw_breakpoints(&mut self) -> Result<Vec<Option<u64>>, Error> {
        Ok(vec![self.state.debug_state.hw_breakpoint])
    }

    fn enable_breakpoints(&mut self, _state: bool) -> Result<(), Error> {
        Ok(())
    }

    fn set_hw_breakpoint(&mut self, unit_index: usize, addr: u64) -> Result<(), Error> {
        if unit_index != 0 {
            return Err(Error::Other(format!(
                "AVR: breakpoint unit {} out of range (max 1)",
                unit_index
            )));
        }
        let addr32 = u32::try_from(addr).map_err(|_| {
            Error::Other(format!(
                "AVR breakpoint address {addr:#x} exceeds 32-bit range"
            ))
        })?;
        debug_avr_hw_break_set(
            self.probe,
            self.chip,
            &mut self.state.debug_state,
            unit_index as u8,
            addr32,
        )
        .map_err(Error::Probe)?;
        self.state.debug_state.hw_breakpoint = Some(addr);
        Ok(())
    }

    fn clear_hw_breakpoint(&mut self, unit_index: usize) -> Result<(), Error> {
        if unit_index != 0 {
            return Err(Error::Other(format!(
                "AVR: breakpoint unit {} out of range (max 1)",
                unit_index
            )));
        }
        // Clear the EDBG-side breakpoint but keep hw_breakpoint address
        // so run() can re-use it with run_to_address.
        // GDB removes/re-inserts breakpoints around each continue cycle.
        // Setting hw_breakpoint to None here would lose the address for run_to_address.
        debug_avr_hw_break_clear(
            self.probe,
            self.chip,
            &mut self.state.debug_state,
            unit_index as u8,
        )
        .map_err(Error::Probe)?;
        self.state.debug_state.hw_breakpoint = None;
        Ok(())
    }

    fn registers(&self) -> &'static CoreRegisters {
        &AVR_CORE_REGISTERS
    }

    fn program_counter(&self) -> &'static CoreRegister {
        &AVR_PC
    }

    fn frame_pointer(&self) -> &'static CoreRegister {
        &AVR_R28
    }

    fn stack_pointer(&self) -> &'static CoreRegister {
        &AVR_SP
    }

    fn return_address(&self) -> &'static CoreRegister {
        &AVR_RA
    }

    fn hw_breakpoints_enabled(&self) -> bool {
        self.state.debug_state.in_debug_mode
    }

    fn architecture(&self) -> Architecture {
        Architecture::Avr
    }

    fn core_type(&self) -> CoreType {
        CoreType::Avr
    }

    fn instruction_set(&mut self) -> Result<crate::InstructionSet, Error> {
        Ok(crate::InstructionSet::Avr)
    }

    fn fpu_support(&mut self) -> Result<bool, Error> {
        Ok(false)
    }

    fn floating_point_register_count(&mut self) -> Result<usize, Error> {
        Ok(0)
    }

    fn reset_catch_set(&mut self) -> Result<(), Error> {
        Err(Error::NotImplemented("AVR: reset catch"))
    }

    fn reset_catch_clear(&mut self) -> Result<(), Error> {
        Err(Error::NotImplemented("AVR: reset catch"))
    }

    fn debug_core_stop(&mut self) -> Result<(), Error> {
        debug_avr_cleanup(self.probe, self.chip, &mut self.state.debug_state).map_err(Error::Probe)
    }
}
