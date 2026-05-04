//! Host-side flash sequence for AVR UPDI targets.
//!
//! AVR chips have no on-chip flash algorithm: page programming is driven entirely
//! from the host through UPDI/EDBG commands. This sequence plugs into the
//! `HostSideFlasher` path so AVR participates in the standard `FlashLoader`
//! erase/program/verify pipeline.

use crate::Error;
use crate::flashing::DebugFlashSequence;
use crate::memory::MemoryInterface;
use crate::session::Session;

/// AVR UPDI flash sequence.
///
/// Erase delegates to `Session::erase_all` (which routes to `UpdiInterface::erase_chip`
/// on AVR). Program/verify go through `Core::write_8` / `Core::read_8`, which the AVR
/// `MemoryInterface` translates into UPDI page-write / page-read transactions.
#[derive(Debug, Default)]
pub struct AvrFlashSequence;

impl AvrFlashSequence {
    /// Create a new AVR flash sequence.
    pub fn new() -> Self {
        Self
    }
}

impl DebugFlashSequence for AvrFlashSequence {
    fn erase_all(&self, session: &mut Session) -> Result<(), Error> {
        session.erase_all()
    }

    fn program(&self, session: &mut Session, address: u64, data: &[u8]) -> Result<(), Error> {
        let mut core = session.core(0)?;
        core.write_8(address, data)?;
        Ok(())
    }

    fn verify(&self, session: &mut Session, address: u64, data: &[u8]) -> Result<bool, Error> {
        let mut core = session.core(0)?;
        let mut readback = vec![0u8; data.len()];
        core.read_8(address, &mut readback)?;
        Ok(readback == data)
    }

    fn supports_chip_erase(&self) -> bool {
        true
    }

    /// AVR UPDI integrates page-erase with page-write — there is no separate
    /// per-sector erase command. The loader will issue one chip-erase before
    /// the per-page program loop instead.
    fn supports_sector_erase(&self) -> bool {
        false
    }
}
